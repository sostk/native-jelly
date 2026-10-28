//! Rust replacement for src/img.c — image decode + GL upload, same C ABI (img.h).
//! Runs on the poster worker threads (decode) + the GL/main thread (upload).
//!
//! **What the panic guard actually covers.** The `catch_unwind` below turns a decoder *panic* — a
//! bounds or arithmetic failure inside the pure-Rust JPEG/PNG decoders on a truncated or malformed
//! file — into a NULL return, so a bad poster is a missing tile instead of a dead app, and so no
//! unwind reaches the C caller. It does **not** make decoding "never crash the app", which is what
//! this doc used to claim flatly: Rust's allocation-failure path is `handle_alloc_error`, which
//! **aborts** the process. An abort unwinds nothing, so it sails straight past every `catch_unwind`
//! in the tree — and an unbounded decode on a device whose manifest declares `requiredMemory: 160`
//! is exactly how you reach one. Bounding the allocation is therefore a separate, explicit job that
//! the guard cannot do for us; see [`decode_limits`].
use std::os::raw::{c_int, c_uchar, c_uint, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
}

use nj_base::eventlog::log;

/// The decode budget, sized for THIS device. `image`'s defaults are not one: `max_image_width` and
/// `max_image_height` default to `None` — no dimension cap at all — and `max_alloc` to **512 MiB**,
/// which a 32-bit ARM TV declaring `requiredMemory: 160` in `pkg/appinfo.json` cannot honour and
/// would abort trying to (see the module doc on why the `catch_unwind` cannot save us there).
///
/// Every image the app fetches is a `/photo/:/transcode?width=W&height=H&minSize=1` request whose
/// W×H **we** choose, so the legitimate ceiling is knowable rather than guessed:
///
/// | call site | box requested |
/// |---|---|
/// | `ui/detail.rs` hero art | 1920×1080 |
/// | `screens/home` hero backdrop | 1280×720 |
/// | `app/adapters/poster.rs` clearLogo (PNG) | 600×240 (`app::adapters::poster::LOGO_REQ_W`/`LOGO_REQ_H`) |
/// | `appkit/info_panel.rs` episode still | 480×270 |
/// | `ui/widgets.rs` catalog poster | 250×375 |
/// | Jellyfin trickplay sheet | up to 4096 per axis and 6,291,456 pixels; typically 3200×1800 |
///
/// — plus the plex.tv sign-in QR PNG, a few hundred pixels square. Ordinary artwork asks at most
/// 1920×1080, while trickplay uses its separately validated sheet layout. `minSize=1` means *cover*, not *fit*, so
/// the server scales until the box is filled and an extreme source aspect comes back longer on its
/// long edge (a 2.4:1 backdrop into a 16:9 box → ~2592×1080; a 2:3 poster into it → 1920×2880).
/// **4096 on each axis** is over twice the requested box, so no legitimate cover-scaled result can
/// trip it, while the classic decompression bomb (a PNG header declaring 64000×64000) is rejected
/// from its HEADER — before a single pixel buffer is allocated, which is the whole point of a
/// strict dimension limit over a byte budget alone.
///
/// **32 MiB** is the byte budget and the tighter of the two gates (4096² would need ~50 MiB even at
/// 3 bytes/px, so a bomb that squeaks under the dimension caps still dies here). It sits against a
/// largest-legitimate decode of ~21 MiB (1920×2880 RGBA, the cover-scaled poster case) and ~6 MiB
/// for the common 1920×1080 RGB JPEG, or ~16.5 MiB for a default trickplay sheet. Note it bounds the DECODER's allocations only — the
/// `to_rgba8()` conversion and the `malloc`'d copy handed back to the caller are ours and sit
/// outside it, which is a second reason to keep this well under the device's headroom, not at it.
fn decode_limits() -> image::Limits {
    // Field assignment rather than a struct literal: `Limits` is `#[non_exhaustive]`.
    let mut l = image::Limits::default();
    l.max_image_width = Some(4096);
    l.max_image_height = Some(4096);
    l.max_alloc = Some(32 * 1024 * 1024);
    // The simulator's supersampled renders request up to 4x these boxes, on a host with the memory.
    #[cfg(feature = "hostsim")]
    if nj_base::surface::render_scale() > 1 {
        l.max_image_width = Some(16384);
        l.max_image_height = Some(16384);
        l.max_alloc = Some(512 * 1024 * 1024);
    }
    l
}

/// Decode compressed bytes into an OWNED RGBA buffer under the same limits, panic guard and
/// failure lines as [`img_decode_rgba`] — for a worker that composes pixels itself (the collection
/// fan baker) rather than handing a `malloc`'d buffer across the store.
pub fn img_decode_owned(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if data.is_empty() {
        return None;
    }
    let len = data.len();
    // decode inside catch_unwind so a decoder panic can't unwind into C. `ImageReader` rather than
    // the `load_from_memory` one-liner purely so the limits above can be attached — that helper
    // hard-codes `Limits::default()` internally, with no way to pass any.
    //
    // The failure now carries its REASON, which it did not have to before: with limits in force,
    // "no image came back" covers both a corrupt file and one this device refuses to decode, and
    // those want opposite responses (ignore it vs. re-open the numbers in `decode_limits`). Same
    // `img: decode-none` prefix, so anything grepping the event log for it still matches.
    let decoded = catch_unwind(AssertUnwindSafe(
        || -> Result<(u32, u32, Vec<u8>), String> {
            let mut rdr = image::ImageReader::new(std::io::Cursor::new(data))
                .with_guessed_format()
                .map_err(|e| format!("unreadable: {e}"))?;
            rdr.limits(decode_limits());
            let img = rdr.decode().map_err(|e| e.to_string())?;
            let r = img.to_rgba8();
            Ok((r.width(), r.height(), r.into_raw()))
        },
    ));
    match decoded {
        Ok(Ok(t)) => Some(t),
        Ok(Err(why)) => {
            log(&format!(
                "img: decode-none len={len} magic={} — {why}",
                magic(data)
            ));
            None
        }
        Err(_) => {
            log(&format!("img: PANIC len={len} magic={}", magic(data)));
            None
        }
    }
}

/// The first six bytes as hex, for a failure line only — never built on the success path.
fn magic(data: &[u8]) -> String {
    data.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Copy owned RGBA into a `malloc`'d buffer the poster store can hold and [`img_free`] releases.
/// Null on an empty input or a refused allocation; `detail` names the picture on the failure line
/// and is only called there.
///
/// The exit that is not the decoder's: the picture decoded, and it is the copy out into the
/// `malloc`'d buffer the caller owns that came back null. The SIZE is the field worth having.
/// `decode_limits` names this very allocation as one of the two that sit OUTSIDE the budget it
/// sets, and puts the largest legitimate decode at ~21 MiB (1920×2880 RGBA) — on a set whose
/// `pkg/appinfo.json` declares `requiredMemory: 160`. So the byte count answers the first question
/// a reader has: one outsized request, or a heap already full. Same `img:` prefix as the decoder's
/// two failures, so one `img:` grep finds this line and those.
pub fn img_malloc_copy(rgba: &[u8], detail: impl FnOnce() -> String) -> *mut c_uchar {
    if rgba.is_empty() {
        return ptr::null_mut();
    }
    let n = rgba.len();
    let px = unsafe { malloc(n) } as *mut c_uchar;
    if px.is_null() {
        // MARKER FIRST, and it takes no allocation: `log` borrows a `&str`, so a literal reaches
        // the file with nothing asked of the heap. The detailed line below goes through `format!`,
        // which allocates — from the same heap that just refused `n` bytes. For the common failure
        // (one outsized request against a heap with room for small ones) that is fine and the
        // second line lands. For the failure a reader most wants explained — a heap actually
        // exhausted — Rust's allocation-error handler ABORTS, so the detailed line would take the
        // process down in place of the diagnosis it was added to give. Ordered this way, the marker
        // survives either outcome, and a marker with no detail after it IS the second diagnosis.
        log("img: malloc-none");
        log(&format!("img: malloc-none {n} bytes for {}", detail()));
        return ptr::null_mut();
    }
    // SAFETY: `px` was just allocated with exactly `n` bytes and cannot overlap `rgba`.
    unsafe { ptr::copy_nonoverlapping(rgba.as_ptr(), px, n) };
    px
}

/// Encode OPAQUE RGBA as an RGB PNG, the format baked artwork is persisted in: the alpha channel
/// is dropped (a quarter of the pixels to deflate, none of them information) and
/// [`img_decode_owned`] restores it as 255. `None` if the encoder refuses.
pub fn img_encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(enc, &rgb, w, h, image::ExtendedColorType::Rgb8).ok()?;
    Some(out)
}

pub fn img_decode_rgba(
    buf: *const c_uchar,
    len: c_int,
    w: *mut c_int,
    h: *mut c_int,
) -> *mut c_uchar {
    if buf.is_null() || len <= 0 {
        return ptr::null_mut();
    }
    let data = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    let Some((iw, ih, raw)) = img_decode_owned(data) else {
        return ptr::null_mut();
    };
    let px = img_malloc_copy(&raw, || {
        format!("{iw}x{ih} len={len} magic={}", magic(data))
    });
    if px.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        if !w.is_null() {
            *w = iw as c_int;
        }
        if !h.is_null() {
            *h = ih as c_int;
        }
    }
    px
}

pub fn img_free(px: *mut c_uchar) {
    if !px.is_null() {
        unsafe { free(px as *mut c_void) }
    }
}

/// Upload decoded RGBA pixels into a fresh GL texture (gfx owns the GL bindings). Main thread.
pub fn img_upload_rgba(px: *const c_uchar, w: c_int, h: c_int) -> c_uint {
    if px.is_null() || w <= 0 || h <= 0 {
        return 0;
    }
    crate::gfx::upload_rgba(0, w, h, px)
}

fn decode_sheet(data: &[u8], max_w: u32, max_h: u32) -> Option<image::DynamicImage> {
    if data.is_empty()
        || max_w == 0
        || max_h == 0
        || max_w > 4096
        || max_h > 4096
        || u64::from(max_w) * u64::from(max_h) > 6_291_456
    {
        return None;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let reader = || {
            image::ImageReader::new(std::io::Cursor::new(data))
                .with_guessed_format()
                .ok()
        };
        let (iw, ih) = reader()?.into_dimensions().ok()?;
        if iw > max_w || ih > max_h {
            return None;
        }
        let mut rdr = reader()?;
        let mut limits = decode_limits();
        limits.max_image_width = Some(max_w);
        limits.max_image_height = Some(max_h);
        rdr.limits(limits);
        rdr.decode().ok()
    }))
    .ok()
    .flatten()
}

/// Bounded opaque CPU sheet, at most 6,291,456 RGB pixels (18 MiB). Never a GPU texture.
/// Trickplay JPEGs have no alpha; keeping RGB avoids a full-sheet RGBA allocation.
pub struct RgbSheet(image::RgbImage);
impl RgbSheet {
    pub fn decode(data: &[u8], max_w: u32, max_h: u32) -> Option<Self> {
        Some(Self(decode_sheet(data, max_w, max_h)?.into_rgb8()))
    }
    pub fn crop(&self, region: (u32, u32, u32, u32)) -> Option<(u32, u32, Vec<u8>)> {
        let (x, y, w, h) = region;
        if w == 0
            || h == 0
            || x.checked_add(w)? > self.0.width()
            || y.checked_add(h)? > self.0.height()
        {
            return None;
        }
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for row in y..y + h {
            let start = ((row * self.0.width() + x) * 3) as usize;
            for rgb in self.0.as_raw()[start..start + w as usize * 3].chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        Some((w, h, rgba))
    }
}

/// Worker-only one-off region decode with the same header and allocation limits.
pub fn img_decode_region(
    data: &[u8],
    max_w: u32,
    max_h: u32,
    region: (u32, u32, u32, u32),
) -> Option<(u32, u32, Vec<u8>)> {
    let (x, y, w, h) = region;
    let img = decode_sheet(data, max_w, max_h)?;
    if w == 0 || h == 0 || x.checked_add(w)? > img.width() || y.checked_add(h)? > img.height() {
        return None;
    }
    Some((w, h, img.crop_imm(x, y, w, h).to_rgba8().into_raw()))
}

#[cfg(test)]
mod preview_region_tests {
    use super::*;
    #[test]
    fn trickplay_decode_crops_the_correct_pixels_and_rejects_bounds() {
        let rgba: Vec<u8> = (0..16).collect();
        let png = img_encode_png(2, 2, &rgba).unwrap();
        let (_, _, tile) = img_decode_region(&png, 2, 2, (1, 1, 1, 1)).unwrap();
        assert_eq!(&tile[..3], &rgba[12..15]);
        assert!(img_decode_region(&png, 2, 2, (2, 0, 1, 1)).is_none());
        assert!(img_decode_region(&png, 1, 1, (0, 0, 1, 1)).is_none());
        assert!(img_decode_region(&png, 4096, 4096, (0, 0, 1, 1)).is_none());
        assert!(img_decode_region(b"broken", 2, 2, (0, 0, 1, 1)).is_none());
        let sheet = RgbSheet::decode(&png, 2, 2).unwrap();
        assert_eq!(sheet.0.as_raw().len(), 2 * 2 * 3);
        assert_eq!(&sheet.crop((1, 1, 1, 1)).unwrap().2[..3], &rgba[12..15]);
        assert!(sheet.crop((u32::MAX, 0, 1, 1)).is_none());
        assert!(sheet.crop((0, 0, 0, 1)).is_none());
        assert!(sheet.crop((1, 1, 2, 1)).is_none());
        assert!(RgbSheet::decode(&png, 1, 1).is_none());
        assert!(RgbSheet::decode(&png, 4096, 4096).is_none());
        assert!(RgbSheet::decode(b"broken", 2, 2).is_none());
    }
}

//! Real GL regression for the residency warm: it must leave the visible/page pixels intact.
//! Run explicitly under a display (xvfb-run on Linux); ordinary host tests have no GL context.
use super::*;

extern "C" {
    fn SDL_Init(flags: u32) -> c_int;
    fn SDL_Quit();
    fn SDL_GL_SetAttribute(attr: c_int, value: c_int) -> c_int;
    fn SDL_CreateWindow(title: *const c_char, x: c_int, y: c_int, w: c_int, h: c_int, flags: u32) -> *mut c_void;
    fn SDL_DestroyWindow(window: *mut c_void);
    fn SDL_GL_CreateContext(window: *mut c_void) -> *mut c_void;
    fn SDL_GL_DeleteContext(context: *mut c_void);
    fn glGetIntegerv(name: c_uint, value: *mut c_int);
}

struct Context {
    window: *mut c_void,
    gl: *mut c_void,
}

impl Context {
    unsafe fn new() -> Self {
        assert_eq!(SDL_Init(0x20), 0, "SDL video initialization");
        #[cfg(target_os = "macos")]
        let (profile, major, minor) = (1, 4, 1); // desktop core profile
        #[cfg(not(target_os = "macos"))]
        let (profile, major, minor) = (4, 2, 0); // GLES2, like the Linux simulator and TV
        for (attr, value) in [(21, profile), (17, major), (18, minor)] {
            assert_eq!(SDL_GL_SetAttribute(attr, value), 0);
        }
        let window = SDL_CreateWindow(b"warm regression\0".as_ptr().cast(), 0, 0, 1920, 1080, 0x2 | 0x8);
        assert!(!window.is_null(), "hidden GL window");
        let gl = SDL_GL_CreateContext(window);
        assert!(!gl.is_null(), "current GL context");
        nj_base::surface::probe(window);
        WARM_TARGET = Some(None); // a previous context's refusal must not poison a fresh one
        init_gl();
        assert!((*std::ptr::addr_of!(WARM_TARGET)).is_none(), "new context resets the warm target");
        init_image();
        let (x, y, w, h) = nj_base::surface::viewport();
        glViewport(x, y, w, h);
        Self { window, gl }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            SDL_GL_DeleteContext(self.gl);
            SDL_DestroyWindow(self.window);
            SDL_Quit();
        }
    }
}

unsafe fn pixels() -> Vec<u8> {
    let (x, y, w, h) = nj_base::surface::viewport();
    let mut pixels = vec![0; w as usize * h as usize * 4];
    glReadPixels(x, y, w, h, GL_RGBA, GL_UNSIGNED_BYTE, pixels.as_mut_ptr().cast());
    assert_eq!(glGetError(), GL_NO_ERROR, "pixel readback");
    pixels
}

unsafe fn assert_target_untouched(tex: c_uint) {
    glClearColor(0.125, 0.25, 0.5, 1.0);
    glClear(GL_COLOR_BUFFER_BIT);
    let before = pixels();
    warm_tex(tex);
    assert_eq!(glGetError(), GL_NO_ERROR, "warm GL commands");
    let mut bound = -1;
    glGetIntegerv(0x8CA6, &mut bound); // GL_FRAMEBUFFER_BINDING
    assert_eq!(bound as u32, nj_base::surface::default_fb(), "restore the current page target");
    let mut view = [0; 4];
    glGetIntegerv(0x0BA2, view.as_mut_ptr()); // GL_VIEWPORT
    let (x, y, w, h) = nj_base::surface::viewport();
    assert_eq!(view, [x, y, w, h], "restore the authored viewport");
    let changed = before.chunks_exact(4).zip(pixels().chunks_exact(4)).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 0, "warm-up must never draw into the window or page framebuffer");
}

#[test]
#[ignore = "requires a real SDL/OpenGL context; run with xvfb-run on Linux"]
fn warming_a_texture_preserves_window_and_redirected_page_pixels() {
    let _serial = nj_base::testlock::serial();
    unsafe {
        let _context = Context::new();
        warm_tex(0);
        assert!((*std::ptr::addr_of!(WARM_TARGET)).is_none(), "a missing texture allocates nothing");
        let tex = upload_rgba(0, 250, 375, vec![255u8; 250 * 375 * 4].as_ptr());
        assert_ne!(tex, 0);
        assert_target_untouched(tex);
        let target = (*std::ptr::addr_of!(WARM_TARGET)).flatten().expect("dedicated warm target");
        assert_ne!(target.0, 0);
        assert_ne!(target.1, 0);
        let (_, fbo) = fbo_target(1920, 1080, "warm regression page").expect("complete page target");
        let _page = nj_base::surface::PageTarget::enter(fbo);
        assert_target_untouched(tex);
        assert_eq!((*std::ptr::addr_of!(WARM_TARGET)).flatten(), Some(target), "reuse the small target");

        // Masking visible images must not suppress the offscreen residency draw.
        crate::overdraw::set_mask("image");
        let red = upload_rgba(0, 1, 1, [255u8, 0, 0, 255].as_ptr());
        assert_target_untouched(red);
        glBindFramebuffer(GL_FRAMEBUFFER, target.1);
        let mut sampled = [0u8; 4];
        glReadPixels(0, 0, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, sampled.as_mut_ptr().cast());
        glBindFramebuffer(GL_FRAMEBUFFER, nj_base::surface::default_fb());
        assert_eq!(sampled, [255, 0, 0, 255], "the warm sampled the new texture despite the image mask");
        crate::overdraw::set_mask("");

        // Model a refused allocation: repeated calls stay disabled, with no visible fallback.
        WARM_TARGET = Some(None);
        assert_target_untouched(tex);
        assert_target_untouched(tex);
        assert_eq!(*std::ptr::addr_of!(WARM_TARGET), Some(None), "a refusal stays latched");
    }
}

/// Compare actual GLES pixels, not only the pure route/partition helpers. Explicitly exercise
/// the optional-program fallback and keep the caller's clip across the multi-draw card path.
#[test]
#[ignore = "requires a real SDL/OpenGL context; run with xvfb-run on Linux"]
fn card_split_and_optional_programs_preserve_pixels_and_clips() {
    let _serial = nj_base::testlock::serial();
    unsafe {
        let _context = Context::new();
        assert!(RPROG != 0, "the ramp specialization compiled");
        let plain = std::ptr::addr_of!(PLAIN_IMAGE).read();
        assert!(plain.is_some(), "the plain texture specialization compiled");
        let data: Vec<u8> = (0..64 * 64).flat_map(|i|
            [(i % 64 * 4) as u8, (i / 64 * 4) as u8, ((i * 37) % 256) as u8, 210]).collect();
        let tex = upload_rgba(0, 64, 64, data.as_ptr());
        let tint = [0.8, 0.7, 0.9, 0.85];
        let rim = [0.5, 0.6, 0.7, 0.3];
        let shadow = [0.0, 0.0, 0.0, 0.4];
        let scissor = || {
            let mut enabled = 0; let mut bounds = [0; 4];
            glGetIntegerv(GL_SCISSOR_TEST, &mut enabled);
            glGetIntegerv(0x0C10, bounds.as_mut_ptr()); // GL_SCISSOR_BOX
            (enabled, bounds)
        };
        let compare = |a: Vec<u8>, b: Vec<u8>, label: &str| {
            let max = a.iter().zip(&b).map(|(&a, &b)| a.abs_diff(b)).max().unwrap();
            assert!(max <= 1, "{label}: greatest channel delta {max}");
        };
        let cards = |split: bool, use_plain: bool, clipped: bool, still: bool| {
            crate::overdraw::set_card_split(split);
            PLAIN_IMAGE = if use_plain { plain } else { None };
            clip_clear(); frame_clear(0.17, 0.25, 0.33);
            if clipped { clip_set(150.25, 140.75, 800.5, 500.25); }
            let before = scissor();
            for (x, y, w, h, radius) in [(123.25, 152.5, 250.0, 375.0, 14.0),
                (430.75, 223.25, 324.0, 182.0, 14.0), (800.25, 350.75, 190.0, 190.0, 95.0)] {
                if still {
                    assert!(draw_tex_carded_still(tex, [0.1, 0.05, 0.8, 0.9], x, y, w, h, radius,
                        [1.0; 4], 1.0, rim, 15.0, 14.0, shadow, 60.0, [0.01, 0.02, 0.03, 0.8], 0.0, 0.0));
                } else {
                    draw_tex_carded(tex, [0.1, 0.05, 0.8, 0.9], x, y, w, h, radius,
                        tint.as_ptr(), 1.0, rim.as_ptr(), 15.0, 14.0, shadow.as_ptr(), 0.0, 0.0);
                }
            }
            let after = scissor();
            assert_eq!(before.0, after.0, "restore the caller's scissor enable");
            if before.0 != 0 { assert_eq!(before.1, after.1, "restore the caller's clip box"); }
            let result = pixels(); clip_clear(); result
        };
        // The same partition must work on the window and a redirected full-resolution page.
        let (page_tex, page_fbo) = fbo_target(1920, 1080, "card comparison").unwrap();
        for redirected in [false, true] {
            let page = redirected.then(|| nj_base::surface::PageTarget::enter(page_fbo));
            for clipped in [false, true] {
                for still in [false, true] {
                    let old = cards(false, false, clipped, still);
                    compare(old.clone(), cards(true, false, clipped, still), "split with legacy program");
                    compare(old, cards(true, true, clipped, still), "split with optional plain program");
                }
            }
            drop(page);
        }
        // A source pass uses a quarter-resolution viewport. Splitting is refused there, and
        // the original source clip must survive every card draw and clip_clear.
        glBindFramebuffer(GL_FRAMEBUFFER, page_fbo);
        glViewport(-10, -20, 480, 270);
        let source = |split| {
            // The target allocation exceeds the live source region; initialize its unused
            // pixels as well so readback does not compare residue from the previous draw.
            CLIP_TARGET = None; clip_clear(); frame_clear(0.17, 0.25, 0.33);
            CLIP_TARGET = Some((-10, -20, 0.25, 300, 180));
            cards(split, split, true, false)
        };
        compare(source(false), source(true), "reduced source-pass fallback");
        assert_eq!(scissor(), (1, [0, 0, 300, 180]), "source clip_clear restores the region clamp");
        drop(DirectPass);
        glDeleteFramebuffers(1, &page_fbo);
        delete_tex(page_tex);
        // A plain image (including a tinted, cropped image) follows exactly the old shader exit.
        let image = |enabled| {
            PLAIN_IMAGE = if enabled { plain } else { None };
            frame_clear(0.17, 0.25, 0.33);
            draw_tex_uv(tex, [0.1, 0.05, 0.8, 0.9], 100.25, 125.5, 900.0, 700.0, 0.0, tint.as_ptr());
            pixels()
        };
        compare(image(false), image(true), "plain image specialization");
        let ramp = RPROG;
        let gradient = |enabled| {
            RPROG = if enabled { ramp } else { 0 };
            frame_clear(0.17, 0.25, 0.33);
            draw_rect(71.25, 23.5, 1700.0, 970.0, 0.0, 0.0,
                [0.1, 0.2, 0.3, 0.7].as_ptr(), [0.4, 0.2, 0.1, 0.1].as_ptr(), 0.0, 0.0);
            pixels()
        };
        compare(gradient(false), gradient(true), "ramp specialization and fallback");
        RPROG = ramp; PLAIN_IMAGE = plain;
        crate::overdraw::set_card_split(true);
        delete_tex(tex);
    }
}

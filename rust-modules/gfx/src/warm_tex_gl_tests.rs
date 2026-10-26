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

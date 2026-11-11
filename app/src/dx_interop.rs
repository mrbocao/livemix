//! Windows OpenGL to D3D11 Program transfer. The caller owns the current WGL context.
use eframe::glow::{self, HasContext};
use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::{Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC}, IDXGIDevice};

type Handle = *mut c_void;
type Close = unsafe extern "system" fn(Handle) -> i32;
type Unregister = unsafe extern "system" fn(Handle, Handle) -> i32;
type Access = unsafe extern "system" fn(Handle, i32, *const Handle) -> i32;

#[link(name = "opengl32")]
extern "system" {
    fn wglGetProcAddress(name: *const u8) -> *const c_void;
}

pub(crate) struct Surface(pub(crate) ID3D11Texture2D);

pub(crate) struct Picture {
    pub surface: Surface,
    pub size: [usize; 2],
}

// D3D11 resources are free-threaded. Once WGL unregisters the texture the
// producer no longer modifies it, and Media Foundation owns a reference.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

pub(crate) struct Bridge {
    device: ID3D11Device,
    interop: Handle,
    close: Close,
    register: unsafe extern "system" fn(Handle, Handle, u32, u32, u32) -> Handle,
    unregister: Unregister,
    lock: Access,
    unlock: Access,
}

// The render callback uses the bridge only while its WGL context is current.
// Target::destroy closes it on that same context.
unsafe impl Send for Bridge {}

impl Bridge {
    pub(crate) unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        macro_rules! load {
            ($name:literal) => {{
                let address = wglGetProcAddress(concat!($name, "\0").as_ptr());
                if address.is_null() || (address as usize) <= 3 || address as usize == usize::MAX {
                    return Err("The OpenGL driver does not expose D3D11 interop".into());
                }
                std::mem::transmute(address)
            }};
        }
        let open: unsafe extern "system" fn(Handle) -> Handle = load!("wglDXOpenDeviceNV");
        let close = load!("wglDXCloseDeviceNV");
        let register = load!("wglDXRegisterObjectNV");
        let unregister = load!("wglDXUnregisterObjectNV");
        let lock = load!("wglDXLockObjectsNV");
        let unlock = load!("wglDXUnlockObjectsNV");
        let device = crate::media::shared_d3d11_device().map_err(|e| e.to_string())?;
        let adapter = device.cast::<IDXGIDevice>().and_then(|dxgi| dxgi.GetAdapter())
            .and_then(|adapter| adapter.GetDesc()).map_err(|e| e.to_string())?;
        let name = String::from_utf16_lossy(&adapter.Description).trim_end_matches('\0').to_owned();
        let renderer = gl.get_parameter_string(glow::RENDERER);
        if !crate::media::renderer_matches_adapter(&renderer, &name) {
            return Err(format!("OpenGL uses {renderer} and D3D11 uses {name}"));
        }
        let interop = open(device.as_raw());
        if interop.is_null() { return Err("The selected D3D11 device could not be opened by OpenGL".into()); }
        Ok(Self { device, interop, close, register, unregister, lock, unlock })
    }

    pub(crate) unsafe fn copy(&self, gl: &glow::Context, source: glow::Framebuffer, [width, height]: [u32; 2])
        -> Result<Surface, String>
    {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width, Height: height, MipLevels: 1, ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let mut surface = None;
        self.device.CreateTexture2D(&desc, None, Some(&mut surface)).map_err(|e| e.to_string())?;
        let surface = surface.ok_or("D3D11 did not allocate a texture")?;
        let texture = gl.create_texture()?;
        let framebuffer = match gl.create_framebuffer() {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                gl.delete_texture(texture);
                return Err(error);
            }
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let object = (self.register)(self.interop, surface.as_raw(), texture.0.get(), glow::TEXTURE_2D, 0x0001);
        let result = if object.is_null() {
            Err("OpenGL could not register the D3D11 texture")
        } else if (self.lock)(self.interop, 1, &object) == 0 {
            (self.unregister)(self.interop, object);
            Err("OpenGL could not lock the D3D11 texture")
        } else {
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            let complete = gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            if complete {
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(source));
                gl.disable(glow::SCISSOR_TEST);
                let (w, h) = (width as i32, height as i32);
                gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            }
            let copied = complete && gl.get_error() == glow::NO_ERROR;
            gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, None, 0);
            let unlocked = (self.unlock)(self.interop, 1, &object) != 0;
            let unregistered = (self.unregister)(self.interop, object) != 0;
            if copied && unlocked && unregistered { Ok(Surface(surface)) }
            else { Err("OpenGL could not transfer the Program texture to D3D11") }
        };
        gl.delete_framebuffer(framebuffer);
        gl.delete_texture(texture);
        result.map_err(str::to_owned)
    }

    pub(crate) unsafe fn close(self) {
        (self.close)(self.interop);
    }

    pub(crate) unsafe fn import(&self, gl: &glow::Context, picture: &Picture, target: glow::Texture,
        resize: bool) -> Result<(), String>
    {
        let [w, h] = picture.size.map(|value| value as i32);
        let previous_draw = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        let previous_texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
        let scissor = gl.is_enabled(glow::SCISSOR_TEST);
        let texture = gl.create_texture()?;
        let fbo = match gl.create_framebuffer() {
            Ok(fbo) => fbo,
            Err(error) => { gl.delete_texture(texture); return Err(error); }
        };
        let target_fbo = match gl.create_framebuffer() {
            Ok(fbo) => fbo,
            Err(error) => { gl.delete_framebuffer(fbo); gl.delete_texture(texture); return Err(error); }
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let object = (self.register)(self.interop, picture.surface.0.as_raw(), texture.0.get(), glow::TEXTURE_2D, 0);
        let result = if object.is_null() {
            Err("OpenGL could not register the decoded D3D11 texture")
        } else {
            let locked = (self.lock)(self.interop, 1, &object) != 0;
            let mut copied = false;
            if locked {
                if resize {
                    gl.bind_texture(glow::TEXTURE_2D, Some(target));
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA,
                        glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_A, glow::ONE as i32);
                }
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(glow::READ_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(target_fbo));
                gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(target), 0);
                if gl.check_framebuffer_status(glow::READ_FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE
                    && gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE
                {
                    gl.disable(glow::SCISSOR_TEST);
                    gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
                    copied = gl.get_error() == glow::NO_ERROR;
                }
                gl.framebuffer_texture_2d(glow::READ_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, None, 0);
                copied &= (self.unlock)(self.interop, 1, &object) != 0;
            }
            if (self.unregister)(self.interop, object) != 0 && copied { Ok(()) }
            else { Err("OpenGL could not import the decoded D3D11 texture") }
        };
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_draw);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous_read);
        gl.bind_texture(glow::TEXTURE_2D, previous_texture);
        if scissor { gl.enable(glow::SCISSOR_TEST); }
        gl.delete_framebuffer(fbo);
        gl.delete_framebuffer(target_fbo);
        gl.delete_texture(texture);
        result.map_err(str::to_owned)
    }
}

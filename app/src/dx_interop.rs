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
    effects: Option<(glow::Program, glow::VertexArray)>,
    luts: Vec<(std::sync::Weak<crate::colour_fx::Lut>, glow::Texture)>,
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
        Ok(Self { device, interop, close, register, unregister, lock, unlock, effects: None, luts: Vec::new() })
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

    pub(crate) unsafe fn destroy(self, gl: &glow::Context) {
        if let Some((shader, vao)) = self.effects {
            gl.delete_program(shader);
            gl.delete_vertex_array(vao);
        }
        for (_, texture) in &self.luts { gl.delete_texture(*texture); }
        self.close();
    }

    unsafe fn lut_texture(&mut self, gl: &glow::Context, lut: &std::sync::Arc<crate::colour_fx::Lut>)
        -> Result<glow::Texture, String>
    {
        let weak = std::sync::Arc::downgrade(lut);
        self.luts.retain(|(key, texture)| {
            let alive = key.strong_count() > 0;
            if !alive { gl.delete_texture(*texture); }
            alive
        });
        if let Some((_, texture)) = self.luts.iter().find(|(key, _)| key.ptr_eq(&weak)) {
            return Ok(*texture);
        }
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_3D, Some(texture));
        for parameter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
            gl.tex_parameter_i32(glow::TEXTURE_3D, parameter, glow::LINEAR as i32);
        }
        for parameter in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T, glow::TEXTURE_WRAP_R] {
            gl.tex_parameter_i32(glow::TEXTURE_3D, parameter, glow::CLAMP_TO_EDGE as i32);
        }
        let size = lut.size as i32;
        gl.tex_image_3d(glow::TEXTURE_3D, 0, glow::RGB32F as i32, size, size, size, 0, glow::RGB,
            glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&lut.cells))));
        if gl.get_error() != glow::NO_ERROR {
            gl.delete_texture(texture);
            return Err("OpenGL could not upload the 3D LUT".into());
        }
        self.luts.push((weak, texture));
        Ok(texture)
    }

    pub(crate) unsafe fn import(&mut self, gl: &glow::Context, picture: &Picture, target: glow::Texture,
        resize: bool, effects: &Effects) -> Result<(), String>
    {
        let (shader, vao) = match self.effects {
            Some(program) => program,
            None => {
                let shader = crate::program::link(gl, crate::program::PACK_VERTEX, INPUT_EFFECTS)?;
                let vao = match gl.create_vertex_array() {
                    Ok(vao) => vao,
                    Err(error) => { gl.delete_program(shader); return Err(error); }
                };
                self.effects = Some((shader, vao));
                (shader, vao)
            }
        };
        let [w, h] = picture.size.map(|value| value as i32);
        let previous_draw = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        let previous_active = gl.get_parameter_i32(glow::ACTIVE_TEXTURE);
        gl.active_texture(glow::TEXTURE1);
        let previous_lut = gl.get_parameter_texture(glow::TEXTURE_BINDING_3D);
        let lut = effects.extra.lut.as_ref().map(|lut| self.lut_texture(gl, lut)).transpose();
        gl.bind_texture(glow::TEXTURE_3D, previous_lut);
        gl.active_texture(previous_active as u32);
        let lut = lut?;
        let previous_program = gl.get_parameter_program(glow::CURRENT_PROGRAM);
        let previous_vao = gl.get_parameter_vertex_array(glow::VERTEX_ARRAY_BINDING);
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        let scissor = gl.is_enabled(glow::SCISSOR_TEST);
        let blend = gl.is_enabled(glow::BLEND);
        let srgb = gl.is_enabled(glow::FRAMEBUFFER_SRGB);
        let texture = gl.create_texture()?;
        let target_fbo = match gl.create_framebuffer() {
            Ok(fbo) => fbo,
            Err(error) => { gl.delete_texture(texture); return Err(error); }
        };
        gl.active_texture(glow::TEXTURE0);
        let previous_texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
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
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::SRGB8_ALPHA8 as i32, w, h, 0, glow::RGBA,
                        glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
                }
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(target_fbo));
                gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(target), 0);
                if gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE {
                    gl.disable(glow::SCISSOR_TEST);
                    gl.disable(glow::BLEND);
                    gl.disable(glow::FRAMEBUFFER_SRGB);
                    gl.viewport(0, 0, w, h);
                    gl.use_program(Some(shader));
                    gl.bind_vertex_array(Some(vao));
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
                    gl.uniform_1_i32(gl.get_uniform_location(shader, "picture").as_ref(), 0);
                    gl.active_texture(glow::TEXTURE1);
                    gl.bind_texture(glow::TEXTURE_3D, lut);
                    effects.bind(gl, shader);
                    gl.draw_arrays(glow::TRIANGLES, 0, 3);
                    gl.bind_texture(glow::TEXTURE_3D, previous_lut);
                    gl.active_texture(glow::TEXTURE0);
                    copied = gl.get_error() == glow::NO_ERROR;
                }
                gl.bind_texture(glow::TEXTURE_2D, None);
                copied &= (self.unlock)(self.interop, 1, &object) != 0;
            }
            if (self.unregister)(self.interop, object) != 0 && copied { Ok(()) }
            else { Err("OpenGL could not import the decoded D3D11 texture") }
        };
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_draw);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous_read);
        gl.bind_texture(glow::TEXTURE_2D, previous_texture);
        gl.active_texture(previous_active as u32);
        gl.use_program(previous_program);
        gl.bind_vertex_array(previous_vao);
        gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        if scissor { gl.enable(glow::SCISSOR_TEST); }
        if blend { gl.enable(glow::BLEND); }
        if srgb { gl.enable(glow::FRAMEBUFFER_SRGB); }
        gl.delete_framebuffer(target_fbo);
        gl.delete_texture(texture);
        result.map_err(str::to_owned)
    }
}

pub(crate) struct Effects {
    pub key: u64,
    pub colour: crate::model::ColourAdjust,
    pub extra: crate::colour_fx::Effects,
}

impl Effects {
    unsafe fn bind(&self, gl: &glow::Context, program: glow::Program) {
        let uniform = |name| gl.get_uniform_location(program, name);
        let key = self.extra.luma;
        gl.uniform_4_f32(uniform("luma_key").as_ref(), f32::from(key.threshold), f32::from(key.softness),
            if key.invert { -1.0 } else { 1.0 }, f32::from(key.enabled));
        gl.uniform_1_i32(uniform("lut").as_ref(), 1);
        gl.uniform_1_f32(uniform("lut_size").as_ref(), self.extra.lut.as_ref().map_or(0.0, |lut| lut.size as f32));
        if let Some(lut) = &self.extra.lut {
            gl.uniform_3_f32_slice(uniform("lut_min").as_ref(), &lut.min);
            gl.uniform_3_f32_slice(uniform("lut_max").as_ref(), &lut.max);
        }
        let c = self.colour;
        let normal = |value: f32| value.clamp(-1.0, 1.0);
        gl.uniform_4_f32(uniform("chroma_key").as_ref(), (self.key >> 16) as u8 as f32,
            (self.key >> 8) as u8 as f32, self.key as u8 as f32,
            if self.key == 0 { -1.0 } else { (self.key >> 24) as u8 as f32 * 1.7 });
        gl.uniform_4_f32(uniform("adjust").as_ref(), normal(c.brightness), 1.0 + normal(c.contrast),
            1.0 + normal(c.saturation), 1.0 + normal(c.alpha));
        gl.uniform_3_f32(uniform("gain").as_ref(), 1.0 + normal(c.red), 1.0 + normal(c.green), 1.0 + normal(c.blue));
        let black = normal(c.black) * 0.5;
        gl.uniform_2_f32(uniform("levels").as_ref(), black, (1.0 + normal(c.white) * 0.5 - black).max(0.1));
    }
}

const INPUT_EFFECTS: &str = r#"
#version 330 core
uniform sampler2D picture;
uniform vec4 chroma_key;
uniform vec4 luma_key;
uniform sampler3D lut;
uniform float lut_size;
uniform vec3 lut_min;
uniform vec3 lut_max;
uniform vec4 adjust;
uniform vec3 gain;
uniform vec2 levels;
out vec4 result;
void main() {
    vec3 rgb = texelFetch(picture, ivec2(gl_FragCoord.xy), 0).rgb;
    float alpha = 1.0;
    if (chroma_key.w >= 0.0) {
        alpha = clamp((distance(rgb * 255.0, chroma_key.xyz) - chroma_key.w) / 40.0, 0.0, 1.0);
        alpha = floor(alpha * 255.0) / 255.0;
    }
    float luma = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    if (luma_key.w > 0.0) {
        float distance_to_key = (luma * 255.0 - luma_key.x) * luma_key.z;
        float matte = luma_key.y == 0.0 ? step(0.0, distance_to_key) : clamp(distance_to_key / luma_key.y, 0.0, 1.0);
        alpha = floor(alpha * matte * 255.0 + 0.5) / 255.0;
    }
    rgb = vec3(luma) + (rgb - luma) * adjust.z;
    rgb = (rgb - 0.5) * adjust.y + 0.5 + adjust.x;
    rgb = clamp((rgb * gain - levels.x) / levels.y, 0.0, 1.0);
    rgb = floor(rgb * 255.0 + 0.5) / 255.0;
    if (lut_size > 0.0) {
        vec3 p = clamp((rgb - lut_min) / (lut_max - lut_min), 0.0, 1.0);
        rgb = texture(lut, (0.5 + p * (lut_size - 1.0)) / lut_size).rgb;
        rgb = floor(clamp(rgb, 0.0, 1.0) * 255.0 + 0.5) / 255.0;
    }
    alpha = floor(clamp(alpha * adjust.w, 0.0, 1.0) * 255.0 + 0.5) / 255.0;
    vec3 linear = mix(pow((rgb + 0.055) / 1.055, vec3(2.4)), rgb / 12.92,
                      lessThanEqual(rgb, vec3(0.04045))) * alpha;
    vec3 srgb = mix(1.055 * pow(linear, vec3(1.0 / 2.4)) - 0.055, linear * 12.92,
                    lessThanEqual(linear, vec3(0.0031308)));
    result = vec4(srgb, alpha);
}
"#;

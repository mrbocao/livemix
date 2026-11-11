//! Offscreen Program rendering in eframe's OpenGL context, reusing its input textures.

use eframe::{egui, egui_glow, glow::{self, HasContext}};
use std::{collections::VecDeque, sync::Arc, time::Instant};

pub struct Target {
    framebuffer: glow::Framebuffer,
    color: glow::Texture,
    shader: glow::Program,
    vao: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    size_uniform: Option<glow::UniformLocation>,
    size: [u32; 2],
    readbacks: VecDeque<Readback>,
    free_buffers: Vec<PackBuffer>,
    cpu_frames: Vec<Arc<Vec<u8>>>,
    readback_bytes: u64,
    ndi_pack: Option<Result<NdiPack, String>>,
    #[cfg(windows)]
    dx_bridge: Option<Result<crate::dx_interop::Bridge, String>>,
}

struct PackBuffer {
    id: glow::Buffer,
    bytes: i32,
}

struct Readback {
    buffer: PackBuffer,
    fence: glow::Fence,
    size: [u32; 2],
    captured_at: Instant,
    destinations: Destinations,
    uyvy: bool,
}

#[derive(Clone, Default)]
pub struct Destinations {
    pub audio: crate::audio::Packet,
    pub recorders: Vec<Arc<crate::media::Recorder>>,
    pub decklinks: Vec<crate::decklink::OutputSink>,
    pub streams: Vec<crate::stream::Output>,
    pub ndi: Vec<crate::ndi::Sender>,
    pub omt: Vec<crate::omt::Sender>,
    pub scope: Option<(ScopeKey, ScopeMailbox)>,
}

impl Destinations {
    fn only_ndi(&self) -> bool {
        !self.ndi.is_empty() && self.recorders.is_empty() && self.decklinks.is_empty()
            && self.streams.is_empty() && self.omt.is_empty() && self.scope.is_none()
    }
}

struct NdiPack {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    shader: glow::Program,
    coefficients: Option<glow::UniformLocation>,
    size: [u32; 2],
}

const PACK_VERTEX: &str = r#"
#version 330 core
void main() {
    vec2 position = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
}
"#;

// Two source pixels are packed into the four channels of one half-width texel.
const PACK_FRAGMENT: &str = r#"
#version 330 core
uniform sampler2D picture;
uniform vec3 luma_weights;
out vec4 packed_frame;
void main() {
    ivec2 origin = ivec2(gl_FragCoord.xy);
    origin.x *= 2;
    vec3 first = texelFetch(picture, origin, 0).rgb;
    vec3 second = texelFetch(picture, origin + ivec2(1, 0), 0).rgb;
    float ya = dot(first, luma_weights);
    float yb = dot(second, luma_weights);
    vec3 average = (first + second) * 0.5;
    float y = (ya + yb) * 0.5;
    float u = 128.0 + 224.0 * (average.b - y) / (2.0 * (1.0 - luma_weights.b));
    float v = 128.0 + 224.0 * (average.r - y) / (2.0 * (1.0 - luma_weights.r));
    // The readback uses the driver's native BGRA transfer order.
    packed_frame = vec4(v, 16.0 + 219.0 * ya, u, 16.0 + 219.0 * yb) / 255.0;
}
"#;

impl NdiPack {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let shader = link(gl, PACK_VERTEX, PACK_FRAGMENT)?;
        let framebuffer = gl.create_framebuffer().map_err(|e| { gl.delete_program(shader); e })?;
        let texture = gl.create_texture().map_err(|e| {
            gl.delete_framebuffer(framebuffer);
            gl.delete_program(shader);
            e
        })?;
        let coefficients = gl.get_uniform_location(shader, "luma_weights");
        Ok(Self { framebuffer, texture, shader, coefficients, size: [0, 0] })
    }

    unsafe fn pack(&mut self, gl: &glow::Context, source: glow::Texture, [w, h]: [u32; 2]) -> bool {
        let size = [w / 2, h];
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        if size != self.size {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, size[0] as i32, h as i32,
                0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.texture), 0);
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return false;
            }
            self.size = size;
        }
        gl.viewport(0, 0, size[0] as i32, h as i32);
        gl.disable(glow::SCISSOR_TEST);
        // The alpha channel is the second pixel's luma, so it must not blend.
        let blending = gl.is_enabled(glow::BLEND);
        gl.disable(glow::BLEND);
        gl.use_program(Some(self.shader));
        let [r, g, b] = if h >= 720 { [0.2126, 0.7152, 0.0722] } else { [0.299, 0.587, 0.114] };
        gl.uniform_3_f32(self.coefficients.as_ref(), r, g, b);
        gl.bind_texture(glow::TEXTURE_2D, Some(source));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        if blending { gl.enable(glow::BLEND); }
        gl.enable(glow::SCISSOR_TEST);
        true
    }
}

#[derive(Clone)]
pub struct NetworkFrame {
    pub pixels: Arc<Vec<u8>>,
    pub width: usize,
    pub height: usize,
}

pub type ScopeKey = (Option<u32>, u8, bool);
pub type ScopeMailbox = Arc<std::sync::Mutex<Option<ScopeFrame>>>;

pub struct ScopeFrame {
    pub key: ScopeKey,
    pub size: [u32; 2],
    pub pixels: Arc<Vec<u8>>,
}

// SAFETY: the fence is an opaque GL handle. Moving this state does not call GL
// or dereference the handle. It is used only with the originating context current.
unsafe impl Send for Readback {}

const VERTEX: &str = r#"
#version 330 core
layout(location = 0) in vec2 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;
uniform vec2 size;
out vec2 tex_coord;
out vec4 tint;
void main() {
    // Draw the top row at the bottom of the framebuffer so read_pixels returns top-down video.
    gl_Position = vec4(2.0 * position / size - 1.0, 0.0, 1.0);
    tex_coord = uv;
    tint = color;
}
"#;

const FRAGMENT: &str = r#"
#version 330 core
uniform sampler2D picture;
in vec2 tex_coord;
in vec4 tint;
out vec4 color;
void main() {
    vec4 sample_color = texture(picture, tex_coord);
    vec3 linear_rgb = sample_color.rgb;
    vec3 srgb = mix(1.055 * pow(linear_rgb, vec3(1.0 / 2.4)) - 0.055,
                   12.92 * linear_rgb, lessThan(linear_rgb, vec3(0.0031308)));
    color = tint * vec4(srgb, sample_color.a);
}
"#;

unsafe fn compile(gl: &glow::Context, shader: glow::Program, kind: u32, source: &str) -> Result<(), String> {
    let stage = gl.create_shader(kind)?;
    gl.shader_source(stage, source);
    gl.compile_shader(stage);
    let result = if gl.get_shader_compile_status(stage) {
        gl.attach_shader(shader, stage);
        Ok(())
    } else {
        Err(gl.get_shader_info_log(stage))
    };
    gl.delete_shader(stage);
    result
}

unsafe fn link(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    let shader = gl.create_program()?;
    let result = (|| {
        compile(gl, shader, glow::VERTEX_SHADER, vertex)?;
        compile(gl, shader, glow::FRAGMENT_SHADER, fragment)?;
        gl.link_program(shader);
        if gl.get_program_link_status(shader) { Ok(shader) } else { Err(gl.get_program_info_log(shader)) }
    })();
    if result.is_err() { gl.delete_program(shader); }
    result
}

impl Target {
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let shader = link(gl, VERTEX, FRAGMENT)?;

        let framebuffer = gl.create_framebuffer()?;
        let color = gl.create_texture()?;
        let vao = gl.create_vertex_array()?;
        let vertices = gl.create_buffer()?;
        let indices = gl.create_buffer()?;
        let size_uniform = gl.get_uniform_location(shader, "size");
        Ok(Self {
            framebuffer, color, shader, vao, vertices, indices, size_uniform, size: [0, 0],
            readbacks: VecDeque::new(), free_buffers: Vec::new(), cpu_frames: Vec::new(), ndi_pack: None,
            readback_bytes: 0,
            #[cfg(windows)]
            dx_bridge: None,
        })
    }

    pub fn has_pending(&self) -> bool {
        !self.readbacks.is_empty()
    }

    pub fn transfer_bytes(&self) -> u64 {
        self.free_buffers.iter().chain(self.readbacks.iter().map(|frame| &frame.buffer))
            .map(|buffer| buffer.bytes as u64).sum()
    }

    pub fn readback_bytes(&self) -> u64 {
        self.readback_bytes
    }

    #[cfg(windows)]
    pub fn interop_error(&self) -> Option<&str> {
        self.dx_bridge.as_ref()?.as_ref().err().map(String::as_str)
    }

    fn copy_readback(&mut self, source: &[u8]) -> Arc<Vec<u8>> {
        // A slot is reusable once the encoder and every sink release the shared frame.
        if let Some(slot) = self.cpu_frames.iter_mut().find(|slot| {
            slot.len() == source.len() && Arc::strong_count(slot) == 1
        }) {
            if let Some(bytes) = Arc::get_mut(slot) {
                bytes.copy_from_slice(source);
                return Arc::clone(slot);
            }
        }
        let frame = Arc::new(source.to_vec());
        let reserved: usize = self.cpu_frames.iter().map(|bytes| bytes.capacity()).sum();
        if self.cpu_frames.len() < 4 && reserved + frame.capacity() <= 96 * 1024 * 1024 {
            self.cpu_frames.push(Arc::clone(&frame));
        }
        frame
    }

    pub unsafe fn collect(&mut self, gl: &glow::Context) -> Result<(), String> {
        let previous_pack = gl.get_parameter_buffer(glow::PIXEL_PACK_BUFFER_BINDING);
        let mut result = Ok(());
        while let Some(frame) = self.readbacks.front() {
            let status = gl.client_wait_sync(frame.fence, 0, 0);
            if status == glow::TIMEOUT_EXPIRED {
                break;
            }
            let frame = self.readbacks.pop_front().unwrap();
            gl.delete_sync(frame.fence);
            let buffer = frame.buffer.id;
            self.free_buffers.push(frame.buffer);
            if status == glow::WAIT_FAILED {
                for recorder in &frame.destinations.recorders { recorder.note_dropped_frames(1); }
                result = Err("Could not wait for the Program readback".into());
                break;
            }
            let [w, h] = frame.size.map(|n| n as usize);
            let len = w * h * if frame.uyvy { 2 } else { 4 };
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
            let data = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, len as i32, glow::MAP_READ_BIT);
            if data.is_null() {
                for recorder in &frame.destinations.recorders { recorder.note_dropped_frames(1); }
                result = Err("Could not map the Program readback".into());
                break;
            }
            let pixels = self.copy_readback(std::slice::from_raw_parts(data, len));
            gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
            for sink in &frame.destinations.decklinks {
                sink.submit(Arc::clone(&pixels), w, h, frame.destinations.audio.clone());
            }
            let network_frame = NetworkFrame { pixels: Arc::clone(&pixels), width: w, height: h };
            for sink in &frame.destinations.streams {
                sink.submit_audio(network_frame.clone(), frame.destinations.audio.clone(), frame.captured_at);
            }
            for sink in &frame.destinations.ndi {
                sink.submit_pixels(network_frame.clone(), frame.uyvy, frame.destinations.audio.clone());
            }
            for sink in &frame.destinations.omt {
                sink.submit(network_frame.clone(), frame.destinations.audio.clone());
            }
            if let Some((key, mailbox)) = frame.destinations.scope {
                *mailbox.lock().unwrap() = Some(ScopeFrame { key, size: frame.size, pixels: Arc::clone(&pixels) });
            }
            crate::media::Recorder::push_group_audio(&frame.destinations.recorders, pixels, w, h, frame.captured_at, true, frame.destinations.audio);
        }
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, previous_pack);
        result
    }

    pub unsafe fn capture(
        &mut self,
        painter: &egui_glow::Painter,
        size: [u32; 2],
        meshes: &[egui::ClippedPrimitive],
        captured_at: Instant,
        destinations: &Destinations,
    ) -> Result<(), String> {
        let gl = painter.gl();
        let previous_draw = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        let previous_pack = gl.get_parameter_buffer(glow::PIXEL_PACK_BUFFER_BINDING);
        let result = self.render(painter, size, meshes, captured_at, destinations);
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, previous_pack);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_draw);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous_read);
        result
    }

    unsafe fn render(
        &mut self,
        painter: &egui_glow::Painter,
        size: [u32; 2],
        meshes: &[egui::ClippedPrimitive],
        captured_at: Instant,
        destinations: &Destinations,
    ) -> Result<(), String> {
        let gl = painter.gl();
        let [width, height] = size.map(|dimension| dimension as i32);
        if self.readbacks.len() >= 2 * crate::model::OUTPUT_COUNT {
            for recorder in &destinations.recorders { recorder.note_dropped_frames(1); }
            return Ok(());
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        if size != self.size {
            self.cpu_frames.clear();
            gl.bind_texture(glow::TEXTURE_2D, Some(self.color));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, width, height, 0,
                glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.color), 0);
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("Could not allocate the Program framebuffer".into());
            }
            self.size = size;
        }
        gl.viewport(0, 0, width, height);
        gl.disable(glow::SCISSOR_TEST);
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.enable(glow::SCISSOR_TEST);
        gl.use_program(Some(self.shader));
        gl.uniform_2_f32(self.size_uniform.as_ref(), width as f32, height as f32);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_vertex_array(Some(self.vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertices));
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.indices));
        let stride = std::mem::size_of::<egui::epaint::Vertex>() as i32;
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
        gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, true, stride, 16);
        for index in 0..3 {
            gl.enable_vertex_attrib_array(index);
        }

        for egui::ClippedPrimitive { clip_rect, primitive } in meshes {
            let egui::epaint::Primitive::Mesh(mesh) = primitive else { continue };
            let Some(texture) = painter.texture(mesh.texture_id) else { continue };
            let x0 = (clip_rect.min.x.round() as i32).clamp(0, width);
            let y0 = (clip_rect.min.y.round() as i32).clamp(0, height);
            let x1 = (clip_rect.max.x.round() as i32).clamp(x0, width);
            let y1 = (clip_rect.max.y.round() as i32).clamp(y0, height);
            gl.scissor(x0, y0, x1 - x0, y1 - y0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&mesh.vertices), glow::STREAM_DRAW);
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(&mesh.indices), glow::STREAM_DRAW);
            gl.draw_elements(glow::TRIANGLES, mesh.indices.len() as i32, glow::UNSIGNED_INT, 0);
        }

        #[cfg(windows)]
        if width % 2 == 0 && height % 2 == 0 && self.readbacks.is_empty()
            && !destinations.recorders.is_empty() && destinations.recorders.iter().all(|recorder| recorder.accepts_surface())
            && destinations.decklinks.is_empty() && destinations.streams.is_empty()
            && destinations.ndi.is_empty() && destinations.omt.is_empty() && destinations.scope.is_none()
        {
            let bridge = self.dx_bridge.get_or_insert_with(|| crate::dx_interop::Bridge::new(gl));
            if let Ok(interop) = bridge {
                match interop.copy(gl, self.framebuffer, size) {
                    Ok(surface) => {
                        crate::media::Recorder::push_surface_group(&destinations.recorders, surface, size,
                            captured_at, destinations.audio.clone());
                        return Ok(());
                    }
                    Err(error) => {
                        let failed = std::mem::replace(bridge, Err(error));
                        if let Ok(failed) = failed { failed.close(); }
                    }
                }
            }
        }
        let mut buffer = match self.free_buffers.pop() {
            Some(buffer) => buffer,
            None => PackBuffer { id: gl.create_buffer()?, bytes: 0 },
        };
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer.id));
        let uyvy = width % 2 == 0 && destinations.only_ndi() && self.ndi_pack
            .get_or_insert_with(|| NdiPack::new(gl)).as_mut().is_ok_and(|target| target.pack(gl, self.color, size));
        if !uyvy { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer)); }
        let read_width = if uyvy { width / 2 } else { width };
        let len = read_width * height * 4;
        if buffer.bytes != len {
            gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, len, glow::STREAM_READ);
            buffer.bytes = len;
        }
        gl.read_pixels(0, 0, read_width, height, glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
        if gl.get_error() != glow::NO_ERROR {
            gl.delete_buffer(buffer.id);
            return Err("OpenGL could not read the Program framebuffer".into());
        }
        let fence = match gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
            Ok(fence) => fence,
            Err(error) => {
                gl.delete_buffer(buffer.id);
                return Err(error);
            }
        };
        self.readbacks.push_back(Readback { buffer, fence, size, captured_at, destinations: destinations.clone(), uyvy });
        self.readback_bytes = self.readback_bytes.saturating_add(len as u64);
        gl.flush();
        Ok(())
    }

    pub unsafe fn destroy(mut self, gl: &glow::Context) {
        #[cfg(windows)]
        if let Some(Ok(bridge)) = self.dx_bridge.take() { bridge.close(); }
        if self.has_pending() {
            gl.finish();
            let _ = self.collect(gl);
        }
        for frame in self.readbacks {
            gl.delete_sync(frame.fence);
            gl.delete_buffer(frame.buffer.id);
        }
        for buffer in self.free_buffers {
            gl.delete_buffer(buffer.id);
        }
        if let Some(Ok(pack)) = self.ndi_pack {
            gl.delete_framebuffer(pack.framebuffer);
            gl.delete_texture(pack.texture);
            gl.delete_program(pack.shader);
        }
        gl.delete_buffer(self.vertices);
        gl.delete_buffer(self.indices);
        gl.delete_vertex_array(self.vao);
        gl.delete_program(self.shader);
        gl.delete_texture(self.color);
        gl.delete_framebuffer(self.framebuffer);
    }
}

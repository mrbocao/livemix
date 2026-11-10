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
    recorders: Vec<Arc<crate::media::Recorder>>,
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

impl Target {
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let shader = gl.create_program()?;
        let link = (|| {
            compile(gl, shader, glow::VERTEX_SHADER, VERTEX)?;
            compile(gl, shader, glow::FRAGMENT_SHADER, FRAGMENT)?;
            gl.link_program(shader);
            if gl.get_program_link_status(shader) { Ok(()) } else { Err(gl.get_program_info_log(shader)) }
        })();
        if let Err(error) = link {
            gl.delete_program(shader);
            return Err(error);
        }

        let framebuffer = gl.create_framebuffer()?;
        let color = gl.create_texture()?;
        let vao = gl.create_vertex_array()?;
        let vertices = gl.create_buffer()?;
        let indices = gl.create_buffer()?;
        let size_uniform = gl.get_uniform_location(shader, "size");
        Ok(Self {
            framebuffer, color, shader, vao, vertices, indices, size_uniform, size: [0, 0],
            readbacks: VecDeque::new(), free_buffers: Vec::new(),
        })
    }

    pub fn has_pending(&self) -> bool {
        !self.readbacks.is_empty()
    }

    pub fn transfer_bytes(&self) -> u64 {
        self.free_buffers.iter().chain(self.readbacks.iter().map(|frame| &frame.buffer))
            .map(|buffer| buffer.bytes as u64).sum()
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
                for recorder in &frame.recorders { recorder.note_dropped_frames(1); }
                result = Err("Could not wait for the Program readback".into());
                break;
            }
            let [w, h] = frame.size.map(|n| n as usize);
            let len = w * h * 4;
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
            let data = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, len as i32, glow::MAP_READ_BIT);
            if data.is_null() {
                for recorder in &frame.recorders { recorder.note_dropped_frames(1); }
                result = Err("Could not map the Program readback".into());
                break;
            }
            let pixels = std::slice::from_raw_parts(data, len).to_vec();
            gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
            crate::media::Recorder::push_group(&frame.recorders, pixels, w, h, frame.captured_at, true);
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
        recorders: &[Arc<crate::media::Recorder>],
    ) -> Result<(), String> {
        let gl = painter.gl();
        let previous_draw = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        let previous_pack = gl.get_parameter_buffer(glow::PIXEL_PACK_BUFFER_BINDING);
        let result = self.render(painter, size, meshes, captured_at, recorders);
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
        recorders: &[Arc<crate::media::Recorder>],
    ) -> Result<(), String> {
        let gl = painter.gl();
        let [width, height] = size.map(|dimension| dimension as i32);
        if self.readbacks.len() >= 2 * crate::model::OUTPUT_COUNT {
            for recorder in recorders { recorder.note_dropped_frames(1); }
            return Ok(());
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        if size != self.size {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.color));
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

        let mut buffer = match self.free_buffers.pop() {
            Some(buffer) => buffer,
            None => PackBuffer { id: gl.create_buffer()?, bytes: 0 },
        };
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer.id));
        let len = width * height * 4;
        if buffer.bytes != len {
            gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, len, glow::STREAM_READ);
            buffer.bytes = len;
        }
        gl.read_pixels(0, 0, width, height, glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
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
        self.readbacks.push_back(Readback { buffer, fence, size, captured_at, recorders: recorders.to_vec() });
        gl.flush();
        Ok(())
    }

    pub unsafe fn destroy(mut self, gl: &glow::Context) {
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
        gl.delete_buffer(self.vertices);
        gl.delete_buffer(self.indices);
        gl.delete_vertex_array(self.vao);
        gl.delete_program(self.shader);
        gl.delete_texture(self.color);
        gl.delete_framebuffer(self.framebuffer);
    }
}

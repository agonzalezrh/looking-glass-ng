use cgmath::Matrix;
use cgmath::Matrix4;
use smithay::backend::renderer::gles::ffi;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::Texture;
use smithay::backend::SwapBuffersError;
use tracing::error;

use crate::scene::Scene;

const QUAD_VS: &str = "\
attribute vec2 a_pos;
attribute vec2 a_uv;
uniform mat4 u_mvp;
varying vec2 v_uv;
void main() {
    gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
}
";

const QUAD_FS: &str = "\
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex;
void main() {
    vec2 uv = v_uv;
    float b = 0.05;
    bvec4 edge = bvec4(
        uv.x < b || uv.x > 1.0 - b,
        uv.y < b || uv.y > 1.0 - b,
        false,
        false
    );
    if (any(edge)) {
        // BRIGHT CYAN border - makes the quad edges visible
        gl_FragColor = vec4(0.0, 1.0, 1.0, 1.0);
    } else {
        gl_FragColor = texture2D(u_tex, uv);
    }
}
";

struct DrawGl {
    program: u32,
    a_pos: u32,
    a_uv: u32,
    u_mvp: i32,
    u_tex: i32,
    vbo: u32,
}

impl DrawGl {
    fn new(gl: &ffi::Gles2) -> Self {
        let vs = Self::compile(gl, ffi::VERTEX_SHADER, QUAD_VS);
        let fs = Self::compile(gl, ffi::FRAGMENT_SHADER, QUAD_FS);
        let program = unsafe { gl.CreateProgram() };
        unsafe {
            gl.AttachShader(program, vs);
            gl.AttachShader(program, fs);
            gl.LinkProgram(program);
            gl.DeleteShader(vs);
            gl.DeleteShader(fs);
        }
        let a_pos = unsafe { gl.GetAttribLocation(program, b"a_pos\0".as_ptr() as *const i8) as u32 };
        let a_uv = unsafe { gl.GetAttribLocation(program, b"a_uv\0".as_ptr() as *const i8) as u32 };
        let u_mvp = unsafe { gl.GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const i8) };
        let u_tex = unsafe { gl.GetUniformLocation(program, b"u_tex\0".as_ptr() as *const i8) };
        let mut vbo = 0;
        unsafe { gl.GenBuffers(1, &mut vbo) };
        let verts: [f32; 16] = [
            -0.5, -0.5, 0.0, 1.0,
             0.5, -0.5, 1.0, 1.0,
            -0.5,  0.5, 0.0, 0.0,
             0.5,  0.5, 1.0, 0.0,
        ];
        unsafe {
            gl.BindBuffer(ffi::ARRAY_BUFFER, vbo);
            gl.BufferData(ffi::ARRAY_BUFFER, std::mem::size_of_val(&verts) as isize,
                verts.as_ptr() as *const std::ffi::c_void, ffi::STATIC_DRAW);
        }
        DrawGl { program, a_pos, a_uv, u_mvp, u_tex, vbo }
    }

    fn compile(gl: &ffi::Gles2, kind: u32, src: &str) -> u32 {
        let s = unsafe { gl.CreateShader(kind) };
        let bytes = src.as_bytes();
        let len = bytes.len() as i32;
        unsafe {
            gl.ShaderSource(s, 1, &(bytes.as_ptr() as *const i8), &len);
            gl.CompileShader(s);
        }
        let mut ok = 0;
        unsafe { gl.GetShaderiv(s, ffi::COMPILE_STATUS, &mut ok) };
        if ok == 0 {
            let mut len = 0;
            unsafe { gl.GetShaderiv(s, ffi::INFO_LOG_LENGTH, &mut len) };
            let mut buf = vec![0u8; len as usize];
            unsafe { gl.GetShaderInfoLog(s, len, std::ptr::null_mut(), buf.as_mut_ptr() as *mut i8) };
            error!("Shader error: {}", String::from_utf8_lossy(&buf));
        }
        s
    }
}

fn draw_textured_quad(
    gl: &ffi::Gles2,
    draw: &DrawGl,
    mvp: &Matrix4<f32>,
    tex_id: u32,
) {
    unsafe {
        gl.UseProgram(draw.program);
        gl.UniformMatrix4fv(draw.u_mvp, 1, 0, mvp.as_ptr());
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, tex_id);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
        gl.Uniform1i(draw.u_tex, 0);

        let stride = 4 * std::mem::size_of::<f32>() as i32;
        gl.BindBuffer(ffi::ARRAY_BUFFER, draw.vbo);
        gl.EnableVertexAttribArray(draw.a_pos);
        gl.VertexAttribPointer(draw.a_pos, 2, ffi::FLOAT, 0, stride, std::ptr::null());
        gl.EnableVertexAttribArray(draw.a_uv);
        gl.VertexAttribPointer(draw.a_uv, 2, ffi::FLOAT, 0, stride,
            (2 * std::mem::size_of::<f32>()) as *const std::ffi::c_void);
        gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
        gl.DisableVertexAttribArray(draw.a_pos);
        gl.DisableVertexAttribArray(draw.a_uv);
    }
}

fn do_render(
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    scene: &Scene,
    view: &Matrix4<f32>,
    window_size: smithay::utils::Size<i32, smithay::utils::Physical>,
    _w: f32, _h: f32, aspect: f32,
) -> Result<(), SwapBuffersError> {
    let (renderer, mut target) = backend.bind()?;
    let mut frame = match renderer.render(&mut target, window_size, smithay::utils::Transform::Normal) {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };

    let draw = match frame.with_context(|gl| DrawGl::new(gl)) {
        Ok(d) => d,
        Err(e) => {
            error!(?e, "Failed to create GL objects");
            return Ok(());
        }
    };

    let _ = frame.with_context(|gl| unsafe {
        gl.ClearColor(0.15, 0.15, 0.15, 1.0);
        gl.Clear(ffi::COLOR_BUFFER_BIT | ffi::DEPTH_BUFFER_BIT);
    });
    let _ = frame.with_context(|gl| unsafe {
        gl.Enable(ffi::BLEND);
        gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        gl.Enable(ffi::DEPTH_TEST);
        gl.DepthFunc(ffi::LESS);
    });

    let proj = cgmath::perspective(cgmath::Deg(45.0), aspect, 1.0, 10000.0);

    for visual in scene.iter() {
        let Some(texture) = visual.texture() else { continue };
        let tex_id = texture.tex_id();
        let gw = texture.size().w as f32;
        let gh = texture.size().h as f32;

        let pos = visual.transform.position;
        let model = Matrix4::from_translation(pos)
            * Matrix4::from(visual.transform.rotation)
            * Matrix4::from_nonuniform_scale(gw, gh, 1.0);
        let mvp = proj * view * model;
        let _ = frame.with_context(|gl| draw_textured_quad(gl, &draw, &mvp, tex_id));
    }

    drop(frame);
    drop(target);
    backend.submit(None)
}

pub fn render_scene(
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    scene: &Scene,
    view: &Matrix4<f32>,
) -> Result<(), SwapBuffersError> {
    let window_size = backend.window_size();
    let w = window_size.w as f32;
    let h = window_size.h as f32;
    let aspect = w / h;

    let r = do_render(backend, scene, view, window_size, w, h, aspect);
    if let Err(SwapBuffersError::ContextLost(e)) = r {
        error!(?e, "Context lost");
    }
    Ok(())
}

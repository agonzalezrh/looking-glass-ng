use cgmath::Matrix;
use cgmath::Matrix4;
use smithay::backend::renderer::gles::ffi;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::Texture;
use smithay::backend::SwapBuffersError;
use smithay::utils::Transform;
use tracing::error;
use tracing::warn;

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
    gl_FragColor = texture2D(u_tex, v_uv);
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
            error!("Shader compile error: {}", String::from_utf8_lossy(&buf));
        }
        s
    }
}

fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Matrix4<f32> {
    let f = 1.0 / (fov_y / 2.0).tan();
    Matrix4::new(
        f / aspect, 0.0, 0.0, 0.0,
        0.0, f, 0.0, 0.0,
        0.0, 0.0, (far + near) / (near - far), 2.0 * far * near / (near - far),
        0.0, 0.0, -1.0, 0.0,
    )
}

pub fn render_scene(
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    scene: &Scene,
) -> Result<(), SwapBuffersError> {
    let window_size = backend.window_size();
    let w = window_size.w as f32;
    let h = window_size.h as f32;

    let (renderer, mut target) = backend.bind()?;

    // Create the GlesFrame — this binds the target's framebuffer
    // and gives us with_context that preserves the binding
    let mut frame = renderer.render(&mut target, window_size, Transform::Normal)
        .map_err(|_| SwapBuffersError::AlreadySwapped)?;

    // Initialize GL state and create draw objects via frame.with_context
    let draw = frame.with_context(|gl| {
        unsafe {
            gl.Viewport(0, 0, window_size.w, window_size.h);
            gl.ClearColor(0.15, 0.15, 0.15, 1.0);
            gl.Clear(ffi::COLOR_BUFFER_BIT);
            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }
        DrawGl::new(gl)
    });
    let draw = match draw {
        Ok(d) => d,
        Err(e) => {
            error!(?e, "Failed to create GL draw objects");
            return Ok(());
        }
    };

    // Orthographic projection: maps pixel coords directly to NDC
    // Left=-w/2, Right=w/2, Bottom=-h/2, Top=h/2, Near=-1000, Far=1000
    let proj = cgmath::ortho(-w/2.0, w/2.0, -h/2.0, h/2.0, -1000.0, 1000.0);
    let view = cgmath::Matrix4::look_to_rh(
        cgmath::Point3::new(0.0, 0.0, 1.0),
        cgmath::Vector3::new(0.0, 0.0, -1.0),
        cgmath::Vector3::new(0.0, 1.0, 0.0),
    );

    for visual in scene.iter() {
        let Some(texture) = visual.texture() else { continue };
        let tex_size = texture.size();
        let gw = tex_size.w as f32;
        let gh = tex_size.h as f32;
        let tex_id = texture.tex_id();

        // Center position in pixel coordinates relative to screen center
        let cx = visual.geometry.loc.x as f32 + gw / 2.0 - w / 2.0;
        let cy = -(visual.geometry.loc.y as f32 + gh / 2.0) + h / 2.0;

        // Scale by 1 pixel = 1 unit, then rotate + translate
        let model = cgmath::Matrix4::from_translation(cgmath::Vector3::new(cx, cy, 0.0))
            * visual.transform.to_matrix()
            * cgmath::Matrix4::from_nonuniform_scale(gw, gh, 1.0);

        let mvp = proj * view * model;

        let _ = frame.with_context(|gl| unsafe {
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
        });
    }

    // Drop frame, then target, then submit
    drop(frame);
    drop(target);

    if let Err(e) = backend.submit(None) {
        warn!(?e, "Submit failed");
        if matches!(e, SwapBuffersError::ContextLost(_)) {
            return Err(e);
        }
    }
    Ok(())
}

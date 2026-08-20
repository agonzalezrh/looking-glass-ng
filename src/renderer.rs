use smithay::backend::renderer::gles::ffi;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::Renderer;
use smithay::backend::SwapBuffersError;
use tracing::error;
use tracing::warn;

use crate::scene::Scene;

const QUAD_VS: &str = "\
attribute vec2 a_pos;
uniform mat4 u_mvp;
void main() {
    gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0);
}
";

const QUAD_FS: &str = "\
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
";

struct DrawGl {
    program: u32,
    a_pos: u32,
    u_mvp: i32,
    u_color: i32,
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
        let u_mvp = unsafe { gl.GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const i8) };
        let u_color = unsafe { gl.GetUniformLocation(program, b"u_color\0".as_ptr() as *const i8) };
        let mut vbo = 0;
        unsafe { gl.GenBuffers(1, &mut vbo) };
        let verts: [f32; 8] = [
            -0.5, -0.5,
             0.5, -0.5,
            -0.5,  0.5,
             0.5,  0.5,
        ];
        unsafe {
            gl.BindBuffer(ffi::ARRAY_BUFFER, vbo);
            gl.BufferData(ffi::ARRAY_BUFFER, std::mem::size_of_val(&verts) as isize,
                verts.as_ptr() as *const std::ffi::c_void, ffi::STATIC_DRAW);
        }
        DrawGl { program, a_pos, u_mvp, u_color, vbo }
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

fn draw_quad(
    gl: &ffi::Gles2,
    draw: &DrawGl,
    mvp: &[f32; 16],
    r: f32, g: f32, b: f32,
) {
    unsafe {
        gl.UseProgram(draw.program);
        gl.UniformMatrix4fv(draw.u_mvp, 1, 0, mvp.as_ptr());
        gl.Uniform4f(draw.u_color, r, g, b, 1.0);

        gl.BindBuffer(ffi::ARRAY_BUFFER, draw.vbo);
        gl.EnableVertexAttribArray(draw.a_pos);
        gl.VertexAttribPointer(draw.a_pos, 2, ffi::FLOAT, 0, 0, std::ptr::null());
        gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
        gl.DisableVertexAttribArray(draw.a_pos);
    }
}

pub fn render_scene(
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    scene: &Scene,
) -> Result<(), SwapBuffersError> {
    let window_size = backend.window_size();
    let w = window_size.w as f32;
    let h = window_size.h as f32;

    let (renderer, mut target) = backend.bind()?;
    let mut frame = renderer.render(&mut target, window_size, smithay::utils::Transform::Normal)
        .map_err(|_| SwapBuffersError::AlreadySwapped)?;

    let draw = match frame.with_context(|gl| DrawGl::new(gl)) {
        Ok(d) => d,
        Err(e) => {
            error!(?e, "Failed to create GL objects");
            return Ok(());
        }
    };

    // Clear: dark grey background
    let _ = frame.with_context(|gl| unsafe {
        gl.ClearColor(0.15, 0.15, 0.15, 1.0);
        gl.Clear(ffi::COLOR_BUFFER_BIT);
    });

    let _ = frame.with_context(|gl| unsafe {
        gl.Enable(ffi::BLEND);
        gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
    });

    // Build orthographic projection and identity view
    // Maps screen coords to NDC: pixel (0,0) → NDC (-1,1), pixel (w,h) → NDC (1,-1)
    let proj: [f32; 16] = [
        2.0/w, 0.0,   0.0, 0.0,
        0.0,  -2.0/h, 0.0, 0.0,
        0.0,   0.0,  -1.0, 0.0,
       -1.0,   1.0,   0.0, 1.0,
    ];

    // Draw TWO quads side by side to prove rotation works
    // Left: unrotated RED square at x=200, y=360 (center of left half)
    let gw = 200.0_f32;
    let gh = 200.0_f32;

    // LEFT QUAD: NO ROTATION, RED
    {
        let px = 200.0 + gw / 2.0;
        let py = 360.0 + gh / 2.0;
        let mut mvp: [f32; 16] = proj;
        mvp[0]  = 2.0/w * gw;
        mvp[5]  = -2.0/h * gh;
        mvp[12] = 2.0/w * px - 1.0;
        mvp[13] = -2.0/h * py + 1.0;
        let _ = frame.with_context(|gl| draw_quad(gl, &draw, &mvp, 1.0, 0.0, 0.0));
    }

    // RIGHT QUAD: 30° Z ROTATION, BLUE
    {
        let px = 800.0 + gw / 2.0;
        let py = 360.0 + gh / 2.0;
        let angle = 30.0_f32.to_radians();
        let c = angle.cos();
        let s = angle.sin();

        let mvp: [f32; 16] = [
            2.0/w * gw * c,  -2.0/h * gw * s,  0.0, 0.0,
            2.0/w * gh * -s, -2.0/h * gh * c,  0.0, 0.0,
            0.0,             0.0,             -1.0, 0.0,
            2.0/w * px - 1.0, -2.0/h * py + 1.0, 0.0, 1.0,
        ];
        let _ = frame.with_context(|gl| draw_quad(gl, &draw, &mvp, 0.0, 0.0, 1.0));
    }

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

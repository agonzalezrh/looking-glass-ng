use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::gles::UniformName;
use smithay::backend::renderer::gles::UniformType;
use smithay::backend::renderer::gles::UniformValue;
use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::Texture;
use smithay::backend::SwapBuffersError;
use smithay::utils::Point;
use smithay::utils::Rectangle;
use smithay::utils::Size;
use smithay::utils::Transform;
use tracing::warn;

use crate::scene::Scene;

/// Custom texture shader that applies a perspective warp to the texture coordinates.
/// Takes a 3x3 matrix as `u_mat` uniform that transforms UV coords.
/// The matrix is a 2D affine that produces a perspective-like skew.
const PERSPECTIVE_SHADER: &str = "
//_DEFINES_
precision mediump float;
uniform sampler2D tex;
uniform float alpha;
uniform mat3 u_mat;
varying vec2 v_coords;

void main() {
    vec3 uv = vec3(v_coords - 0.5, 1.0);
    vec3 transformed = u_mat * uv;
    vec2 final_uv = transformed.xy / transformed.z + 0.5;
    if (final_uv.x < 0.0 || final_uv.x > 1.0 ||
        final_uv.y < 0.0 || final_uv.y > 1.0) {
        discard;
    }
    gl_FragColor = texture2D(tex, final_uv) * alpha;
}
";

fn skew_matrix(skew_x: f32, skew_y: f32, scale_x: f32, scale_y: f32) -> [f32; 9] {
    // Returns a 3x3 matrix in column-major order that:
    // 1. Applies perspective division via [2] row (skew_x, skew_y, 1)
    // 2. Scales by (scale_x, scale_y)
    [
        scale_x, 0.0,    0.0,
        0.0,     scale_y, 0.0,
        skew_x,  skew_y,  1.0,
    ]
}

pub fn render_scene(
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    scene: &Scene,
) -> Result<(), SwapBuffersError> {
    let window_size = backend.window_size();

    let (renderer, mut target) = backend.bind()?;

    let perspective_program = renderer.compile_custom_texture_shader(
        PERSPECTIVE_SHADER,
        &[UniformName::new("u_mat", UniformType::Matrix3x3)],
    );
    let perspective_program = match perspective_program {
        Ok(p) => Some(p),
        Err(e) => {
            warn!(?e, "Failed to compile perspective shader");
            None
        }
    };

    let mut frame = match renderer.render(&mut target, window_size, Transform::Normal) {
        Ok(f) => f,
        Err(e) => {
            warn!(?e, "Failed to create render frame");
            return Ok(());
        }
    };

    for visual in scene.iter() {
        let texture = match visual.texture() {
            Some(t) => t,
            None => continue,
        };

        let tex_size = texture.size();
        let gw = tex_size.w;
        let gh = tex_size.h;

        let x = visual.geometry.loc.x;
        let y = visual.geometry.loc.y;

        let dest = Rectangle::new(Point::new(x, y), Size::new(gw, gh));
        let src = Rectangle::new(Point::new(0.0, 0.0), Size::new(gw as f64, gh as f64));

        // Check if this visual has a transform that's not identity
        let use_perspective = visual.transform.rotation_angle().abs() > 0.001;

        let (use_program, uniforms) = if use_perspective {
            // Build a perspective warp matrix from the quaternion
            let angle = visual.transform.rotation_angle();
            let skew = angle * 0.8;
            let mat = skew_matrix(skew, 0.0, 1.0, 1.0);
            let uv = UniformValue::Matrix3x3 { matrices: vec![mat], transpose: true };
            (
                perspective_program.as_ref().map(|p| p as &_),
                vec![
                    Uniform::new("u_mat", uv),
                ],
            )
        } else {
            (None, vec![])
        };

        if let Err(e) = frame.render_texture_from_to(
            texture,
            src,
            dest,
            &[dest],
            &[],
            Transform::Normal,
            1.0,
            use_program,
            &uniforms,
        ) {
            warn!(?e, "Failed to render texture");
        }
    }

    drop(frame);
    drop(target);
    backend.submit(None)
}

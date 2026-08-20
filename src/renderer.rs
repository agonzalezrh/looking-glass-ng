use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexProgram;
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::gles::UniformName;
use smithay::backend::renderer::gles::UniformType;
use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::Texture;
use smithay::backend::SwapBuffersError;
use smithay::utils::Point;
use smithay::utils::Rectangle;
use smithay::utils::Size;
use smithay::utils::Transform;
use tracing::warn;

use crate::scene::Scene;

/// Custom texture shader that applies a 2D rotation to texture coordinates.
/// The rotation is around the center of the texture by `u_angle` radians.
const ROTATION_SHADER: &str = "
//_DEFINES_
precision mediump float;
uniform sampler2D tex;
uniform float alpha;
uniform float u_angle;
varying vec2 v_coords;

void main() {
    vec2 uv = v_coords - 0.5;
    float c = cos(u_angle);
    float s = sin(u_angle);
    vec2 rotated = vec2(
        uv.x * c - uv.y * s,
        uv.x * s + uv.y * c
    );
    rotated += 0.5;
    if (rotated.x < 0.0 || rotated.x > 1.0 || rotated.y < 0.0 || rotated.y > 1.0) {
        discard;
    }
    gl_FragColor = texture2D(tex, rotated) * alpha;
}
";

pub fn render_scene(
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    scene: &Scene,
) -> Result<(), SwapBuffersError> {
    let window_size = backend.window_size();

    let (renderer, mut target) = backend.bind()?;

    // Keep renderer borrowed for the operation
    let (renderer, window_size) = (renderer, window_size);

    // Compile the rotation shader once
    let rotation_program = renderer.compile_custom_texture_shader(
        ROTATION_SHADER,
        &[UniformName::new("u_angle", UniformType::_1f)],
    );
    let rotation_program = match rotation_program {
        Ok(p) => Some(p),
        Err(e) => {
            warn!(?e, "Failed to compile rotation shader");
            None
        }
    };

    // Create the render frame
    let mut frame = match renderer.render(&mut target, window_size, Transform::Normal) {
        Ok(f) => f,
        Err(e) => {
            warn!(?e, "Failed to create render frame");
            return Ok(());
        }
    };

    // Draw each visual
    for visual in scene.iter() {
        let texture = match visual.texture() {
            Some(t) => t,
            None => continue,
        };

        let tex_size = texture.size();
        let gw = tex_size.w;
        let gh = tex_size.h;

        // Position at visual.geometry origin
        let x = visual.geometry.loc.x;
        let y = visual.geometry.loc.y;

        let dest = Rectangle::new(Point::new(x, y), Size::new(gw, gh));
        let src = Rectangle::new(Point::new(0.0, 0.0), Size::new(gw as f64, gh as f64));

        // Determine if we need the rotation shader
        let angle = visual.transform.rotation_angle();
        let use_program = if angle.abs() > 0.001 {
            rotation_program.as_ref()
        } else {
            None
        };

        let uniforms = if use_program.is_some() {
            vec![Uniform::new("u_angle", angle)]
        } else {
            vec![]
        };

        if let Err(e) = frame.render_texture_from_to(
            texture,
            src,
            dest,
            &[dest],
            &[],
            Transform::Normal,
            1.0,
            use_program.map(|p| p as &GlesTexProgram),
            &uniforms,
        ) {
            warn!(?e, "Failed to render texture");
        }
    }

    drop(frame);
    drop(target);
    backend.submit(None)
}

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::Texture;
use smithay::backend::SwapBuffersError;
use smithay::utils::Point;
use smithay::utils::Rectangle;
use smithay::utils::Size;
use smithay::utils::Transform;
use tracing::warn;

use crate::scene::Scene;

pub fn render_scene(
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    scene: &Scene,
) -> Result<(), SwapBuffersError> {
    let window_size = backend.window_size();

    let (renderer, mut target) = backend.bind()?;
    let mut frame = renderer.render(&mut target, window_size, Transform::Normal)?;

    for visual in scene.iter() {
        if let Some(texture) = visual.texture() {
            let tex_size = texture.size();
            let dest = Rectangle::new(
                Point::new(visual.geometry.loc.x, visual.geometry.loc.y),
                Size::new(tex_size.w, tex_size.h),
            );
            let src = Rectangle::new(
                Point::new(0.0, 0.0),
                Size::new(tex_size.w as f64, tex_size.h as f64),
            );
            if let Err(e) = frame.render_texture_from_to(
                texture,
                src,
                dest,
                &[dest],
                &[],
                Transform::Normal,
                1.0,
                None,
                &[],
            ) {
                warn!(?e, "Failed to render texture");
            }
        }
    }

    drop(frame);
    drop(target);
    backend.submit(None)
}

use cgmath::InnerSpace;
use cgmath::Matrix4;
use cgmath::SquareMatrix;
use cgmath::Vector3;
use cgmath::Vector4;

use crate::scene::{Scene, VisualId};

/// Classification of a pointer event's destination.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InteractionTarget {
    /// The event is for compositor scene interaction (manipulation/camera).
    Scene,
    /// The event is directed at the content of a specific visual.
    Content(VisualId),
}

/// The kind of pointer event being delivered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEventKind {
    Down,
    Up,
    Motion,
    Scroll(f64, f64),
}

/// Abstraction for delivering input events to application content.
///
/// Implementations convert compositor-level pointer events into
/// application-specific input (Wayland seat events, KVMFR guest input, etc.).
/// The compositor never knows which implementation is behind this trait.
pub trait InputSink: std::fmt::Debug {
    /// Deliver a pointer event in visual-local normalized coordinates.
    /// `u`, `v` are in [0, 1] where (0,0) = top-left, (1,1) = bottom-right.
    fn handle_pointer(&mut self, kind: PointerEventKind, u: f64, v: f64);
}

/// Determines whether a pointer event targets the compositor scene or a visual's content.
///
/// Rule:
/// - If any modifier (shift/ctrl/alt) is held → Scene (manipulation)
/// - Otherwise → Content (application input to the selected visual)
pub fn classify_pointer_target(
    scene: &Scene,
    _proj_view: &Matrix4<f32>,
    shift: bool,
    ctrl: bool,
    alt: bool,
) -> InteractionTarget {
    if shift || ctrl || alt {
        return InteractionTarget::Scene;
    }
    match scene.selected_id {
        Some(vid) => InteractionTarget::Content(vid),
        None => InteractionTarget::Scene,
    }
}

/// Convert screen coordinates to visual-local normalized UV coordinates.
///
/// Returns `(u, v)` where both are in [0, 1], or `None` if the ray doesn't
/// intersect the given visual.
///
/// The pipeline:
///   screen → NDC → world ray → visual-local intersection → [0,1] normalized
pub fn screen_to_visual_uv(
    proj_view: &Matrix4<f32>,
    ndc_x: f32,
    ndc_y: f32,
    visual_transform: &crate::scene::Transform3D,
    geom_w: f32,
    geom_h: f32,
) -> Option<(f64, f64)> {
    let inv_pv = proj_view.invert().unwrap_or(Matrix4::identity());

    let near = inv_pv * Vector4::new(ndc_x, ndc_y, -1.0, 1.0);
    let far = inv_pv * Vector4::new(ndc_x, ndc_y, 1.0, 1.0);
    let near = Vector3::new(near.x / near.w, near.y / near.w, near.z / near.w);
    let far = Vector3::new(far.x / far.w, far.y / far.w, far.z / far.w);
    let dir = (far - near).normalize();

    let model = Matrix4::from_translation(visual_transform.position)
        * Matrix4::from(visual_transform.rotation)
        * Matrix4::from_nonuniform_scale(geom_w, geom_h, 1.0);

    let inv_model = model.invert().unwrap_or(Matrix4::identity());
    let local_origin = inv_model * Vector4::new(near.x, near.y, near.z, 1.0);
    let local_dir = inv_model * Vector4::new(dir.x, dir.y, dir.z, 0.0);
    let lo = Vector3::new(local_origin.x, local_origin.y, local_origin.z) / local_origin.w;
    let ld = Vector3::new(local_dir.x, local_dir.y, local_dir.z);

    if ld.z.abs() < 1e-8 {
        return None;
    }
    let t = -lo.z / ld.z;
    if t < 0.0 {
        return None;
    }
    let hit = lo + ld * t;
    if hit.x.abs() > 0.5 || hit.y.abs() > 0.5 {
        return None;
    }

    // Convert from [-0.5, 0.5] quad coords to [0, 1] UV
    let u = (hit.x + 0.5) as f64;
    let v = (1.0 - (hit.y + 0.5)) as f64;
    Some((u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)))
}

/// Convert normalized UV to pixel coordinates within the visual.
pub fn uv_to_pixels(u: f64, v: f64, width: u32, height: u32) -> (u32, u32) {
    let px = (u * width as f64) as u32;
    let py = (v * height as f64) as u32;
    (px.min(width.saturating_sub(1)), py.min(height.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Transform3D;
    use cgmath::Deg;
    use cgmath::Quaternion;
    use cgmath::Rotation3;
    use cgmath::Vector3;

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn classify_modifier_returns_scene() {
        let scene = Scene::default();
        let pv = Matrix4::identity();
        assert_eq!(
            classify_pointer_target(&scene, &pv, true, false, false),
            InteractionTarget::Scene
        );
        assert_eq!(
            classify_pointer_target(&scene, &pv, false, true, false),
            InteractionTarget::Scene
        );
        assert_eq!(
            classify_pointer_target(&scene, &pv, false, false, true),
            InteractionTarget::Scene
        );
    }

    #[test]
    fn classify_no_selection_returns_scene() {
        let scene = Scene::default();
        let pv = Matrix4::identity();
        assert_eq!(
            classify_pointer_target(&scene, &pv, false, false, false),
            InteractionTarget::Scene
        );
    }

    #[test]
    fn screen_to_uv_center() {
        let transform = Transform3D::identity();
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let pv = proj * view;
        let uv = screen_to_visual_uv(&pv, 0.0, 0.0, &transform, 200.0, 100.0);
        assert!(uv.is_some(), "should hit center of 200x100 quad");
        let (u, v) = uv.unwrap();
        assert!(approx_eq(u, 0.5, 1e-4), "u should be ~0.5, got {}", u);
        assert!(approx_eq(v, 0.5, 1e-4), "v should be ~0.5, got {}", v);
    }

    #[test]
    fn screen_to_uv_corner() {
        let transform = Transform3D::identity();
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let pv = proj * view;
        // Aim at top-left of the quad: quad is [-100,100]x[-50,50] in world
        // At z=0, NDC (-0.3125, 0.4167) ≈ world (-100, 50)
        let ndc_x = -100.0 / 320.0; // ≈ -0.3125
        let ndc_y = 50.0 / 240.0;   // ≈ 0.2083
        let uv = screen_to_visual_uv(&pv, ndc_x, ndc_y, &transform, 200.0, 100.0);
        assert!(uv.is_some(), "should hit near top-left of 200x100 quad");
        let (u, v) = uv.unwrap();
        assert!(u < 0.1, "u should be near 0 (top-left), got {}", u);
        assert!(v < 0.1, "v should be near 0 (top-left), got {}", v);
    }

    #[test]
    fn uv_to_pixel_center() {
        let (px, py) = uv_to_pixels(0.5, 0.5, 1920, 1080);
        assert_eq!(px, 960);
        assert_eq!(py, 540);
    }

    #[test]
    fn uv_to_pixel_corner() {
        let (px, py) = uv_to_pixels(0.0, 0.0, 1920, 1080);
        assert_eq!(px, 0);
        assert_eq!(py, 0);
    }

    #[test]
    fn uv_to_pixel_edge_clamp() {
        let (px, py) = uv_to_pixels(1.0, 1.0, 1920, 1080);
        assert_eq!(px, 1919);
        assert_eq!(py, 1079);
    }

    #[test]
    fn screen_to_uv_rotated_visual() {
        let mut transform = Transform3D::identity();
        transform.rotation = Quaternion::from_angle_y(Deg(45.0));
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let pv = proj * view;
        // After 45° Y rotation, the center should still give (0.5, 0.5)
        let uv = screen_to_visual_uv(&pv, 0.0, 0.0, &transform, 200.0, 100.0);
        assert!(uv.is_some(), "should hit rotated visual at center");
        let (u, v) = uv.unwrap();
        assert!(approx_eq(u, 0.5, 1e-4), "u should be ~0.5, got {}", u);
        assert!(approx_eq(v, 0.5, 1e-4), "v should be ~0.5, got {}", v);
    }

    #[test]
    fn screen_to_uv_scaled_visual() {
        let mut transform = Transform3D::identity();
        transform.scale = Vector3::new(2.0, 2.0, 1.0);
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let pv = proj * view;
        // Scaled 2x, the 200x100 quad now spans 400x200 world units
        // Center should still be (0.5, 0.5)
        let uv = screen_to_visual_uv(&pv, 0.0, 0.0, &transform, 200.0, 100.0);
        assert!(uv.is_some(), "should hit scaled visual at center");
        let (u, v) = uv.unwrap();
        assert!(approx_eq(u, 0.5, 1e-4), "u should be ~0.5, got {}", u);
        assert!(approx_eq(v, 0.5, 1e-4), "v should be ~0.5, got {}", v);
    }

    #[test]
    fn screen_to_uv_miss_outside() {
        let transform = Transform3D::identity();
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let pv = proj * view;
        // Far corner in NDC → should miss
        let uv = screen_to_visual_uv(&pv, -0.9, 0.9, &transform, 200.0, 100.0);
        assert!(uv.is_none(), "should miss when pointing at far corner");
    }
}

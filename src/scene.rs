use cgmath::Deg;
use cgmath::InnerSpace;
use cgmath::Matrix4;
use cgmath::Quaternion;
use cgmath::Rotation3;
use cgmath::SquareMatrix;
use cgmath::Vector3;
use cgmath::Vector4;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::utils::Rectangle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VisualId(u64);

impl VisualId {
    fn next() -> Self {
        use std::sync::atomic::AtomicU64;
        use std::sync::atomic::Ordering;
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        VisualId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone)]
pub struct Transform3D {
    pub position: Vector3<f32>,
    pub rotation: Quaternion<f32>,
    pub scale: Vector3<f32>,
}

impl Transform3D {
    pub fn identity() -> Self {
        Transform3D {
            position: Vector3::new(0.0, 0.0, 0.0),
            rotation: Quaternion::from_angle_z(Deg(0.0)),
            scale: Vector3::new(1.0, 1.0, 1.0),
        }
    }

    pub fn rotation_angle(&self) -> f32 {
        use cgmath::InnerSpace;
        let s = self.rotation.s;
        let len = self.rotation.v.magnitude();
        if len < 1e-6 {
            return 0.0;
        }
        2.0 * s.acos()
    }

    pub fn to_matrix(&self) -> Matrix4<f32> {
        let t = Matrix4::from_translation(self.position);
        let r = Matrix4::from(self.rotation);
        let s = Matrix4::from_nonuniform_scale(self.scale.x, self.scale.y, self.scale.z);
        t * r * s
    }
}

#[derive(Debug, Clone)]
pub enum VisualContent {
    WaylandSurface(GlesTexture),
    ExternalTexture(GlesTexture),
}

#[derive(Debug, Clone)]
pub struct Visual {
    pub id: VisualId,
    pub content: VisualContent,
    pub geometry: Rectangle<i32, smithay::utils::Logical>,
    pub transform: Transform3D,
    pub selected: bool,
}

impl Visual {
    pub fn new(content: VisualContent, geometry: Rectangle<i32, smithay::utils::Logical>) -> Self {
        Visual {
            id: VisualId::next(),
            content,
            geometry,
            transform: Transform3D::identity(),
            selected: false,
        }
    }

    pub fn texture(&self) -> Option<&GlesTexture> {
        match &self.content {
            VisualContent::WaylandSurface(t) | VisualContent::ExternalTexture(t) => Some(t),
        }
    }

    pub fn texture_mut(&mut self) -> Option<&mut GlesTexture> {
        match &mut self.content {
            VisualContent::WaylandSurface(t) | VisualContent::ExternalTexture(t) => Some(t),
        }
    }
}

#[derive(Debug, Default)]
pub struct Scene {
    pub visuals: Vec<Visual>,
    pub selected_id: Option<VisualId>,
}

impl Scene {
    pub fn add(&mut self, visual: Visual) {
        self.visuals.push(visual);
    }

    pub fn remove(&mut self, id: VisualId) {
        self.visuals.retain(|v| v.id != id);
        if self.selected_id == Some(id) {
            self.selected_id = None;
        }
    }

    pub fn get_mut(&mut self, id: VisualId) -> Option<&mut Visual> {
        self.visuals.iter_mut().find(|v| v.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Visual> {
        self.visuals.iter()
    }

    /// Set the selected visual. Deselects the previous one.
    pub fn select(&mut self, id: Option<VisualId>) {
        if self.selected_id == id {
            return;
        }
        if let Some(old) = self.selected_id {
            if let Some(v) = self.get_mut(old) {
                v.selected = false;
            }
        }
        self.selected_id = id;
        if let Some(new) = id {
            if let Some(v) = self.get_mut(new) {
                v.selected = true;
            }
        }
    }

    /// Pick the closest visual under a screen coordinate.
    /// `proj_view` = projection × view matrix from the camera.
    /// `ndc_x` / `ndc_y` = normalized device coordinates [-1..1].
    pub fn pick(
        &self,
        proj_view: &Matrix4<f32>,
        ndc_x: f32,
        ndc_y: f32,
    ) -> Option<(VisualId, f32)> {
        pick_visual(proj_view, ndc_x, ndc_y, &self.visuals)
    }
}

/// Pure function: test which visual is hit by a ray from screen NDC.
///
/// `proj_view` = projection × view matrix.
/// `ndc_x`, `ndc_y` = normalized device coordinates in [-1, 1].
/// `visuals` = slice of visual references.
///
/// Returns the closest intersected `(VisualId, hit_distance)` or `None`.
pub fn pick_visual(
    proj_view: &Matrix4<f32>,
    ndc_x: f32,
    ndc_y: f32,
    visuals: &[Visual],
) -> Option<(VisualId, f32)> {
    let items: Vec<_> = visuals
        .iter()
        .map(|v| {
            (
                v.id,
                v.transform.clone(),
                (v.geometry.size.w as f32, v.geometry.size.h as f32),
            )
        })
        .collect();
    pick_visual_items(proj_view, ndc_x, ndc_y, &items)
}

/// Pure picking math operating on (id, transform, width, height) tuples.
/// Used by pick_visual internally and by unit tests.
fn pick_visual_items(
    proj_view: &Matrix4<f32>,
    ndc_x: f32,
    ndc_y: f32,
    items: &[(VisualId, Transform3D, (f32, f32))],
) -> Option<(VisualId, f32)> {
    let inv_pv = proj_view.invert().unwrap_or(Matrix4::identity());

    let near = inv_pv * Vector4::new(ndc_x, ndc_y, -1.0, 1.0);
    let far = inv_pv * Vector4::new(ndc_x, ndc_y, 1.0, 1.0);
    let near = Vector3::new(near.x / near.w, near.y / near.w, near.z / near.w);
    let far = Vector3::new(far.x / far.w, far.y / far.w, far.z / far.w);
    let dir = (far - near).normalize();

    let mut closest: Option<(VisualId, f32)> = None;
    for (id, transform, (gw, gh)) in items {
        let model = Matrix4::from_translation(transform.position)
            * Matrix4::from(transform.rotation)
            * Matrix4::from_nonuniform_scale(*gw, *gh, 1.0);

        let inv_model = model.invert().unwrap_or(Matrix4::identity());
        let local_origin = inv_model * Vector4::new(near.x, near.y, near.z, 1.0);
        let local_dir = inv_model * Vector4::new(dir.x, dir.y, dir.z, 0.0);
        let lo = Vector3::new(local_origin.x, local_origin.y, local_origin.z) / local_origin.w;
        let ld = Vector3::new(local_dir.x, local_dir.y, local_dir.z);

        if ld.z.abs() < 1e-8 {
            continue;
        }
        let t = -lo.z / ld.z;
        if t < 0.0 {
            continue;
        }
        let hit_pt = lo + ld * t;
        if hit_pt.x.abs() > 0.5 || hit_pt.y.abs() > 0.5 {
            continue;
        }

        let local_hit = Vector4::new(hit_pt.x, hit_pt.y, 0.0, 1.0);
        let world_hit_4 = model * local_hit;
        let world_hit = Vector3::new(world_hit_4.x, world_hit_4.y, world_hit_4.z) / world_hit_4.w;
        let dist = (world_hit - near).magnitude();

        match closest {
            Some((_, closest_dist)) if dist >= closest_dist => {}
            _ => closest = Some((*id, dist)),
        }
    }
    closest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_center_hit() {
        let items = vec![(
            VisualId(1),
            Transform3D {
                position: Vector3::new(0.0, 0.0, 0.0),
                rotation: Quaternion::from_angle_z(Deg(0.0)),
                scale: Vector3::new(1.0, 1.0, 1.0),
            },
            (200.0, 100.0),
        )];
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let r = pick_visual_items(&(proj * view), 0.0, 0.0, &items);
        assert!(r.is_some(), "should hit center");
        assert_eq!(r.unwrap().0, VisualId(1));
    }

    #[test]
    fn pick_miss() {
        let items = vec![(
            VisualId(1),
            Transform3D {
                position: Vector3::new(0.0, 0.0, 0.0),
                rotation: Quaternion::from_angle_z(Deg(0.0)),
                scale: Vector3::new(1.0, 1.0, 1.0),
            },
            (200.0, 100.0),
        )];
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let r = pick_visual_items(&(proj * view), -0.9, 0.9, &items);
        assert!(r.is_none(), "should miss when pointing at corner");
    }

    #[test]
    fn pick_depth_wins() {
        let items = vec![
            (
                VisualId(1),
                Transform3D {
                    position: Vector3::new(0.0, 0.0, 0.0),
                    rotation: Quaternion::from_angle_z(Deg(0.0)),
                    scale: Vector3::new(1.0, 1.0, 1.0),
                },
                (200.0, 100.0),
            ),
            (
                VisualId(2),
                Transform3D {
                    position: Vector3::new(0.0, 0.0, -200.0),
                    rotation: Quaternion::from_angle_z(Deg(0.0)),
                    scale: Vector3::new(1.0, 1.0, 1.0),
                },
                (200.0, 100.0),
            ),
        ];
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let r = pick_visual_items(&(proj * view), 0.0, 0.0, &items);
        assert!(r.is_some(), "should hit something");
        assert_eq!(r.unwrap().0, VisualId(1), "should pick closer visual (z=0 vs z=-200)");
    }

    #[test]
    fn pick_rotated() {
        let items = vec![(
            VisualId(1),
            Transform3D {
                position: Vector3::new(0.0, 0.0, 0.0),
                rotation: Quaternion::from_angle_y(Deg(45.0)),
                scale: Vector3::new(1.0, 1.0, 1.0),
            },
            (200.0, 100.0),
        )];
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(0.0, 0.0, 500.0),
            cgmath::Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        let r = pick_visual_items(&(proj * view), 0.0, 0.0, &items);
        assert!(r.is_some(), "should hit rotated visual at center");
    }

    #[test]
    fn pick_still_works_after_camera_move() {
        let items = vec![(
            VisualId(1),
            Transform3D {
                position: Vector3::new(100.0, 50.0, 0.0),
                rotation: Quaternion::from_angle_z(Deg(0.0)),
                scale: Vector3::new(1.0, 1.0, 1.0),
            },
            (200.0, 100.0),
        )];
        // Camera shifted right and up
        let view = Matrix4::look_at_rh(
            cgmath::Point3::new(50.0, 25.0, 500.0),
            cgmath::Point3::new(50.0, 25.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        let proj = cgmath::ortho(-320.0, 320.0, -240.0, 240.0, 1.0, 1000.0);
        // The visual is at (100, 50), camera is at (50, 25), so visual is at
        // (100-50, 50-25) = (50, 25) in camera-relative. NDC center should hit.
        let r = pick_visual_items(&(proj * view), 0.0, 0.0, &items);
        assert!(r.is_some(), "should hit after camera move");
    }
}


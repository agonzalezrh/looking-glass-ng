use cgmath::Deg;
use cgmath::Matrix4;
use cgmath::Quaternion;
use cgmath::Rotation3;
use cgmath::Vector3;
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
    /// Texture originating from a Wayland client wl_buffer
    WaylandSurface(GlesTexture),
    /// Texture from an external producer (Looking Glass, synthetic, etc.)
    ExternalTexture(GlesTexture),
}

#[derive(Debug, Clone)]
pub struct Visual {
    pub id: VisualId,
    pub content: VisualContent,
    pub geometry: Rectangle<i32, smithay::utils::Logical>,
    pub transform: Transform3D,
}

impl Visual {
    pub fn new(content: VisualContent, geometry: Rectangle<i32, smithay::utils::Logical>) -> Self {
        Visual {
            id: VisualId::next(),
            content,
            geometry,
            transform: Transform3D::identity(),
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
}

impl Scene {
    pub fn add(&mut self, visual: Visual) {
        self.visuals.push(visual);
    }

    pub fn remove(&mut self, id: VisualId) {
        self.visuals.retain(|v| v.id != id);
    }

    pub fn get_mut(&mut self, id: VisualId) -> Option<&mut Visual> {
        self.visuals.iter_mut().find(|v| v.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Visual> {
        self.visuals.iter()
    }
}

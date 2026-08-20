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
pub enum VisualContent {
    SurfaceTexture(GlesTexture),
}

#[derive(Debug, Clone)]
pub struct Visual {
    pub id: VisualId,
    pub content: VisualContent,
    pub geometry: Rectangle<i32, smithay::utils::Logical>,
}

impl Visual {
    pub fn new(content: VisualContent, geometry: Rectangle<i32, smithay::utils::Logical>) -> Self {
        Visual {
            id: VisualId::next(),
            content,
            geometry,
        }
    }

    pub fn texture(&self) -> Option<&GlesTexture> {
        match &self.content {
            VisualContent::SurfaceTexture(t) => Some(t),
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

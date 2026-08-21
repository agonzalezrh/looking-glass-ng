use cgmath::Point3;

use crate::input::Camera;
use crate::layout::LayoutMode;

/// A workspace saves the camera view and layout mode for a specific
/// presentation of the global Scene. Workspaces share the same Visuals
/// but can arrange and view them differently.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub camera: Camera,
    pub layout_mode: LayoutMode,
}

impl Workspace {
    pub fn new() -> Self {
        Workspace {
            camera: Camera::new(),
            layout_mode: LayoutMode::Freeform,
        }
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Scene;

    #[test]
    fn new_workspace_defaults() {
        let ws = Workspace::new();
        assert_eq!(ws.layout_mode, LayoutMode::Freeform);
        assert_eq!(ws.camera.position, Point3::new(0.0, 0.0, 800.0));
    }

    #[test]
    fn workspace_camera_independence() {
        let mut ws1 = Workspace::new();
        let mut ws2 = Workspace::new();
        ws1.camera.position = Point3::new(100.0, 200.0, 300.0);
        ws2.camera.position = Point3::new(400.0, 500.0, 600.0);
        ws1.layout_mode = LayoutMode::Flat;
        ws2.layout_mode = LayoutMode::Grid { columns: 3 };
        assert_ne!(ws1.camera.position, ws2.camera.position);
        assert_ne!(ws1.layout_mode, ws2.layout_mode);
    }

    #[test]
    fn workspace_switch_preserves_state() {
        let mut ws1 = Workspace::new();
        ws1.camera.position = Point3::new(100.0, 200.0, 300.0);
        ws1.layout_mode = LayoutMode::Flat;

        let mut ws2 = Workspace::new();
        ws2.camera.position = Point3::new(400.0, 500.0, 600.0);
        ws2.layout_mode = LayoutMode::Grid { columns: 2 };

        // Simulate switching: save current to ws1, restore ws2
        let (cam1, lay1) = (ws1.camera.clone(), ws1.layout_mode);
        let (cam2, lay2) = (ws2.camera.clone(), ws2.layout_mode);
        assert_eq!(cam1.position, Point3::new(100.0, 200.0, 300.0));
        assert_eq!(cam2.position, Point3::new(400.0, 500.0, 600.0));
        assert_eq!(lay1, LayoutMode::Flat);
        assert_eq!(lay2, LayoutMode::Grid { columns: 2 });
    }
}

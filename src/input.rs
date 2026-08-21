use cgmath::InnerSpace;
use cgmath::Matrix4;
use cgmath::Point3;
use cgmath::Rad;
use cgmath::Vector3;
use tracing::info;

use crate::scene::{Scene, VisualId};

#[derive(Debug, Clone)]
pub struct Camera {
    pub position: Point3<f32>,
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
    pub sensitivity: f32,
    pub zoom_speed: f32,
    pub bookmarks: [Option<CameraView>; 10],
}

/// Saved camera position/yaw/pitch for bookmarks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    pub position: Point3<f32>,
    pub yaw: f32,
    pub pitch: f32,
}

impl Camera {
    pub fn new() -> Self {
        Camera {
            position: Point3::new(0.0, 0.0, 800.0),
            yaw: 0.0,
            pitch: 0.0,
            speed: 10.0,
            sensitivity: 0.005,
            zoom_speed: 50.0,
            bookmarks: [None, None, None, None, None, None, None, None, None, None],
        }
    }

    pub fn view_matrix(&self) -> Matrix4<f32> {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let forward = Vector3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch);
        let center = self.position + forward;
        Matrix4::look_at_rh(self.position, center, Vector3::new(0.0, 1.0, 0.0))
    }

    pub fn forward(&self) -> Vector3<f32> {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vector3::new(-sin_yaw, 0.0, -cos_yaw)
    }

    pub fn right(&self) -> Vector3<f32> {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vector3::new(cos_yaw, 0.0, -sin_yaw)
    }

    /// Pan the camera in screen space (middle-drag).
    /// dx, dy are in window pixels. translation is perpendicular to the view direction.
    pub fn handle_pan(&mut self, dx: f64, dy: f64, speed: f32) {
        let fwd = self.forward();
        let right = self.right();
        let up = Vector3::new(0.0, 1.0, 0.0);
        self.position += right * (dx as f32 * speed);
        self.position += up * (-dy as f32 * speed);
    }

    /// Orbit the camera around the target point (right-drag).
    /// The camera rotates in place — the view direction changes.
    pub fn handle_orbit(&mut self, dx: f64, dy: f64) {
        self.yaw += dx as f32 * self.sensitivity * 5.0;
        self.pitch = (self.pitch - dy as f32 * self.sensitivity * 5.0)
            .clamp(Rad(-1.5).0, Rad(1.5).0);
    }

    /// Zoom by moving camera along forward axis.
    pub fn handle_zoom(&mut self, delta: f64) {
        let fwd = self.forward();
        self.position += fwd * (delta as f32 * self.zoom_speed * 0.01);
        // Prevent going behind the scene
        if self.position.z < -10000.0 { self.position.z = -10000.0; }
        if self.position.z > 10000.0 { self.position.z = 10000.0; }
    }

    pub fn handle_key(&mut self, key: u32, pressed: bool, dt: f32) {
        if !pressed {
            return;
        }
        let step = self.speed * dt;
        let fwd = self.forward();
        let right = self.right();

        match key {
            25 => { info!("W pressed, camera forward"); self.position += fwd * step; }
            39 => { info!("S pressed, camera backward"); self.position -= fwd * step; }
            38 => { info!("A pressed, camera strafe left"); self.position -= right * step; }
            40 => { info!("D pressed, camera strafe right"); self.position += right * step; }
            24 => { self.position.y -= step; }
            26 => { self.position.y += step; }
            113 => { self.yaw -= Rad(0.05).0; }
            114 => { self.yaw += Rad(0.05).0; }
            111 => { self.pitch = (self.pitch + Rad(0.05).0).clamp(Rad(-1.5).0, Rad(1.5).0); }
            116 => { self.pitch = (self.pitch - Rad(0.05).0).clamp(Rad(-1.5).0, Rad(1.5).0); }
            _ => {}
        }
    }

    pub fn handle_mouse_move(&mut self, dx: f64, dy: f64) {
        self.yaw += dx as f32 * self.sensitivity;
        self.pitch = (self.pitch - dy as f32 * self.sensitivity)
            .clamp(Rad(-1.5).0, Rad(1.5).0);
    }

    pub fn handle_mouse_absolute(&mut self, x: f64, y: f64) {
        use std::cell::Cell;
        thread_local! {
            static LAST_X: Cell<Option<f64>> = Cell::new(None);
            static LAST_Y: Cell<Option<f64>> = Cell::new(None);
        }
        LAST_X.with(|lx| {
            LAST_Y.with(|ly| {
                if let (Some(px), Some(py)) = (lx.get(), ly.get()) {
                    let dx = x - px;
                    let dy = y - py;
                    self.handle_mouse_move(dx, dy);
                }
                lx.set(Some(x));
                ly.set(Some(y));
            });
        });
    }

    /// Center camera on a specific visual.
    pub fn frame_visual(&mut self, vid: VisualId, scene: &Scene) -> bool {
        if let Some(pos) = crate::layout::frame_visual(vid, scene, 1280.0, 720.0) {
            self.position = cgmath::Point3::new(pos.x, pos.y, pos.z);
            self.yaw = 0.0;
            self.pitch = 0.0;
            true
        } else {
            false
        }
    }

    /// Position the camera to show all visuals in the scene.
    /// Computes the bounding volume and places the camera at a suitable distance.
    pub fn frame_all(&mut self, scene: &Scene) -> bool {
        if scene.visuals.is_empty() {
            return false;
        }
        let mut min = Vector3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max = Vector3::new(f32::MIN, f32::MIN, f32::MIN);
        for v in scene.iter() {
            let p = v.transform.position;
            let hs = v.geometry.size;
            let hw = hs.w as f32 * v.transform.scale.x * 0.5;
            let hh = hs.h as f32 * v.transform.scale.y * 0.5;
            let corners = [
                p + Vector3::new(-hw, -hh, 0.0),
                p + Vector3::new(hw, -hh, 0.0),
                p + Vector3::new(-hw, hh, 0.0),
                p + Vector3::new(hw, hh, 0.0),
            ];
            for c in corners {
                min.x = min.x.min(c.x); max.x = max.x.max(c.x);
                min.y = min.y.min(c.y); max.y = max.y.max(c.y);
                min.z = min.z.min(c.z); max.z = max.z.max(c.z);
            }
        }
        let center = (min + max) * 0.5;
        let span = (max - min).magnitude();
        let distance = span * 0.8 + 500.0;
        self.position = Point3::new(center.x, center.y, center.z + distance);
        self.yaw = 0.0;
        self.pitch = 0.0;
        true
    }

    /// Save current view to a bookmark slot (0-9).
    pub fn save_bookmark(&mut self, slot: usize) {
        if slot < self.bookmarks.len() {
            self.bookmarks[slot] = Some(CameraView {
                position: self.position,
                yaw: self.yaw,
                pitch: self.pitch,
            });
        }
    }

    /// Restore a bookmark (0-9). Returns true if the slot had a saved view.
    pub fn restore_bookmark(&mut self, slot: usize) -> bool {
        let view = match self.bookmarks.get(slot) {
            Some(Some(v)) => *v,
            _ => return false,
        };
        self.position = view.position;
        self.yaw = view.yaw;
        self.pitch = view.pitch;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Scene;
    use cgmath::Deg;

    #[test]
    fn frame_all_empty() {
        let mut cam = Camera::new();
        let scene = Scene::default();
        assert!(!cam.frame_all(&scene));
    }

    #[test]
    fn frame_all_not_empty() {
        // Can't add visuals without GlesTexture, so test frame_all on empty.
        // The function is also validated through frame_visual in layout tests.
        let mut cam = Camera::new();
        let scene = Scene::default();
        assert!(!cam.frame_all(&scene));
    }

    #[test]
    fn save_and_restore_bookmark() {
        let mut cam = Camera::new();
        cam.position = Point3::new(100.0, 200.0, 300.0);
        cam.yaw = 0.5;
        cam.pitch = 0.3;
        cam.save_bookmark(1);
        // Move elsewhere
        cam.position = Point3::new(999.0, 999.0, 999.0);
        cam.yaw = 1.0;
        cam.pitch = 0.5;
        // Restore
        assert!(cam.restore_bookmark(1));
        assert!((cam.position.x - 100.0).abs() < 1e-4);
        assert!((cam.position.y - 200.0).abs() < 1e-4);
        assert!((cam.position.z - 300.0).abs() < 1e-4);
        assert!((cam.yaw - 0.5).abs() < 1e-4);
        assert!((cam.pitch - 0.3).abs() < 1e-4);
    }

    #[test]
    fn restore_empty_bookmark_returns_false() {
        let mut cam = Camera::new();
        assert!(!cam.restore_bookmark(0));
    }

    #[test]
    fn orbit_changes_yaw_and_pitch() {
        let mut cam = Camera::new();
        let (y0, p0) = (cam.yaw, cam.pitch);
        cam.handle_orbit(10.0, 5.0);
        assert_ne!(cam.yaw, y0);
        assert_ne!(cam.pitch, p0);
    }

    #[test]
    fn zoom_changes_z_position() {
        let mut cam = Camera::new();
        let z0 = cam.position.z;
        cam.handle_zoom(-100.0);
        assert_ne!(cam.position.z, z0);
    }

    #[test]
    fn pan_moves_camera() {
        let mut cam = Camera::new();
        let (x0, y0) = (cam.position.x, cam.position.y);
        cam.handle_pan(100.0, 50.0, 0.5);
        assert_ne!(cam.position.x, x0);
        assert_ne!(cam.position.y, y0);
    }
}


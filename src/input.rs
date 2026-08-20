use cgmath::Matrix4;
use cgmath::Point3;
use cgmath::Rad;
use cgmath::Vector3;
use tracing::info;

#[derive(Debug, Clone)]
pub struct Camera {
    pub position: Point3<f32>,
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
    pub sensitivity: f32,
}

impl Camera {
    pub fn new() -> Self {
        Camera {
            position: Point3::new(0.0, 0.0, 800.0),
            yaw: 0.0,
            pitch: 0.0,
            speed: 10.0,
            sensitivity: 0.005,
        }
    }

    pub fn view_matrix(&self) -> Matrix4<f32> {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();

        let forward = Vector3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch);
        let center = self.position + forward;
        Matrix4::look_at_rh(self.position, center, Vector3::new(0.0, 1.0, 0.0))
    }

    pub fn handle_key(&mut self, key: u32, pressed: bool, dt: f32) {
        if !pressed {
            return;
        }
        let step = self.speed * dt;
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let forward = Vector3::new(-sin_yaw, 0.0, -cos_yaw);
        let right = Vector3::new(cos_yaw, 0.0, -sin_yaw);

        match key {
            // W - forward
            25 => {
                info!("W pressed, camera forward");
                self.position += forward * step;
            }
            // S - backward
            39 => {
                info!("S pressed, camera backward");
                self.position -= forward * step;
            }
            // A - strafe left
            38 => {
                info!("A pressed, camera strafe left");
                self.position -= right * step;
            }
            // D - strafe right
            40 => {
                info!("D pressed, camera strafe right");
                self.position += right * step;
            }
            // Q - down
            24 => {
                self.position.y -= step;
            }
            // E - up
            26 => {
                self.position.y += step;
            }
            // Left arrow - yaw left
            113 => {
                self.yaw -= Rad(0.05).0;
            }
            // Right arrow - yaw right
            114 => {
                self.yaw += Rad(0.05).0;
            }
            // Up arrow - pitch up
            111 => {
                self.pitch = (self.pitch + Rad(0.05).0).clamp(Rad(-1.5).0, Rad(1.5).0);
            }
            // Down arrow - pitch down
            116 => {
                self.pitch = (self.pitch - Rad(0.05).0).clamp(Rad(-1.5).0, Rad(1.5).0);
            }
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
}

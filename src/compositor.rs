//! Wayland protocol integration and central compositor state.

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::ImportMemWl;
use smithay::backend::renderer::Texture;
use smithay::backend::SwapBuffersError;
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::delegate_compositor;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_xdg_shell;
use smithay::input::keyboard::LedState;
use smithay::input::pointer::CursorImageStatus;
use smithay::input::Seat;
use smithay::input::SeatHandler;
use smithay::input::SeatState;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::Serial;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::with_states;
use smithay::wayland::compositor::BufferAssignment;
use smithay::wayland::compositor::CompositorClientState;
use smithay::wayland::compositor::CompositorHandler;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::compositor::SurfaceAttributes;
use smithay::wayland::shell::xdg::Configure;
use smithay::wayland::shell::xdg::PositionerState;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::XdgShellHandler;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shm::ShmHandler;
use smithay::wayland::shm::ShmState;
use std::collections::HashMap;

use cgmath::Matrix4;

use crate::input::Camera;
use crate::input_router::{self, InputSink, KeyboardEvent, PointerEventKind};
use crate::interaction::InteractionController;
use crate::layout;
use crate::perf::PerfStats;
use crate::producer::{FrameProducer, FrameResult};
use crate::scene::{Scene, Visual, VisualContent, VisualId};
use crate::renderer;
use tracing::error;
use tracing::info;
use tracing::warn;

#[derive(Debug, Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        info!(?client_id, "client connected");
    }
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        info!(?client_id, ?reason, "client disconnected");
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceLifecycle {
    Created,
    Configured,
    Mapped,
}

#[derive(Debug, Clone)]
pub struct ToplevelInfo {
    pub toplevel: ToplevelSurface,
    pub wl_surface: WlSurface,
    pub app_id: String,
    pub title: String,
    pub lifecycle: SurfaceLifecycle,
    pub visual_id: Option<VisualId>,
    pub size: Option<(i32, i32)>,
}

impl ToplevelInfo {
    fn new(toplevel: ToplevelSurface) -> Self {
        let wl_surface = toplevel.wl_surface().clone();
        let (title, app_id) = with_states(&wl_surface, |states| {
            let title = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|attrs| attrs.lock().unwrap().title.clone().unwrap_or_default())
                .unwrap_or_default();
            let app_id = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|attrs| attrs.lock().unwrap().app_id.clone().unwrap_or_default())
                .unwrap_or_default();
            (title, app_id)
        });
        ToplevelInfo {
            lifecycle: SurfaceLifecycle::Created,
            toplevel,
            wl_surface,
            app_id,
            title,
            visual_id: None,
            size: None,
        }
    }

    fn refresh_metadata(&mut self) {
        with_states(&self.wl_surface, |states| {
            if let Some(attrs) = states.data_map.get::<XdgToplevelSurfaceData>() {
                let attrs = attrs.lock().unwrap();
                self.title = attrs.title.clone().unwrap_or_default();
                self.app_id = attrs.app_id.clone().unwrap_or_default();
            }
        });
    }
}

pub struct LookingGlass {
    pub display_handle: DisplayHandle,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub seat_state: SeatState<Self>,
    pub shm_state: ShmState,
    pub backend: Option<WinitGraphicsBackend<GlesRenderer>>,
    pub toplevels: Vec<ToplevelInfo>,
    pub scene: Scene,
    pub camera: Camera,
    pub spatial_mode: bool,
    pub layout_mode: layout::LayoutMode,
    /// Registered frame producers (e.g. animated textures, Looking Glass)
    producers: Vec<(VisualId, Box<dyn FrameProducer>)>,
    pub perf: PerfStats,
    pub window_size: (f32, f32),
    pub last_mouse: (f64, f64),
    pub interaction: InteractionController,
    input_sinks: HashMap<VisualId, Box<dyn InputSink>>,
}

impl LookingGlass {
    pub fn new(
        display_handle: &DisplayHandle,
        backend: WinitGraphicsBackend<GlesRenderer>,
    ) -> Self {
        let compositor_state = CompositorState::new::<Self>(display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(display_handle);
        let shm_state = ShmState::new::<Self>(display_handle, vec![]);
        let seat_state = SeatState::new();

        LookingGlass {
            display_handle: display_handle.clone(),
            compositor_state,
            xdg_shell_state,
            seat_state,
            shm_state,
            backend: Some(backend),
            toplevels: Vec::new(),
            scene: Scene::default(),
            camera: Camera::new(),
            spatial_mode: false,
            layout_mode: layout::LayoutMode::Freeform,
            producers: Vec::new(),
            perf: PerfStats::new(),
            window_size: (1280.0, 720.0),
            last_mouse: (0.0, 0.0),
            interaction: InteractionController::new(),
            input_sinks: HashMap::new(),
        }
    }

    pub fn cleanup(&mut self) {
        self.toplevels.retain(|t| t.toplevel.alive());
    }

    fn find_toplevel(&mut self, surface: &WlSurface) -> Option<&mut ToplevelInfo> {
        self.toplevels
            .iter_mut()
            .find(|t| t.toplevel.wl_surface() == surface)
    }

    fn handle_commit(&mut self, surface: &WlSurface) {
        // Extract visual_id BEFORE borrowing self mutably
        let existing_vid = self.toplevels.iter()
            .find(|t| t.toplevel.wl_surface() == surface)
            .and_then(|t| t.visual_id);
        let idx = self.toplevels.iter()
            .position(|t| t.toplevel.wl_surface() == surface);
        let Some(idx) = idx else { return };

        // On first commit, compute position data before other borrows
        let first_commit = self.toplevels[idx].lifecycle != SurfaceLifecycle::Mapped;
        let pos_data = if first_commit {
            let z_off = [-200.0, 0.0, 200.0];
            let y_ang = [5.0, 0.0, -5.0];
            let n = self.toplevels.len();
            let z = if idx < z_off.len() { z_off[idx] } else { idx as f32 * 50.0 };
            let ay = if idx < y_ang.len() { y_ang[idx] } else { 0.0 };
            let x = idx as f32 * 20.0 - (n as f32 - 1.0) * 10.0;
            Some((x, z, ay))
        } else {
            None
        };

        // Extract buffer + damage
        let (wl_buffer, damage): (Option<_>, Vec<_>) = with_states(surface, |states| {
            let mut cached = states.cached_state.get::<SurfaceAttributes>();
            let attrs = cached.current();
            let buf = match &attrs.buffer {
                Some(BufferAssignment::NewBuffer(b)) => Some(b.clone()),
                _ => None,
            };
            let dmg = attrs.damage.iter().filter_map(|d| match d {
                smithay::wayland::compositor::Damage::Buffer(r) => Some(*r),
                smithay::wayland::compositor::Damage::Surface(r) => {
                    let bs = attrs.buffer_scale.max(1);
                    Some(smithay::utils::Rectangle::new(
                        smithay::utils::Point::new(r.loc.x * bs, r.loc.y * bs),
                        smithay::utils::Size::new(r.size.w * bs, r.size.h * bs),
                    ))
                }
            }).collect();
            (buf, dmg)
        });
        let Some(wl_buffer) = wl_buffer else { return };

        if let Some(backend) = self.backend.as_mut() {
            let renderer = backend.renderer();
            let result = with_states(surface, |states| {
                renderer.import_shm_buffer(&wl_buffer, Some(states), &damage)
            });
            match result {
                Ok(texture) => {
                    if first_commit {
                        let (x, z, angle_y) = pos_data.unwrap();
                        let tex_size = texture.size();
                        self.toplevels[idx].lifecycle = SurfaceLifecycle::Mapped;
                        self.toplevels[idx].size = Some((tex_size.w, tex_size.h));

                        let mut visual = Visual::new(
                            VisualContent::WaylandSurface(texture),
                            smithay::utils::Rectangle::new(
                                smithay::utils::Point::new(0, 0),
                                smithay::utils::Size::new(tex_size.w, tex_size.h),
                            ),
                        );
                        use cgmath::Deg;
                        use cgmath::Rotation3;
                        visual.transform.position = cgmath::Vector3::new(x, 0.0, z);
                        visual.transform.rotation = cgmath::Quaternion::from_angle_y(Deg(angle_y));
                        let visual_id = visual.id;
                        self.toplevels[idx].visual_id = Some(visual_id);
                        self.scene.add(visual);
                        info!(?visual_id, app_id = %self.toplevels[idx].app_id, "surface mapped");
                    } else if let Some(vid) = existing_vid {
                        if let Some(visual) = self.scene.get_mut(vid) {
                            if let Some(dst) = visual.texture_mut() {
                                *dst = texture;
                            }
                        }
                    }
                }
                Err(e) => warn!(?e, "SHM import failed"),
            }
        }
    }

    /// Create a visual from external (non-Wayland) pixel data.
    /// This demonstrates the VisualContent::ExternalTexture abstraction.
    /// A real external producer (e.g. Looking Glass framebuffer) would
    /// provide GPU textures through this same path.
    pub fn add_external_visual(&mut self, pixels: Vec<u8>, width: u32, height: u32) {
        use smithay::backend::allocator::Fourcc;
        use smithay::backend::renderer::ImportMem;

        let Some(backend) = self.backend.as_mut() else { return };
        let renderer = backend.renderer();
        if let Ok(texture) = renderer.import_memory(
            &pixels,
            Fourcc::Abgr8888,
            (width as i32, height as i32).into(),
            false,
        ) {
            let visual = Visual::new(
                VisualContent::ExternalTexture(texture),
                smithay::utils::Rectangle::new(
                    smithay::utils::Point::new(0, 0),
                    smithay::utils::Size::new(width as i32, height as i32),
                ),
            );
            info!(visual_id = ?visual.id, width, height, "external visual created");
            self.scene.add(visual);
        }
    }

    /// Add a benchmark visual at a grid position
    /// Register an InputSink for a visual.
    pub fn register_input_sink(&mut self, vid: VisualId, sink: Box<dyn InputSink>) {
        self.input_sinks.insert(vid, sink);
        info!(?vid, "input sink registered");
    }

    pub fn add_benchmark_visual(&mut self, mut producer: Box<dyn FrameProducer>, index: usize, total: usize) {
        let Some(backend) = self.backend.as_mut() else { return };
        let renderer = backend.renderer();
        if !matches!(producer.update(renderer), FrameResult::Unchanged) { return; }
        let (w, h) = producer.size();
        let cols = (total as f32).sqrt().ceil() as i32;
        let spacing = 160;
        let gx = (index as i32 % cols) * spacing - (cols * spacing) / 2;
        let gy = (index as i32 / cols) * spacing - (total as i32 / cols * spacing) / 2;

        let mut visual = Visual::new(
            VisualContent::ExternalTexture(producer.texture().clone()),
            smithay::utils::Rectangle::new(
                smithay::utils::Point::new(0, 0),
                smithay::utils::Size::new(w as i32, h as i32),
            ),
        );
        use cgmath::Deg;
        use cgmath::Rotation3;
        visual.transform.position = cgmath::Vector3::new(gx as f32, gy as f32, 0.0);
        // Rotate odd rows slightly for 3D variety
        if (index / cols as usize) % 2 == 1 {
            visual.transform.rotation = cgmath::Quaternion::from_angle_y(Deg(10.0));
        }
        let vid = visual.id;
        self.scene.add(visual);
        self.producers.push((vid, producer));
    }

    /// Register a frame producer and create its Visual in the scene.
    /// If the producer fails on its first update, it is not added.
    /// Returns the VisualId if the producer was registered successfully.
    pub fn add_producer(&mut self, mut producer: Box<dyn FrameProducer>) -> Option<VisualId> {
        let Some(backend) = self.backend.as_mut() else { return None };
        let renderer = backend.renderer();
        let result = producer.update(renderer);
        let is_ok = matches!(result, FrameResult::Updated | FrameResult::Unchanged | FrameResult::Resized(_, _));
        if !is_ok {
            match result {
                FrameResult::Error(msg) => warn!(?msg, "frame producer not added: initial update failed"),
                FrameResult::Finished => info!("frame producer finished before registration"),
                _ => {}
            }
            return None;
        }

        let (w, h) = producer.size();
        let tex = producer.texture().clone();
        let mut visual = Visual::new(
            VisualContent::ExternalTexture(tex),
            smithay::utils::Rectangle::new(
                smithay::utils::Point::new(0, 0),
                smithay::utils::Size::new(w as i32, h as i32),
            ),
        );
        visual.transform.position = cgmath::Vector3::new(0.0, 200.0, 0.0);
        let vid = visual.id;

        // Try to create an InputSink from the producer before moving it
        if let Some(sink) = producer.create_input_sink() {
            self.input_sinks.insert(vid, sink);
            info!(?vid, "input sink registered from producer");
        }

        self.scene.add(visual);
        self.producers.push((vid, producer));
        info!(visual_id = ?vid, width = w, height = h, "frame producer registered");
        Some(vid)
    }

    pub fn render(&mut self) {
        use crate::perf::PipelineStage;
        let t_frame = std::time::Instant::now();
        self.perf.begin_frame();

        // Step 1: Update frame producers (measure each)
        let mut removed = Vec::new();
        let mut updates: Vec<(VisualId, GlesTexture)> = Vec::new();
        {
            let backend = match self.backend.as_mut() {
                Some(b) => b,
                None => return,
            };
            let renderer = backend.renderer();
            let mut i = 0;
            while i < self.producers.len() {
                let (vid, producer) = &mut self.producers[i];
                let t0 = std::time::Instant::now();
                let result = producer.update(renderer);
                let dt = t0.elapsed().as_nanos() as u64;
                match result {
                    FrameResult::Updated => {
                        self.perf.record_stage(PipelineStage::ProducerUpdate, dt);
                        updates.push((*vid, producer.texture().clone()));
                        i += 1;
                    }
                    FrameResult::Unchanged => {
                        self.perf.record_stage(PipelineStage::ProducerUpdate, dt);
                        self.perf.record_dropped();
                        i += 1;
                    }
                    FrameResult::Resized(_w, _h) => {
                        self.perf.record_stage(PipelineStage::ProducerUpdate, dt);
                        updates.push((*vid, producer.texture().clone()));
                        i += 1;
                    }
                    FrameResult::Error(msg) => {
                        warn!(?vid, ?msg, "producer error");
                        i += 1;
                    }
                    FrameResult::Finished => {
                        info!(?vid, "producer finished");
                        removed.push(*vid);
                        self.producers.swap_remove(i);
                    }
                }
            }
        }

        // Step 2: Copy updated textures to Visuals
        let t_tex_start = std::time::Instant::now();
        for (vid, tex) in &updates {
            if let Some(visual) = self.scene.get_mut(*vid) {
                if let Some(dst) = visual.texture_mut() {
                    *dst = tex.clone();
                }
            }
        }
        self.perf.record_stage(PipelineStage::TexCopy, t_tex_start.elapsed().as_nanos() as u64);

        // Step 3: Remove finished Visuals
        let t_rem_start = std::time::Instant::now();
        for vid in &removed {
            self.scene.remove(*vid);
        }
        self.perf.record_stage(PipelineStage::Remove, t_rem_start.elapsed().as_nanos() as u64);

        // Step 3.5: Apply layout
        let (w, h) = self.window_size;
        let world_w = w;
        let world_h = h;
        let detached = self.scene.detached_set.clone();
        layout::apply_layout(
            &mut self.scene,
            self.layout_mode,
            &layout::LayoutConfig::default(),
            &detached,
            world_w,
            world_h,
        );

        // Step 4: Camera + render
        let Some(backend) = self.backend.as_mut() else { return; };
        if !self.spatial_mode {
            self.camera.position = cgmath::Point3::new(0.0, 0.0, 500.0);
            self.camera.yaw = 0.0;
            self.camera.pitch = 0.0;
        }
        let (w, h) = self.window_size;
        let view = self.camera.view_matrix();
        let proj = if self.spatial_mode {
            cgmath::perspective(cgmath::Deg(45.0), w / h, 1.0, 10000.0)
        } else {
            cgmath::ortho(-w / 2.0, w / 2.0, -h / 2.0, h / 2.0, -1000.0, 1000.0)
        };
        if let Err(SwapBuffersError::ContextLost(e)) = renderer::render_scene(backend, &self.scene, &view, &proj, &mut self.perf) {
            error!(?e, "Context lost");
            self.backend = None;
        }

        self.perf.record_stage(PipelineStage::Total, t_frame.elapsed().as_nanos() as u64);
        self.perf.record_frame();
    }

    /// Compute proj × view matrix for the current camera.
    fn proj_view(&self) -> Matrix4<f32> {
        let (w, h) = self.window_size;
        let proj = if self.spatial_mode {
            cgmath::perspective(cgmath::Deg(45.0), w / h, 1.0, 10000.0)
        } else {
            cgmath::ortho(-w / 2.0, w / 2.0, -h / 2.0, h / 2.0, -1000.0, 1000.0)
        };
        proj * self.camera.view_matrix()
    }

    /// Route a pointer event to the selected visual's InputSink.
    /// Focus follows click: sets focused visual to the selected one.
    fn route_to_content(&mut self, kind: PointerEventKind, x: f64, y: f64) {
        let Some(vid) = self.scene.selected_id else { return };

        // Focus follows click + bring to front
        if kind == PointerEventKind::Down {
            self.scene.focus(Some(vid));
            self.scene.bring_to_front(vid);
            info!(?vid, "focus set, brought to front");
        }

        let (w, h) = self.window_size;
        let ndc_x = (x as f32 / w) * 2.0 - 1.0;
        let ndc_y = -((y as f32 / h) * 2.0 - 1.0);
        let pv = self.proj_view();

        let transform_and_size = self.scene.visuals.iter().find(|v| v.id == vid).map(|v| {
            (v.transform.clone(), v.geometry.size.w as f32, v.geometry.size.h as f32)
        });
        let Some((transform, gw, gh)) = transform_and_size else { return };
        let Some(sink) = self.input_sinks.get_mut(&vid) else { return };

        if let Some((u, v)) = input_router::screen_to_visual_uv(
            &pv, ndc_x, ndc_y, &transform, gw, gh,
        ) {
            sink.handle_pointer(kind, u, v);
        }
    }

    /// Route a keyboard event to the focused visual's InputSink.
    /// key: winit platform key code (X11 keycodes when under X11, offset +8 from evdev).
    /// The offset is subtracted to get raw evdev codes for HID mapping.
    fn route_keyboard(&mut self, key: u32, pressed: bool) {
        let Some(vid) = self.scene.focused_id else { return };
        let Some(sink) = self.input_sinks.get_mut(&vid) else { return };
        // Winit on X11 adds 8 to the evdev scancode. Detect and adjust.
        let evdev = if key > 8 { key - 8 } else { key };
        let hid = input_router::linux_to_hid(evdev);
        if hid == 0 {
            return; // unmapped key
        }
        sink.handle_keyboard(KeyboardEvent { key: hid, pressed });
    }

    /// Public entry point for a pointer button press.
    pub fn handle_pointer_down(&mut self, x: f64, y: f64, shift: bool, ctrl: bool, alt: bool) {
        self.interaction.window_size = self.window_size;
        let mode = self.interaction.handle_pointer_down(
            x, y, &mut self.scene, &self.camera, self.spatial_mode, shift, ctrl, alt,
        );
        match mode {
            Some(_) => {
                // Manipulation started — event consumed by compositor
            }
            None => {
                // No manipulation — route to content if a visual is selected
                self.route_to_content(PointerEventKind::Down, x, y);
            }
        }
    }

    /// Public entry point for pointer button release.
    pub fn handle_pointer_up(&mut self, x: f64, y: f64) {
        let has_active = self.interaction.is_dragging();
        self.interaction.handle_pointer_up();
        if !has_active {
            self.route_to_content(PointerEventKind::Up, x, y);
        }
    }

    /// Public entry point for pointer motion.
    pub fn handle_pointer_move(&mut self, x: f64, y: f64) {
        self.last_mouse = (x, y);
        self.interaction.window_size = self.window_size;
        self.interaction.handle_pointer_move(x, y, &mut self.scene, &self.camera, self.spatial_mode);
    }

    /// Center the camera on the currently selected visual.
    /// Returns true if a visual was framed.
    pub fn frame_selected(&mut self) -> bool {
        let Some(vid) = self.scene.selected_id else { return false };
        let (w, h) = self.window_size;
        if let Some(pos) = layout::frame_visual(vid, &self.scene, w, h) {
            self.camera.position = cgmath::Point3::new(pos.x, pos.y, pos.z);
            info!(?vid, ?pos, "camera framed on visual");
            true
        } else {
            false
        }
    }

    /// Public entry point for keyboard events.
    /// Routes to the focused visual's InputSink.
    /// Tab key (23) is always consumed by the compositor for spatial mode toggle.
    pub fn handle_key(&mut self, linux_key: u32, pressed: bool) {
        // Tab is always compositor-only
        if linux_key == 23 && pressed {
            self.spatial_mode = !self.spatial_mode;
            tracing::info!(spatial_mode = self.spatial_mode, "mode toggled");
            return;
        }
        // Route keyboard to focused visual
        self.route_keyboard(linux_key, pressed);
    }
}

impl CompositorHandler for LookingGlass {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a Client,
    ) -> &'a CompositorClientState {
        let state: &ClientState = client.get_data().unwrap();
        &state.compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        self.handle_commit(surface);
    }
}

delegate_compositor!(LookingGlass);

impl XdgShellHandler for LookingGlass {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.cleanup();
        let mut info = ToplevelInfo::new(surface);
        info.toplevel.send_configure();
        info.lifecycle = SurfaceLifecycle::Configured;
        info!(
            app_id = %info.app_id,
            title = %info.title,
            "toplevel created"
        );
        self.toplevels.push(info);
    }

    fn new_popup(
        &mut self,
        _surface: smithay::wayland::shell::xdg::PopupSurface,
        _positioner: PositionerState,
    ) {
    }

    fn grab(
        &mut self,
        _surface: smithay::wayland::shell::xdg::PopupSurface,
        _seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _serial: Serial,
    ) {
    }

    fn reposition_request(
        &mut self,
        _surface: smithay::wayland::shell::xdg::PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }

    fn ack_configure(&mut self, _surface: WlSurface, configure: Configure) {
        info!(?configure, "configure acknowledged");
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(info) = self.find_toplevel(surface.wl_surface()) {
            let old = info.title.clone();
            info.refresh_metadata();
            if info.title != old {
                info!(title = %info.title, "title changed");
            }
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(info) = self.find_toplevel(surface.wl_surface()) {
            let old = info.app_id.clone();
            info.refresh_metadata();
            if info.app_id != old {
                info!(app_id = %info.app_id, "app_id changed");
            }
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface();
        if let Some(idx) = self
            .toplevels
            .iter()
            .position(|t| t.toplevel.wl_surface() == wl_surface)
        {
            let info = self.toplevels.remove(idx);
            if let Some(vid) = info.visual_id {
                self.scene.remove(vid);
            }
            info!(
                app_id = %info.app_id,
                title = %info.title,
                lifecycle = ?info.lifecycle,
                "surface destroyed"
            );
        }
    }

    fn client_destroyed(&mut self, _client: smithay::wayland::shell::xdg::ShellClient) {
        info!("shell client destroyed");
    }
}

delegate_xdg_shell!(LookingGlass);

impl SeatHandler for LookingGlass {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn led_state_changed(&mut self, _seat: &Seat<Self>, _led_state: LedState) {}
}

delegate_seat!(LookingGlass);

impl ShmHandler for LookingGlass {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for LookingGlass {
    fn buffer_destroyed(
        &mut self,
        _buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
    }
}

delegate_shm!(LookingGlass);

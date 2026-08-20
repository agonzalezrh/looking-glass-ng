//! Wayland protocol integration and central compositor state.

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::ImportMemWl;
use smithay::backend::renderer::Renderer;
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
use smithay::utils::Point;
use smithay::utils::Rectangle;
use smithay::utils::Serial;
use smithay::utils::Size;
use smithay::utils::Transform;
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
    pub texture: Option<GlesTexture>,
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
            texture: None,
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
        let idx = self
            .toplevels
            .iter()
            .position(|t| t.toplevel.wl_surface() == surface);
        let Some(idx) = idx else {
            return;
        };
        if self.toplevels[idx].lifecycle != SurfaceLifecycle::Configured {
            return;
        }
        let wl_buffer = with_states(surface, |states| {
            let mut cached = states.cached_state.get::<SurfaceAttributes>();
            let attrs = cached.current();
            match &attrs.buffer {
                Some(BufferAssignment::NewBuffer(buf)) => Some(buf.clone()),
                _ => None,
            }
        });
        let Some(wl_buffer) = wl_buffer else {
            return;
        };
        // Import the buffer using the backend's renderer
        if let Some(backend) = self.backend.as_mut() {
            let renderer = backend.renderer();
            let result = with_states(surface, |states| {
                renderer.import_shm_buffer(&wl_buffer, Some(states), &[])
            });
            match result {
                Ok(texture) => {
                    let info = &mut self.toplevels[idx];
                    info.lifecycle = SurfaceLifecycle::Mapped;
                    let tex_size = texture.size();
                    info.size = Some((tex_size.w, tex_size.h));
                    info.texture = Some(texture);
                    info!(
                        app_id = %info.app_id,
                        title = %info.title,
                        size = ?info.size,
                        "surface mapped"
                    );
                }
                Err(e) => {
                    warn!(?e, "failed to import SHM buffer");
                }
            }
        }
    }

    pub fn render(&mut self) {
        let window_size = {
            let Some(backend) = self.backend.as_ref() else {
                return;
            };
            backend.window_size()
        };

        let Some(backend) = self.backend.as_mut() else {
            return;
        };

        fn do_render(
            backend: &mut WinitGraphicsBackend<GlesRenderer>,
            toplevels: &[ToplevelInfo],
            window_size: smithay::utils::Size<i32, smithay::utils::Physical>,
        ) -> Result<(), SwapBuffersError> {
            let (renderer, mut target) = backend.bind()?;
            let mut frame = renderer.render(&mut target, window_size, Transform::Normal)?;

            let mut info_idx = 0;
            while info_idx < toplevels.len() {
                let info = &toplevels[info_idx];
                if info.lifecycle != SurfaceLifecycle::Mapped {
                    info_idx += 1;
                    continue;
                }
                if let Some(ref texture) = info.texture {
                    let tex_size = texture.size();
                    let dest = Rectangle::new(
                        Point::new(10, 10 + info_idx as i32 * (tex_size.h + 10)),
                        Size::new(tex_size.w, tex_size.h),
                    );
                    let src = Rectangle::new(
                        Point::new(0.0, 0.0),
                        Size::new(tex_size.w as f64, tex_size.h as f64),
                    );
                    let _ = frame.render_texture_from_to(
                        texture,
                        src,
                        dest,
                        &[dest],
                        &[],
                        Transform::Normal,
                        1.0,
                        None,
                        &[],
                    );
                }
                info_idx += 1;
            }

            drop(frame);
            drop(target);
            backend.submit(None)
        }

        if let Err(SwapBuffersError::ContextLost(e)) = do_render(backend, &self.toplevels, window_size) {
            error!(?e, "Context lost");
            self.backend = None;
        }
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

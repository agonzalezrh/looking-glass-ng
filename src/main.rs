mod backend;
mod compositor;
mod config;
mod input;
mod producer;
mod renderer;
mod scene;
mod window;

use std::sync::Arc;

use compositor::{ClientState, LookingGlass};
use producer::AnimatedCheckerboard;
use smithay::backend::input::{AbsolutePositionEvent, InputEvent, KeyboardKeyEvent};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::PostAction;
use smithay::reexports::calloop::Interest;
use smithay::reexports::calloop::Mode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "looking_glass_ng=info,warn".into()),
        )
        .init();

    tracing::info!("Looking Glass NG starting");

    let mut event_loop: EventLoop<'static, LookingGlass> =
        EventLoop::try_new().expect("Failed to create event loop");
    let handle = event_loop.handle();

    let display: Display<LookingGlass> = Display::new().expect("Failed to create Wayland display");
    let display_handle = display.handle();

    // Initialize the winit backend
    let (backend, winit_source) =
        winit::init::<GlesRenderer>().expect("Failed to initialize winit backend");

    let mut state = LookingGlass::new(&display_handle, backend);

    // Register the animated checkerboard frame producer
    // This proves the external frame producer pipeline works with continuous updates.
    // A Looking Glass KVMFR producer would be registered the same way.
    if let Some(prod) = AnimatedCheckerboard::new(
        state.backend.as_mut().map(|b| b.renderer()).unwrap(),
    ) {
        state.add_producer(Box::new(prod));
    }

    // Wayland socket listener
    let source = ListeningSocketSource::new_auto().expect("Failed to create listening socket");
    let socket_name = source.socket_name().to_string_lossy().into_owned();
    handle
        .insert_source(source, |client_stream, _, state| {
            if let Err(err) = state
                .display_handle
                .insert_client(client_stream, Arc::new(ClientState::default()))
            {
                tracing::warn!("Error adding wayland client: {}", err);
            };
        })
        .expect("Failed to init wayland socket source");
    tracing::info!("Listening on wayland socket: {}", socket_name);

    // Wayland display dispatch source
    handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                let inner = unsafe { display.get_mut() };
                let _ = inner.dispatch_clients(state);
                let _ = inner.flush_clients();
                state.render();
                Ok(PostAction::Continue)
            },
        )
        .expect("Failed to init wayland server source");

    // Winit event source + rendering
    handle
        .insert_source(winit_source, |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                tracing::debug!("Window resized to {:?}", size);
                state.render();
            }
            WinitEvent::Input(event) => {
                match event {
                    InputEvent::Keyboard { event } => {
                        let key = event.key_code();
                        let pressed = event.state() == smithay::backend::input::KeyState::Pressed;
                        // Tab key (keycode 23) toggles spatial mode
                        if u32::from(key) == 23 && pressed {
                            state.spatial_mode = !state.spatial_mode;
                            tracing::info!(spatial_mode = state.spatial_mode, "mode toggled");
                        }
                        state.camera.handle_key(key.into(), pressed, 1.0);
                    }
                    InputEvent::PointerMotionAbsolute { event } => {
                        let x = event.x();
                        let y = event.y();
                        state.camera.handle_mouse_absolute(x, y);
                    }
                    _ => {}
                }
                state.render();
            }
            WinitEvent::CloseRequested => {
                tracing::info!("Close requested, shutting down");
                state.backend.take();
            }
            _ => {}
        })
        .expect("Failed to register winit event source");

    tracing::info!("Looking Glass NG running on {}", socket_name);

    let _ = event_loop.run(None, &mut state, |_| {});
}

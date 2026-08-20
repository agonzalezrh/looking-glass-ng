mod backend;
mod compositor;
mod config;
mod input;
mod renderer;
mod scene;
mod window;

use std::sync::Arc;

use compositor::{ClientState, LookingGlass};
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

    let mut state = LookingGlass::new(&display_handle);

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
                // Safety: we do not drop the display and the callback is sequential
                let inner = unsafe { display.get_mut() };
                let _ = inner.dispatch_clients(state);
                let _ = inner.flush_clients();
                Ok(PostAction::Continue)
            },
        )
        .expect("Failed to init wayland server source");

    // Winit backend
    let (_backend, winit_source) =
        winit::init::<smithay::backend::renderer::gles::GlesRenderer>()
            .expect("Failed to initialize winit backend");

    handle
        .insert_source(winit_source, move |event, _, _state| match event {
            WinitEvent::Resized { size, .. } => {
                tracing::debug!("Window resized to {:?}", size);
            }
            WinitEvent::Input(event) => {
                let _ = event;
            }
            WinitEvent::CloseRequested => {
                tracing::info!("Close requested, shutting down");
            }
            _ => {}
        })
        .expect("Failed to register winit event source");

    tracing::info!("Looking Glass NG running on {}", socket_name);

    let _ = event_loop.run(None, &mut state, |_| {});
}

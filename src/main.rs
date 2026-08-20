mod backend;
mod compositor;
mod config;
mod input;
mod renderer;
mod scene;
mod window;

use smithay::backend::winit::{self, WinitEvent};
use smithay::reexports::calloop::EventLoop;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "looking_glass_ng=info,warn".into()),
        )
        .init();

    tracing::info!("Looking Glass NG starting");

    let (_backend, winit_source) =
        winit::init::<smithay::backend::renderer::gles::GlesRenderer>()
            .expect("Failed to initialize winit backend");

    let mut event_loop: EventLoop<'static, ()> =
        EventLoop::try_new().expect("Failed to create event loop");

    let handle = event_loop.handle();

    handle
        .insert_source(winit_source, move |event, _, _| match event {
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

    tracing::info!("Looking Glass NG running");

    let _ = event_loop.run(None, &mut (), |_| {});
}

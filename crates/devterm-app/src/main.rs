//! DevTerm application entry point.
//!
//! The M0 walking skeleton: a winit event loop hosting one full-window PowerShell pane,
//! wiring ConPTY output through the terminal model into the wgpu renderer.

mod app;
mod keymap;

use anyhow::Result;
use winit::event_loop::{ControlFlow, EventLoop};

use app::{App, UserEvent};
use devterm_config::Config;

fn main() -> Result<()> {
    // Our logs default to `info`, but wgpu's internals are noisy (per-reconfigure backend
    // chatter); keep them at warn/error. `RUST_LOG` still overrides everything.
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("info,wgpu_core=warn,wgpu_hal=error,naga=warn"),
    )
    .init();

    let config = Config::load(&Config::default_path()).unwrap_or_default();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    // Redraw is driven by input and PTY wakeups, so wait for events rather than polling.
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = App::new(config, proxy);
    event_loop.run_app(&mut app)?;

    Ok(())
}

//! DevTerm application entry point.
//!
//! A winit event loop hosting a multi-pane terminal: splits/focus/resize over a layout
//! tree, config-driven keybindings/theme/shell, selection + clipboard, and hot-reload of
//! `config.toml`. Wires PTY output through the terminal model into the wgpu renderer.

// Ship as a pure GUI app on Windows: no console window pops up on launch (behaves like
// Windows Terminal / VS Code). Kept only for release builds so debug runs still get a
// console for `env_logger` output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod keymap;
mod update;

use std::path::PathBuf;

use anyhow::Result;
use winit::event_loop::{ControlFlow, EventLoop};

use app::{App, UserEvent, spawn_config_watcher};
use devterm_config::Config;

fn main() -> Result<()> {
    // Our logs default to `info`, but wgpu's internals are noisy (per-reconfigure backend
    // chatter); keep them at warn/error. `RUST_LOG` still overrides everything.
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("info,wgpu_core=warn,wgpu_hal=error,naga=warn"),
    )
    .init();

    // `devterm [PATH]` — open in PATH, like `code <dir>`. With no argument the inherited
    // working directory (i.e. wherever the shell launched us) is used as-is. Setting the
    // process CWD here means every pane, tab and split inherits it via `cwd: None`. All our
    // own paths (config in %APPDATA%, the exe for self-update) are absolute, so this is safe.
    if let Some(dir) = std::env::args_os().nth(1).filter(|a| !is_flag(a)) {
        let path = PathBuf::from(&dir);
        if let Err(err) = std::env::set_current_dir(&path) {
            log::warn!(
                "cannot open in {}: {err}; using the current directory",
                path.display()
            );
        }
    }

    let config = Config::load(&Config::default_path()).unwrap_or_default();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    // Redraw is driven by input and PTY wakeups, so wait for events rather than polling.
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    // Hot-reload watcher: kept alive for the program's lifetime (dropping it stops watching).
    let _config_watcher = spawn_config_watcher(proxy.clone());

    // Clear any binary left behind by a prior self-update, then check for a newer release in
    // the background (posts `UserEvent::UpdateAvailable` if one is found).
    update::cleanup_leftovers();
    update::spawn_check(proxy.clone());

    let mut app = App::new(config, proxy);
    event_loop.run_app(&mut app)?;

    Ok(())
}

/// True for `-`-prefixed arguments, reserved for future flags so they are never mistaken for
/// a startup directory.
fn is_flag(arg: &std::ffi::OsStr) -> bool {
    arg.to_str().is_some_and(|s| s.starts_with('-'))
}

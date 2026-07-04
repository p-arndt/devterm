//! Background `config.toml` file watcher that posts reload events to the winit loop.

use notify::{EventKind, RecursiveMode, Watcher};
use winit::event_loop::EventLoopProxy;

use devterm_config::Config;

use super::state::UserEvent;

/// Watch the `config.toml` directory and post [`UserEvent::ReloadConfig`] on change.
///
/// Returns the watcher, which the caller must keep alive (dropping it stops watching);
/// `None` disables hot-reload (directory absent, or the watcher could not be created).
pub fn spawn_config_watcher(
    proxy: EventLoopProxy<UserEvent>,
) -> Option<notify::RecommendedWatcher> {
    let path = Config::default_path();
    let dir = path.parent()?.to_path_buf();
    if dir.as_os_str().is_empty() || !dir.exists() {
        log::info!("config directory {dir:?} absent; hot-reload disabled");
        return None;
    }

    let target = path;
    // Debounce: editors emit a burst of events per save. Start in the past so the first
    // change always passes.
    let last = std::sync::Mutex::new(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or_else(std::time::Instant::now),
    );

    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else {
                return;
            };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                return;
            }
            if !event.paths.contains(&target) {
                return;
            }
            if let Ok(mut guard) = last.lock() {
                let now = std::time::Instant::now();
                if now.duration_since(*guard) < std::time::Duration::from_millis(150) {
                    return;
                }
                *guard = now;
            }
            let _ = proxy.send_event(UserEvent::ReloadConfig);
        }) {
            Ok(watcher) => watcher,
            Err(err) => {
                log::warn!("failed to create config watcher: {err}");
                return None;
            }
        };

    if let Err(err) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
        log::warn!("failed to watch config dir {dir:?}: {err}");
        return None;
    }
    log::info!("watching {dir:?} for config changes");
    Some(watcher)
}

//! One terminal pane and the constructor that spawns its child process.

use crossbeam_channel::Receiver;
use winit::event_loop::EventLoopProxy;

use devterm_config::Config;
use devterm_pty::{Pty, PtyCommandSpec, PtyEvent, PtySize};
use devterm_term::{Palette, Term};

use super::state::UserEvent;

/// One terminal pane: its child process and the emulator model driving it.
pub(super) struct Pane {
    pub(super) pty: Pty,
    pub(super) term: Term,
    pub(super) events: Receiver<PtyEvent>,
}

/// Build a `{ pty, term }` pane at `cols` x `rows` using the resolved shell and palette.
pub(super) fn build_pane(
    config: &Config,
    proxy: &EventLoopProxy<UserEvent>,
    palette: Palette,
    cols: u16,
    rows: u16,
) -> anyhow::Result<Pane> {
    let mut term = Term::new(cols, rows, config.scrollback_lines);
    term.set_palette(palette);

    // Resolve the shell from config; an empty program means "app default".
    let resolved = config.resolve_shell();
    let spec = if resolved.program.is_empty() {
        let mut spec = PtyCommandSpec::default_shell();
        spec.args = resolved.args;
        spec
    } else {
        PtyCommandSpec {
            program: resolved.program,
            args: resolved.args,
            cwd: None,
            env: Vec::new(),
        }
    };

    let proxy = proxy.clone();
    let pty = Pty::spawn(&spec, PtySize { cols, rows }, move || {
        let _ = proxy.send_event(UserEvent::Wake);
    })?;
    let events = pty.events();
    Ok(Pane { pty, term, events })
}

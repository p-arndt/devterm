//! Value types for the PTY layer: terminal size, child events, and the command spec.

use std::path::PathBuf;

use portable_pty::PtySize as NativePtySize;

use crate::shell::default_shell_program;

/// Terminal size in cells (pixel_* may be 0; ConPTY ignores them).
#[derive(Clone, Copy, Debug)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl From<PtySize> for NativePtySize {
    fn from(size: PtySize) -> Self {
        NativePtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// What the child terminal produced or its exit.
#[derive(Clone, Debug)]
pub enum PtyEvent {
    /// Raw bytes from the child (feed straight into `devterm_term::Term::advance`).
    Output(Vec<u8>),
    /// The child process ended; exit code if known.
    Exited(Option<i32>),
}

/// How to launch the shell.
#[derive(Clone, Debug)]
pub struct PtyCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl PtyCommandSpec {
    /// The platform default interactive shell.
    ///
    /// - Windows: PowerShell 7 (`pwsh.exe`) if on PATH, else Windows PowerShell
    ///   (`powershell.exe`).
    /// - Unix: the user's `$SHELL` if set, else the first of `bash`, `zsh`, `sh`
    ///   found on `PATH`, else `/bin/sh` as a last resort.
    ///
    /// Resolution happens at spawn time.
    pub fn default_shell() -> Self {
        PtyCommandSpec {
            program: default_shell_program(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
        }
    }
}

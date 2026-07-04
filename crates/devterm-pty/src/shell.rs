//! Default shell resolution: pick the platform's interactive shell and probe `PATH`.

use std::path::PathBuf;

/// Windows: PowerShell 7 if available, else Windows PowerShell, else `cmd.exe`.
///
/// `cmd.exe` is the last-resort fallback for stripped-down installs where neither
/// PowerShell is on `PATH`; it always exists on Windows, so resolution never fails.
#[cfg(windows)]
pub(crate) fn default_shell_program() -> String {
    if find_on_path("pwsh.exe").is_some() {
        "pwsh.exe".to_string()
    } else if find_on_path("powershell.exe").is_some() {
        "powershell.exe".to_string()
    } else {
        "cmd.exe".to_string()
    }
}

/// Unix: honor `$SHELL`, then probe common shells, then fall back to `/bin/sh`.
#[cfg(not(windows))]
pub(crate) fn default_shell_program() -> String {
    if let Some(shell) = std::env::var_os("SHELL")
        && !shell.is_empty()
    {
        return shell.to_string_lossy().into_owned();
    }
    for candidate in ["bash", "zsh", "sh"] {
        if let Some(path) = find_on_path(candidate) {
            return path.to_string_lossy().into_owned();
        }
    }
    "/bin/sh".to_string()
}

/// Look up an executable name on the `PATH` environment variable.
///
/// Returns the first matching absolute path, or `None` if not found.
fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::PtyCommandSpec;

    #[cfg(windows)]
    #[test]
    fn default_shell_picks_a_windows_shell() {
        let spec = PtyCommandSpec::default_shell();
        assert!(
            matches!(
                spec.program.as_str(),
                "pwsh.exe" | "powershell.exe" | "cmd.exe"
            ),
            "unexpected default shell: {}",
            spec.program
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn default_shell_is_non_empty_on_unix() {
        // Whatever the environment, resolution must yield a runnable program
        // name — never an empty string (which would fail to spawn).
        let spec = PtyCommandSpec::default_shell();
        assert!(
            !spec.program.is_empty(),
            "default shell program should not be empty"
        );
    }
}

//! Shell presets and resolution.
//!
//! [`ShellChoice`] is a friendly enum in `config.toml`; [`crate::Config::resolve_shell`]
//! turns it (or an explicit `shell_program`) into a [`ResolvedShell`]. Resolution is
//! pure and best-effort — it never spawns or probes for executables.

use serde::{Deserialize, Serialize};

/// A friendly shell preset selectable in `config.toml`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShellChoice {
    /// Let the app pick (its own default-shell resolution).
    #[default]
    Auto,
    /// PowerShell Core (`pwsh.exe`).
    Pwsh,
    /// Windows PowerShell (`powershell.exe`).
    WindowsPowerShell,
    /// Classic command prompt (`cmd.exe`).
    Cmd,
    /// Git Bash (`bash.exe -i`).
    GitBash,
    /// Windows Subsystem for Linux (`wsl.exe`).
    Wsl,
}

/// A fully resolved shell command: program + arguments.
///
/// An empty `program` means "no explicit shell chosen" — the app should fall back
/// to its own default-shell resolution, passing `args` along.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ResolvedShell {
    /// Executable to launch; empty means "app default".
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
}

impl ShellChoice {
    /// The Windows-oriented program for this choice, or `None` for [`ShellChoice::Auto`].
    fn program(&self) -> Option<&'static str> {
        match self {
            ShellChoice::Auto => None,
            ShellChoice::Pwsh => Some("pwsh.exe"),
            ShellChoice::WindowsPowerShell => Some("powershell.exe"),
            ShellChoice::Cmd => Some("cmd.exe"),
            ShellChoice::GitBash => Some("bash.exe"),
            ShellChoice::Wsl => Some("wsl.exe"),
        }
    }

    /// Preset arguments implied by this choice.
    fn preset_args(&self) -> Vec<String> {
        match self {
            ShellChoice::GitBash => vec!["-i".to_owned()],
            _ => Vec::new(),
        }
    }
}

impl crate::Config {
    /// Resolve the shell to launch.
    ///
    /// Precedence:
    /// 1. A non-empty `shell_program` wins, used with `shell_args`.
    /// 2. Otherwise map [`Config::shell`](crate::Config::shell) to a Windows-oriented
    ///    program; preset args are prepended to `shell_args`.
    /// 3. [`ShellChoice::Auto`] leaves `program` empty (app falls back to its own
    ///    default), carrying `shell_args` through.
    ///
    /// Pure and best-effort: never spawns or probes the filesystem.
    pub fn resolve_shell(&self) -> ResolvedShell {
        if !self.shell_program.is_empty() {
            return ResolvedShell {
                program: self.shell_program.clone(),
                args: self.shell_args.clone(),
            };
        }

        match self.shell.program() {
            Some(program) => {
                let mut args = self.shell.preset_args();
                args.extend(self.shell_args.iter().cloned());
                ResolvedShell {
                    program: program.to_owned(),
                    args,
                }
            }
            None => ResolvedShell {
                program: String::new(),
                args: self.shell_args.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn explicit_program_overrides_choice() {
        let config = Config {
            shell_program: "/bin/zsh".to_owned(),
            shell_args: vec!["-l".to_owned()],
            shell: ShellChoice::Pwsh,
            ..Config::default()
        };
        let resolved = config.resolve_shell();
        assert_eq!(resolved.program, "/bin/zsh");
        assert_eq!(resolved.args, vec!["-l".to_owned()]);
    }

    #[test]
    fn auto_leaves_program_empty() {
        let config = Config {
            shell: ShellChoice::Auto,
            shell_args: vec!["--login".to_owned()],
            ..Config::default()
        };
        let resolved = config.resolve_shell();
        assert!(resolved.program.is_empty());
        assert_eq!(resolved.args, vec!["--login".to_owned()]);
    }

    #[test]
    fn choice_maps_to_windows_programs() {
        let mk = |c| Config {
            shell: c,
            ..Config::default()
        };
        assert_eq!(mk(ShellChoice::Pwsh).resolve_shell().program, "pwsh.exe");
        assert_eq!(
            mk(ShellChoice::WindowsPowerShell).resolve_shell().program,
            "powershell.exe"
        );
        assert_eq!(mk(ShellChoice::Cmd).resolve_shell().program, "cmd.exe");
        assert_eq!(mk(ShellChoice::Wsl).resolve_shell().program, "wsl.exe");
    }

    #[test]
    fn git_bash_gets_interactive_flag() {
        let config = Config {
            shell: ShellChoice::GitBash,
            ..Config::default()
        };
        let resolved = config.resolve_shell();
        assert_eq!(resolved.program, "bash.exe");
        assert_eq!(resolved.args, vec!["-i".to_owned()]);
    }

    #[test]
    fn preset_args_precede_user_args() {
        let config = Config {
            shell: ShellChoice::GitBash,
            shell_args: vec!["-c".to_owned(), "ls".to_owned()],
            ..Config::default()
        };
        let resolved = config.resolve_shell();
        assert_eq!(resolved.args, vec!["-i", "-c", "ls"]);
    }
}

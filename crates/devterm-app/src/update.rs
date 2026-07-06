//! Lightweight update check with an opt-in, one-click self-update from GitHub Releases.
//!
//! A background thread asks GitHub for the latest published release — at most once per
//! [`CHECK_INTERVAL`], cached in `%APPDATA%\DevTerm\update-check.json` so ordinary launches
//! never wait on the network. If the latest release is newer than the running build, the
//! winit loop is notified ([`UserEvent::UpdateAvailable`]) so it can show a "new version
//! available" prompt. Accepting it downloads the new `devterm.exe` and swaps it in place;
//! the swap takes effect on the next launch (offered immediately as a restart).
//!
//! This is a convenience updater for a personal project, not a hardened supply chain: it
//! deliberately performs no signature or checksum verification. It only downloads from the
//! project's own GitHub release assets over https.

use std::io::Read;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;

/// GitHub coordinates of the published releases.
const REPO_OWNER: &str = "p-arndt";
const REPO_NAME: &str = "devterm";

/// The running build's version (the workspace `Cargo.toml` version, stamped at compile time).
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Refresh the "latest version" cache at most this often, so normal launches don't repeatedly
/// hit the network.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Upper bound on any single network operation, so a slow/unreachable GitHub can't hang the
/// background thread indefinitely.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Cap on the GitHub JSON response (a release payload is a few KB).
const MAX_JSON: u64 = 2 << 20; // 2 MiB
/// Cap on a downloaded binary, so a corrupt/hostile response can't exhaust memory.
#[cfg(all(windows, target_arch = "x86_64"))]
const MAX_BINARY: u64 = 256 << 20; // 256 MiB

/// The release asset name carrying the standalone Windows binary, matching what
/// `.github/workflows/release.yml` uploads.
#[cfg(all(windows, target_arch = "x86_64"))]
const ASSET_NAME: &str = "devterm-x86_64-pc-windows-msvc.exe";

// -----------------------------------------------------------------------------
// GitHub API types (only the fields we use).
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    // Only the Windows self-update path downloads assets; elsewhere the field is unused.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[serde(default)]
    assets: Vec<Asset>,
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

// -----------------------------------------------------------------------------
// Persisted "last check" cache.
// -----------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Unix seconds of the last completed check; 0 if never.
    last_check: u64,
    /// Latest version seen (without a leading `v`); empty if unknown.
    latest: String,
}

fn cache_path() -> Option<PathBuf> {
    devterm_config::Config::default_path()
        .parent()
        .map(|dir| dir.join("update-check.json"))
}

fn load_state() -> State {
    let Some(path) = cache_path() else {
        return State::default();
    };
    let Ok(data) = std::fs::read(&path) else {
        return State::default();
    };
    let mut st: State = serde_json::from_slice(&data).unwrap_or_default();
    // The cached version is printed to the title bar, so give it the same validation the
    // live release tag gets — a tampered cache must not smuggle control characters onto UI.
    if !valid_version(&st.latest) {
        st.latest = String::new();
    }
    st
}

fn save_state(st: &State) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(data) = serde_json::to_vec(st) {
        let _ = std::fs::write(&path, data);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// -----------------------------------------------------------------------------
// Startup entry points.
// -----------------------------------------------------------------------------

/// Spawn the background update check. Posts [`UserEvent::UpdateAvailable`] if a newer release
/// is known (from cache immediately, and/or after a fresh check). Never blocks the caller.
pub fn spawn_check(proxy: EventLoopProxy<UserEvent>) {
    thread::spawn(move || {
        let mut st = load_state();

        // Notify right away if a prior check already recorded a newer version.
        if !st.latest.is_empty() && is_newer(&st.latest, CURRENT) {
            let _ = proxy.send_event(UserEvent::UpdateAvailable(st.latest.clone()));
        }

        // Only hit the network once per interval.
        if now_unix().saturating_sub(st.last_check) < CHECK_INTERVAL.as_secs() {
            return;
        }
        st.last_check = now_unix();
        save_state(&st); // claim the window up front, even if the fetch below fails

        let Ok(rel) = latest_release() else { return };
        let latest = rel.tag_name.trim_start_matches('v').to_string();
        if !valid_version(&latest) {
            return;
        }
        st.latest = latest.clone();
        save_state(&st);
        if is_newer(&latest, CURRENT) {
            let _ = proxy.send_event(UserEvent::UpdateAvailable(latest));
        }
    });
}

/// Best-effort removal of the `devterm.old` file left beside the executable by a prior
/// Windows self-update (a running `.exe` can't be deleted, so the swap renames it aside).
pub fn cleanup_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old"));
    }
}

/// Show the "update available" prompt. On accept, downloads + swaps the binary on a
/// background thread and posts [`UserEvent::UpdateResult`] when done. On non-Windows this
/// just logs (self-update targets the Windows installer build).
pub fn prompt_update(proxy: &EventLoopProxy<UserEvent>, latest: &str) {
    #[cfg(windows)]
    {
        let msg = format!(
            "DevTerm {latest} is available — you have {CURRENT}.\n\nDownload and install it now?"
        );
        if !message_box("DevTerm update", &msg, DialogKind::YesNo) {
            return;
        }
        let proxy = proxy.clone();
        let latest = latest.to_string();
        thread::spawn(move || {
            let result = install(&latest).map(|()| latest.clone());
            let _ = proxy.send_event(UserEvent::UpdateResult(result));
        });
    }
    #[cfg(not(windows))]
    {
        let _ = proxy;
        log::info!(
            "DevTerm {latest} is available (you have {CURRENT}); \
             download it from https://github.com/{REPO_OWNER}/{REPO_NAME}/releases"
        );
    }
}

/// Show the post-install result. Returns `true` if the user chose to restart into the new
/// version (the caller relaunches + exits).
#[must_use]
pub fn prompt_restart(result: &Result<String, String>) -> bool {
    match result {
        Ok(version) => confirm_restart(version),
        Err(err) => {
            notify_error(err);
            false
        }
    }
}

#[cfg(windows)]
fn confirm_restart(version: &str) -> bool {
    let msg = format!("DevTerm {version} was installed.\n\nRestart now to use the new version?");
    message_box("Update complete", &msg, DialogKind::YesNo)
}

#[cfg(not(windows))]
fn confirm_restart(version: &str) -> bool {
    log::info!("DevTerm {version} installed; restart to use it");
    false
}

#[cfg(windows)]
fn notify_error(err: &str) {
    message_box(
        "Update failed",
        &format!("The update could not be installed:\n\n{err}"),
        DialogKind::Ok,
    );
}

#[cfg(not(windows))]
fn notify_error(err: &str) {
    log::error!("update failed: {err}");
}

// -----------------------------------------------------------------------------
// Network + install.
// -----------------------------------------------------------------------------

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .user_agent("devterm-updater")
        .build()
}

fn latest_release() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
    let resp = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("contacting GitHub failed: {e}"))?;

    let mut body = String::new();
    resp.into_reader()
        .take(MAX_JSON)
        .read_to_string(&mut body)
        .map_err(|e| format!("reading GitHub response failed: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("parsing GitHub response failed: {e}"))
}

/// Download the latest binary asset and swap it in for the running executable.
#[cfg(all(windows, target_arch = "x86_64"))]
fn install(_latest: &str) -> Result<(), String> {
    let rel = latest_release()?;
    let asset = rel
        .assets
        .iter()
        .find(|a| a.name == ASSET_NAME)
        .ok_or_else(|| format!("this release has no `{ASSET_NAME}` asset to install"))?;

    // Assets we install must be our own GitHub release downloads, served over https.
    if !asset
        .browser_download_url
        .starts_with("https://github.com/")
    {
        return Err("refusing to download the update from an unexpected URL".into());
    }

    let bin = download(&asset.browser_download_url)?;
    if bin.is_empty() {
        return Err("the downloaded binary was empty".into());
    }

    let exe = std::env::current_exe().map_err(|e| format!("cannot locate DevTerm: {e}"))?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    replace_executable(&exe, &bin)
}

// `install` is only ever called from `prompt_update`'s Windows branch, so it need only
// exist on Windows. This fallback covers non-x86_64 Windows (e.g. arm64).
#[cfg(all(windows, not(target_arch = "x86_64")))]
fn install(_latest: &str) -> Result<(), String> {
    Err("self-update is only supported on 64-bit Windows".into())
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn download(url: &str) -> Result<Vec<u8>, String> {
    let resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("downloading the update failed: {e}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_BINARY + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("reading the update failed: {e}"))?;
    if buf.len() as u64 > MAX_BINARY {
        return Err("the update exceeds its size limit — refusing a truncated install".into());
    }
    Ok(buf)
}

/// Atomically swap the file at `exe` for `new_bin`. A running Windows `.exe` can't be
/// overwritten but *can* be renamed, so the current binary is moved to `devterm.old`
/// (cleaned up on the next launch) and the new one takes its place.
#[cfg(all(windows, target_arch = "x86_64"))]
fn replace_executable(exe: &std::path::Path, new_bin: &[u8]) -> Result<(), String> {
    let dir = exe
        .parent()
        .ok_or_else(|| "cannot resolve the install directory".to_string())?;

    let tmp = dir.join("devterm-update.tmp");
    std::fs::write(&tmp, new_bin).map_err(|e| {
        format!(
            "cannot write to {} ({e}) — reinstall DevTerm or run as admin",
            dir.display()
        )
    })?;

    let old = exe.with_extension("old");
    let _ = std::fs::remove_file(&old); // a stale leftover would block the rename

    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "cannot move the current binary aside ({e}) — is DevTerm running elsewhere?"
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, exe) {
        let _ = std::fs::rename(&old, exe); // put the original back so DevTerm still launches
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot install the new binary ({e})"));
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Native "Yes/No" dialog (Windows).
// -----------------------------------------------------------------------------

#[cfg(windows)]
enum DialogKind {
    YesNo,
    Ok,
}

/// Show a native message box. Returns `true` only when the user clicked "Yes".
#[cfg(windows)]
fn message_box(title: &str, text: &str, kind: DialogKind) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONINFORMATION, MB_OK, MB_YESNO, MessageBoxW,
    };

    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (wtext, wtitle) = (wide(text), wide(title));
    let style = match kind {
        DialogKind::YesNo => MB_YESNO | MB_ICONINFORMATION,
        DialogKind::Ok => MB_OK | MB_ICONINFORMATION,
    };
    // Null owner window: the box is app-modal but not tied to the winit HWND, which is
    // fine since it's shown from the main thread in response to a user-driven event.
    let ret = unsafe { MessageBoxW(std::ptr::null_mut(), wtext.as_ptr(), wtitle.as_ptr(), style) };
    ret == IDYES
}

// -----------------------------------------------------------------------------
// Version comparison (semver-ish; ported from the shenv updater).
// -----------------------------------------------------------------------------

/// A plausible release version: bounded length, starts with a digit, only semver characters.
/// Rejects anything else (including control characters) before it reaches the cache or UI.
fn valid_version(v: &str) -> bool {
    if v.is_empty() || v.len() > 64 || !v.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    v.bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'+'))
}

/// Whether `latest` is strictly newer than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    compare_versions(latest, current) == std::cmp::Ordering::Greater
}

/// Compare two semver-ish versions. A leading `v` and build metadata (`+...`) are ignored;
/// a pre-release (`-pre.1`) sorts below the same core version without one.
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (a_core, a_pre) = split_prerelease(normalize(a));
    let (b_core, b_pre) = split_prerelease(normalize(b));

    match compare_core(a_core, b_core) {
        Ordering::Equal => {}
        non_eq => return non_eq,
    }
    match (a_pre.is_empty(), b_pre.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater, // release > pre-release
        (false, true) => Ordering::Less,
        (false, false) => compare_prerelease(a_pre, b_pre),
    }
}

fn normalize(v: &str) -> &str {
    let v = v.trim().trim_start_matches('v');
    match v.split_once('+') {
        Some((core, _meta)) => core,
        None => v,
    }
}

fn split_prerelease(v: &str) -> (&str, &str) {
    v.split_once('-').unwrap_or((v, ""))
}

fn compare_core(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (ai, bi) => {
                let ord = compare_numeric(ai.unwrap_or("0"), bi.unwrap_or("0"));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

fn compare_numeric(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

/// Semver pre-release precedence: numeric identifiers compared as numbers and ranked below
/// alphanumeric ones; a shorter identifier set sorts below a longer one with an equal prefix.
fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                    (Ok(_), Err(_)) => Ordering::Less, // numeric < alphanumeric
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn newer_versions_win() {
        assert!(is_newer("0.2.0", "0.1.1"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.2", "0.1.1"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn v_prefix_and_metadata_ignored() {
        assert_eq!(compare_versions("v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.3+build", "1.2.3"), Ordering::Equal);
    }

    #[test]
    fn prerelease_sorts_below_release() {
        assert!(is_newer("0.2.0", "0.2.0-pre.1"));
        assert!(is_newer("0.2.0-pre.2", "0.2.0-pre.1"));
        assert!(!is_newer("0.2.0-pre.1", "0.2.0"));
    }

    #[test]
    fn version_validation() {
        assert!(valid_version("0.1.1"));
        assert!(valid_version("1.2.3-pre.1"));
        assert!(!valid_version("")); // empty
        assert!(!valid_version("v1.2.3")); // must be pre-stripped
        assert!(!valid_version("1.2.3\x1b[31m")); // control chars rejected
    }
}

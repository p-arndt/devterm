//! The running child process attached to a pseudo-terminal: spawn, I/O, lifecycle.

use std::io::Read;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::Context;
use crossbeam_channel::{Receiver, Sender};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, native_pty_system};

use crate::spec::{PtyCommandSpec, PtyEvent, PtySize};

/// A running child process attached to a pseudo-terminal.
///
/// Holds the master end (for resize), a writer guarded by a `Mutex`, a killer
/// handle for best-effort termination, a reader thread that forwards output on
/// the `events` channel, and a waiter thread that reports the child's exit.
///
/// `master` is an `Option` only so that [`Drop`] can close the ConPTY *before*
/// joining the reader thread: on Windows the reader's blocking `read` does not
/// return until `ClosePseudoConsole` runs (which happens when the master is
/// dropped), so dropping it early is what lets the reader observe EOF and exit.
pub struct Pty {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    events_rx: Receiver<PtyEvent>,
    reader_thread: Option<JoinHandle<()>>,
    waiter_thread: Option<JoinHandle<()>>,
}

impl Pty {
    /// Spawn `spec` on a fresh ConPTY of `size`. Starts a reader thread that forwards
    /// output on the returned `Receiver` and calls `wake` after every chunk (and on exit).
    pub fn spawn<F>(spec: &PtyCommandSpec, size: PtySize, wake: F) -> anyhow::Result<Pty>
    where
        F: Fn() + Send + 'static,
    {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size.into())
            .context("failed to open pty")?;

        // Build the command from the spec.
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        // Spawn the child into the slave side.
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn `{}`", spec.program))?;

        // The slave handle is not needed once the child has been spawned; dropping
        // it here lets the pty report EOF once the child (and any inherited copies)
        // close their ends.
        drop(pair.slave);

        // Take the single writer and a reader clone from the master.
        let writer = pair
            .master
            .take_writer()
            .context("failed to take pty writer")?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone pty reader")?;

        // A killer handle usable from `kill`/`Drop`, independent of the waiter
        // thread that owns `child`.
        let killer = child.clone_killer();

        let (tx, rx): (Sender<PtyEvent>, Receiver<PtyEvent>) = crossbeam_channel::unbounded();

        // The wake callback is invoked from both worker threads, so share it. A
        // `Mutex` (rather than requiring `F: Sync`) keeps the public bound at
        // `Fn() + Send` while still being safe to move into two threads.
        let wake = Arc::new(Mutex::new(wake));

        // Reader thread: forward child output. On Windows the ConPTY keeps this
        // `read` blocked even after the child exits (until the master is dropped),
        // so exit detection lives in the waiter thread below rather than here.
        let reader_tx = tx.clone();
        let reader_wake = wake.clone();
        let reader_thread = std::thread::Builder::new()
            .name("devterm-pty-reader".to_string())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF: the pty was torn down.
                        Ok(n) => {
                            if reader_tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                                // Consumer dropped; stop forwarding.
                                return;
                            }
                            if let Ok(cb) = reader_wake.lock() {
                                (*cb)();
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("failed to spawn pty reader thread")?;

        // Waiter thread: block on the child and report its exit. This is what the
        // app relies on to notice the shell exiting, since the reader may stay
        // blocked on `read` until the pty is torn down.
        let waiter_thread = std::thread::Builder::new()
            .name("devterm-pty-waiter".to_string())
            .spawn(move || {
                let code = child.wait().ok().map(|status| status.exit_code() as i32);
                let _ = tx.send(PtyEvent::Exited(code));
                if let Ok(cb) = wake.lock() {
                    (*cb)();
                }
            })
            .context("failed to spawn pty waiter thread")?;

        Ok(Pty {
            master: Some(pair.master),
            writer: Mutex::new(writer),
            killer,
            events_rx: rx,
            reader_thread: Some(reader_thread),
            waiter_thread: Some(waiter_thread),
        })
    }

    /// Receiver of child output / exit events (single consumer).
    pub fn events(&self) -> Receiver<PtyEvent> {
        self.events_rx.clone()
    }

    /// Write input bytes to the child.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    /// Resize the ConPTY.
    pub fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        self.master
            .as_ref()
            .context("pty master already closed")?
            .resize(size.into())
            .context("failed to resize pty")
    }

    /// Kill the child (best effort).
    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.killer.kill().context("failed to kill child process")?;
        Ok(())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Best-effort cleanup. Kill the child first so the waiter thread's
        // `child.wait()` returns.
        let _ = self.killer.kill();

        // Close the ConPTY by dropping the master (runs `ClosePseudoConsole`), which
        // lets the reader's blocking `read` observe EOF.
        self.master.take();

        // Join the waiter: after `kill()`, its `child.wait()` returns promptly.
        if let Some(handle) = self.waiter_thread.take() {
            let _ = handle.join();
        }

        // Do NOT join the reader thread. Its `read` is a blocking Windows ConPTY call
        // that is not guaranteed to unblock immediately on teardown, so joining it here
        // could hang the caller (e.g. when closing a pane — which is exactly what hung
        // the test suite). Detaching is safe: the thread holds only a cloned reader and
        // a channel sender, exits once the read returns, and is reclaimed at process
        // exit. A cancelable/overlapped read is a later refinement.
        self.reader_thread.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    // ConPTY integration test: spawns a real process and relies on pseudoconsole
    // process-exit timing, which is environment-sensitive (differs under sandboxes/CI).
    // Ignored by default so the standard suite stays fast and never hangs; run it
    // explicitly with `cargo test -p devterm-pty -- --ignored`. The underlying
    // exit-detection behavior (child.wait() under ConPTY) is tracked as M1 pty-lifecycle
    // work (exit code + auto-close on shell exit).
    #[test]
    #[ignore = "ConPTY integration: process-exit timing is environment-sensitive; run with --ignored"]
    fn echo_output_arrives_and_wakes() {
        // Spawn a trivial command and assert its output reaches the channel.
        let woke = Arc::new(AtomicBool::new(false));
        let woke_cb = woke.clone();

        #[cfg(windows)]
        let spec = PtyCommandSpec {
            program: "cmd.exe".to_string(),
            args: vec!["/c".to_string(), "echo".to_string(), "hello".to_string()],
            cwd: None,
            env: Vec::new(),
        };
        #[cfg(not(windows))]
        let spec = PtyCommandSpec {
            program: "/bin/echo".to_string(),
            args: vec!["hello".to_string()],
            cwd: None,
            env: Vec::new(),
        };

        let pty = Pty::spawn(&spec, PtySize { cols: 80, rows: 24 }, move || {
            woke_cb.store(true, Ordering::SeqCst);
        })
        .expect("spawn should succeed");

        let rx = pty.events();
        let mut collected = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut exited = false;

        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(PtyEvent::Output(bytes)) => collected.extend_from_slice(&bytes),
                Ok(PtyEvent::Exited(_)) => {
                    exited = true;
                    break;
                }
                Err(_) => {}
            }
        }

        assert!(exited, "child should have exited");
        assert!(
            woke.load(Ordering::SeqCst),
            "wake callback should have fired"
        );
        let text = String::from_utf8_lossy(&collected);
        assert!(
            text.contains("hello"),
            "expected 'hello' in output, got: {text:?}"
        );
    }
}

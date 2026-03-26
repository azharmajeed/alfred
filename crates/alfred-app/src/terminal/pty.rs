//! PTY spawning and I/O tasks.
//!
//! `run_pty` is spawned as a tokio task from `WorkspaceManager`.
//! It:
//!   1. Spawns a shell via `portable-pty` (ConPTY on Windows, Unix PTY on Linux).
//!   2. Launches a blocking reader task that sends `UserEvent::PtyOutput` to winit.
//!   3. Drives a writer loop that forwards keyboard bytes to the PTY master.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::mpsc::UnboundedReceiver;
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::terminal::emulator::TerminalState;
use crate::workspace::pane::PaneId;

pub async fn run_pty(
    pane_id: PaneId,
    cols: u16,
    rows: u16,
    mut writer_rx: UnboundedReceiver<Vec<u8>>,
    terminal: Arc<Mutex<TerminalState>>,
    dirty: Arc<AtomicBool>,
    proxy: EventLoopProxy<UserEvent>,
) {
    // ── Open PTY pair ──────────────────────────────────────────────────────
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            log::error!("[pane {pane_id}] Failed to open PTY: {e}");
            return;
        }
    };

    // ── Spawn shell ────────────────────────────────────────────────────────
    let shell = crate::platform::default_shell();
    log::info!("[pane {pane_id}] Spawning shell: {}", shell.display());

    let mut cmd = CommandBuilder::new(&shell);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let _child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            log::error!("[pane {pane_id}] Failed to spawn shell: {e}");
            return;
        }
    };

    drop(pair.slave);

    // ── PTY reader — blocking task ─────────────────────────────────────────
    let master_reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            log::error!("[pane {pane_id}] Failed to clone PTY reader: {e}");
            return;
        }
    };

    let proxy_reader = proxy.clone();

    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut reader = master_reader;
        let mut buf = [0u8; 8192];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = buf[..n].to_vec();
                    if proxy_reader
                        .send_event(UserEvent::PtyOutput { pane_id, bytes })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    log::debug!("[pane {pane_id}] PTY read ended: {e}");
                    break;
                }
            }
        }

        log::info!("[pane {pane_id}] PTY reader finished — sending PtyExited");
        let _ = proxy_reader.send_event(UserEvent::PtyExited { pane_id });
    });

    // ── PTY writer — async loop ────────────────────────────────────────────
    let mut master_writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            log::error!("[pane {pane_id}] Failed to take PTY writer: {e}");
            return;
        }
    };

    while let Some(bytes) = writer_rx.recv().await {
        if let Err(e) = {
            use std::io::Write;
            master_writer.write_all(&bytes)
        } {
            log::debug!("[pane {pane_id}] PTY write error: {e}");
            break;
        }
    }

    log::debug!("[pane {pane_id}] PTY writer task ended");
    let _ = dirty;
    let _ = terminal;
}

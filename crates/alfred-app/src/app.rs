use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::renderer::Renderer;
use crate::terminal::emulator::TerminalState;

// ── User events sent from background tasks → winit event loop ────────────────

#[derive(Clone, Debug)]
pub enum UserEvent {
    /// Raw bytes read from the PTY master.
    PtyOutput(Vec<u8>),
    /// PTY process exited — close the window.
    PtyExited,
}

// ── Inner application state (exists after the first `resumed` call) ──────────

struct AppInner {
    window: Arc<Window>,
    renderer: Renderer,
    terminal: Arc<Mutex<TerminalState>>,
    dirty: Arc<AtomicBool>,
    /// Send bytes to the PTY writer task.
    pty_writer: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    /// Keep the runtime alive for the lifetime of the app.
    _rt: tokio::runtime::Runtime,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    inner: Option<AppInner>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self { proxy, inner: None }
    }

    fn cell_dims() -> (f32, f32) {
        (9.0, 18.0) // cell_width, cell_height in pixels
    }
}

// ── winit ApplicationHandler impl ────────────────────────────────────────────

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return; // already initialised (e.g. Android resume)
        }

        // Wait for events rather than spinning — essential for an idle terminal.
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);

        // ── Create window ──────────────────────────────────────────────────
        let attrs = Window::default_attributes()
            .with_title("Alfred")
            .with_inner_size(winit::dpi::LogicalSize::new(1200u32, 800u32));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;

        let (cw, ch) = Self::cell_dims();
        let cols = (size.width as f32 / (cw * scale)).floor().max(1.0) as u16;
        let rows = (size.height as f32 / (ch * scale)).floor().max(1.0) as u16;

        // ── Tokio runtime for PTY I/O ──────────────────────────────────────
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");

        // Channel shared by keyboard input AND terminal PtyWrite responses.
        let (pty_tx, pty_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        // ── Terminal state (shared between winit thread and tokio tasks) ───
        // pty_tx clone given to EventProxy so VT responses (e.g. DSR cursor
        // position reply) are forwarded back to the shell automatically.
        let terminal = Arc::new(Mutex::new(TerminalState::new(cols, rows, pty_tx.clone())));
        let dirty = Arc::new(AtomicBool::new(true));

        let term_clone = terminal.clone();
        let dirty_clone = dirty.clone();
        let proxy_clone = self.proxy.clone();

        rt.spawn(async move {
            crate::terminal::pty::run_pty(
                cols,
                rows,
                pty_rx,
                term_clone,
                dirty_clone,
                proxy_clone,
            )
            .await;
        });

        // ── Renderer (blocks until GPU is ready) ──────────────────────────
        let renderer =
            pollster::block_on(Renderer::new(window.clone())).expect("renderer init");

        self.inner = Some(AppInner {
            window,
            renderer,
            terminal,
            dirty,
            pty_writer: pty_tx,
            _rt: rt,
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput(bytes) => {
                let Some(inner) = &mut self.inner else { return };
                {
                    let mut term = inner.terminal.lock().unwrap();
                    term.process_bytes(&bytes);
                }
                inner.dirty.store(true, Ordering::Release);
                inner.window.request_redraw();
            }
            UserEvent::PtyExited => {
                log::info!("PTY exited — closing.");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(inner) = &mut self.inner else { return };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    inner.renderer.resize(new_size);
                    let scale = inner.window.scale_factor() as f32;
                    let (cw, ch) = Self::cell_dims();
                    let cols = (new_size.width as f32 / (cw * scale)).floor().max(1.0) as u16;
                    let rows = (new_size.height as f32 / (ch * scale)).floor().max(1.0) as u16;
                    let mut term = inner.terminal.lock().unwrap();
                    term.resize(cols, rows);
                }
                inner.window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                if inner.dirty.swap(false, Ordering::AcqRel) {
                    let cells = inner.terminal.lock().unwrap().collect_cells();
                    let cursor = inner.terminal.lock().unwrap().cursor_pos();
                    match inner.renderer.render(&cells, cursor) {
                        Ok(()) => {}
                        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                            inner.renderer.reconfigure();
                            inner.window.request_redraw();
                        }
                        Err(wgpu::SurfaceError::Timeout) => {
                            log::warn!("Surface timeout — skipping frame");
                        }
                        Err(e) => {
                            log::error!("Render error: {:?}", e);
                        }
                    }
                }
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                is_synthetic: false,
                ..
            } => {
                let bytes = key_to_bytes(&logical_key);
                if !bytes.is_empty() {
                    let _ = inner.pty_writer.send(bytes);
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // If something was marked dirty from a non-winit thread, request a redraw.
        if let Some(inner) = &self.inner {
            if inner.dirty.load(Ordering::Acquire) {
                inner.window.request_redraw();
            }
        }
    }
}

// ── Key translation ───────────────────────────────────────────────────────────

fn key_to_bytes(key: &Key) -> Vec<u8> {
    match key {
        Key::Character(s) => s.as_str().as_bytes().to_vec(),
        Key::Named(NamedKey::Enter) => b"\r".to_vec(),
        Key::Named(NamedKey::Backspace) => b"\x7f".to_vec(),
        Key::Named(NamedKey::Escape) => b"\x1b".to_vec(),
        Key::Named(NamedKey::Tab) => b"\t".to_vec(),
        Key::Named(NamedKey::ArrowUp) => b"\x1b[A".to_vec(),
        Key::Named(NamedKey::ArrowDown) => b"\x1b[B".to_vec(),
        Key::Named(NamedKey::ArrowRight) => b"\x1b[C".to_vec(),
        Key::Named(NamedKey::ArrowLeft) => b"\x1b[D".to_vec(),
        Key::Named(NamedKey::Home) => b"\x1b[H".to_vec(),
        Key::Named(NamedKey::End) => b"\x1b[F".to_vec(),
        Key::Named(NamedKey::Delete) => b"\x1b[3~".to_vec(),
        Key::Named(NamedKey::PageUp) => b"\x1b[5~".to_vec(),
        Key::Named(NamedKey::PageDown) => b"\x1b[6~".to_vec(),
        Key::Named(NamedKey::F1) => b"\x1bOP".to_vec(),
        Key::Named(NamedKey::F2) => b"\x1bOQ".to_vec(),
        Key::Named(NamedKey::F3) => b"\x1bOR".to_vec(),
        Key::Named(NamedKey::F4) => b"\x1bOS".to_vec(),
        Key::Named(NamedKey::F5) => b"\x1b[15~".to_vec(),
        Key::Named(NamedKey::F6) => b"\x1b[17~".to_vec(),
        Key::Named(NamedKey::F7) => b"\x1b[18~".to_vec(),
        Key::Named(NamedKey::F8) => b"\x1b[19~".to_vec(),
        Key::Named(NamedKey::F9) => b"\x1b[20~".to_vec(),
        Key::Named(NamedKey::F10) => b"\x1b[21~".to_vec(),
        Key::Named(NamedKey::F11) => b"\x1b[23~".to_vec(),
        Key::Named(NamedKey::F12) => b"\x1b[24~".to_vec(),
        _ => vec![],
    }
}

// Allow unused PhysicalSize import
#[allow(dead_code)]
fn _check_size(_: PhysicalSize<u32>) {}

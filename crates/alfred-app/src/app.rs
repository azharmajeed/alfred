use std::sync::atomic::Ordering;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::renderer::{PaneView, Renderer};
use crate::workspace::layout::{PhysRect, SplitDir};
use crate::workspace::manager::WorkspaceManager;
use crate::workspace::pane::PaneId;

// ── User events sent from background tasks → winit event loop ────────────────

#[derive(Clone, Debug)]
pub enum UserEvent {
    /// Raw bytes read from a PTY master — tagged with the originating pane.
    PtyOutput { pane_id: PaneId, bytes: Vec<u8> },
    /// A PTY process exited.
    PtyExited { pane_id: PaneId },
}

// ── Inner application state (exists after `resumed`) ─────────────────────────

struct AppInner {
    window: Arc<Window>,
    renderer: Renderer,
    workspace: WorkspaceManager,
    proxy: EventLoopProxy<UserEvent>,
    modifiers: ModifiersState,
    /// Tokio runtime — kept alive for the lifetime of the app.
    _rt: Arc<tokio::runtime::Runtime>,
}

impl AppInner {
    fn phys_rect(&self) -> PhysRect {
        let s = self.window.inner_size();
        PhysRect { x: 0, y: 0, w: s.width, h: s.height }
    }
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
}

// ── winit ApplicationHandler impl ────────────────────────────────────────────

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }

        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);

        // ── Window ────────────────────────────────────────────────────────
        let attrs = Window::default_attributes()
            .with_title("Alfred")
            .with_inner_size(winit::dpi::LogicalSize::new(1200u32, 800u32));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let phys = window.inner_size();
        let scale = window.scale_factor() as f32;

        // ── Tokio runtime ─────────────────────────────────────────────────
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime"),
        );

        // Initial terminal size (logical pixels → cells).
        let (cw, ch) = (9.0f32, 18.0f32);
        let cols = (phys.width as f32 / (cw * scale)).floor().max(1.0) as u16;
        let rows = (phys.height as f32 / (ch * scale)).floor().max(1.0) as u16;

        // ── Workspace manager (spawns first PTY) ──────────────────────────
        let workspace = WorkspaceManager::new(
            rt.clone(),
            cols,
            rows,
            self.proxy.clone(),
            scale,
        );

        // ── Renderer ──────────────────────────────────────────────────────
        let renderer =
            pollster::block_on(Renderer::new(window.clone())).expect("renderer init");

        self.inner = Some(AppInner {
            window,
            renderer,
            workspace,
            proxy: self.proxy.clone(),
            modifiers: ModifiersState::empty(),
            _rt: rt,
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(inner) = &mut self.inner else { return };

        match event {
            UserEvent::PtyOutput { pane_id, bytes } => {
                let ws = inner.workspace.active_workspace();
                if let Some(pane) = ws.panes.get(&pane_id) {
                    pane.terminal.lock().unwrap().process_bytes(&bytes);
                    pane.dirty.store(true, Ordering::Release);
                    inner.window.request_redraw();
                }
            }
            UserEvent::PtyExited { pane_id } => {
                log::info!("PTY {pane_id} exited");
                let should_close = inner.workspace.remove_pane(pane_id);
                if should_close {
                    event_loop.exit();
                } else {
                    inner.window.request_redraw();
                }
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
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::ModifiersChanged(m) => {
                inner.modifiers = m.state();
            }

            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    inner.renderer.resize(new_size);
                    let rect = inner.phys_rect();
                    inner.workspace.resize_all(rect);
                }
                inner.window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                let ws = inner.workspace.active_workspace();
                let rect = inner.phys_rect();
                let layout = ws.tree.layout(rect);

                // Collect cell data only for panes that have new output.
                // `swap(false)` atomically reads the dirty flag and clears it.
                struct FramePane {
                    id: u32,
                    cells: Vec<crate::terminal::emulator::TermCell>,
                    cursor: (u16, u16),
                    rect: PhysRect,
                    is_active: bool,
                    needs_reshape: bool,
                }

                let frame_data: Vec<FramePane> = layout
                    .iter()
                    .filter_map(|(pane_id, pane_rect)| {
                        ws.panes.get(pane_id).map(|pane| {
                            let was_dirty = pane.dirty.swap(false, Ordering::AcqRel);
                            let (cells, cursor) = if was_dirty {
                                // Single lock — collect cells and cursor in one pass.
                                pane.terminal.lock().unwrap().collect_frame()
                            } else {
                                (vec![], (0, 0))
                            };
                            FramePane {
                                id: *pane_id,
                                cells,
                                cursor,
                                rect: *pane_rect,
                                is_active: *pane_id == ws.active_pane,
                                needs_reshape: was_dirty,
                            }
                        })
                    })
                    .collect();

                let views: Vec<PaneView<'_>> = frame_data
                    .iter()
                    .map(|p| PaneView {
                        id: p.id,
                        cells: &p.cells,
                        cursor: p.cursor,
                        rect: p.rect,
                        is_active: p.is_active,
                        needs_reshape: p.needs_reshape,
                    })
                    .collect();

                match inner.renderer.render(&views) {
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

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        physical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                is_synthetic: false,
                ..
            } => {
                let ctrl = inner.modifiers.control_key();
                let shift = inner.modifiers.shift_key();

                // ── Multiplexer shortcuts (Ctrl+Shift+…) ─────────────────
                if ctrl && shift {
                    if let PhysicalKey::Code(code) = physical_key {
                        let rect = inner.phys_rect();
                        match code {
                            // Ctrl+Shift+E — vertical split (left | right)
                            KeyCode::KeyE => {
                                inner.workspace.split_active_pane(
                                    SplitDir::Vertical,
                                    rect,
                                    inner.proxy.clone(),
                                );
                                inner.window.request_redraw();
                                return;
                            }
                            // Ctrl+Shift+O — horizontal split (top / bottom)
                            KeyCode::KeyO => {
                                inner.workspace.split_active_pane(
                                    SplitDir::Horizontal,
                                    rect,
                                    inner.proxy.clone(),
                                );
                                inner.window.request_redraw();
                                return;
                            }
                            // Ctrl+Shift+] — focus next pane
                            KeyCode::BracketRight => {
                                inner.workspace.focus_next_pane();
                                inner.window.request_redraw();
                                return;
                            }
                            // Ctrl+Shift+[ — focus previous pane
                            KeyCode::BracketLeft => {
                                inner.workspace.focus_prev_pane();
                                inner.window.request_redraw();
                                return;
                            }
                            // Ctrl+Shift+T — new workspace tab
                            KeyCode::KeyT => {
                                let phys = inner.window.inner_size();
                                let scale = inner.window.scale_factor() as f32;
                                let cols = (phys.width as f32 / (9.0 * scale)).floor().max(1.0) as u16;
                                let rows = (phys.height as f32 / (18.0 * scale)).floor().max(1.0) as u16;
                                let n = inner.workspace.workspaces.len() + 1;
                                inner.workspace.new_workspace(
                                    &format!("Workspace {n}"),
                                    cols,
                                    rows,
                                    inner.proxy.clone(),
                                );
                                inner.workspace.active = inner.workspace.workspaces.len() - 1;
                                inner.window.request_redraw();
                                return;
                            }
                            // Ctrl+Shift+W — close active pane
                            KeyCode::KeyW => {
                                let active_pane =
                                    inner.workspace.active_workspace().active_pane;
                                let should_close = inner.workspace.remove_pane(active_pane);
                                if should_close {
                                    event_loop.exit();
                                } else {
                                    inner.window.request_redraw();
                                }
                                return;
                            }
                            // Ctrl+Shift+Tab — previous workspace
                            KeyCode::Tab => {
                                inner.workspace.prev_workspace();
                                inner.window.request_redraw();
                                return;
                            }
                            _ => {}
                        }
                    }
                }

                // Ctrl+Tab — next workspace
                if ctrl && !shift {
                    if let PhysicalKey::Code(KeyCode::Tab) = physical_key {
                        inner.workspace.next_workspace();
                        inner.window.request_redraw();
                        return;
                    }
                }

                // ── Default: send keystroke to the active pane's PTY ──────
                let bytes = key_to_bytes(&logical_key);
                if !bytes.is_empty() {
                    let ws = inner.workspace.active_workspace();
                    if let Some(pane) = ws.panes.get(&ws.active_pane) {
                        let _ = pane.pty_tx.send(bytes);
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // Positive delta → scroll up (view older history).
                let lines: i32 = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as i32,
                    MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as i32,
                };
                if lines != 0 {
                    let ws = inner.workspace.active_workspace();
                    if let Some(pane) = ws.panes.get(&ws.active_pane) {
                        pane.terminal.lock().unwrap().scroll_display(lines);
                        pane.dirty.store(true, Ordering::Release);
                        inner.window.request_redraw();
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(inner) = &self.inner {
            if inner.workspace.any_dirty() {
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

// Suppress unused import warning from winit::dpi::PhysicalSize.
#[allow(dead_code)]
fn _use_physical_size(_: PhysicalSize<u32>) {}

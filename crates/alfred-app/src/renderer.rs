use std::collections::HashMap;
use std::sync::Arc;

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::MultisampleState;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::terminal::emulator::TermCell;
use crate::workspace::layout::PhysRect;
use crate::workspace::pane::PaneId;

const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 18.0;
const PADDING: f32 = 4.0;

const TERM_FONT_FAMILY: Family<'static> = Family::Monospace;

/// Data for one pane passed to `Renderer::render` each frame.
pub struct PaneView<'a> {
    pub id: PaneId,
    pub cells: &'a [TermCell],
    pub cursor: (u16, u16),
    /// Physical-pixel rect within the window.
    pub rect: PhysRect,
    pub is_active: bool,
    /// When false the pane's Buffer is already up-to-date — skip reshaping.
    pub needs_reshape: bool,
}

pub struct Renderer {
    _window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,

    /// One `Buffer` per pane — persisted across frames so glyphon reuses
    /// the glyph atlas rather than re-rasterising glyphs every frame.
    pane_buffers: HashMap<PaneId, Buffer>,

    scale_factor: f32,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let scale_factor = window.scale_factor() as f32;
        let size = window.inner_size();

        // ── wgpu instance ──────────────────────────────────────────────────
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: crate::platform::gpu_backends(),
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone())?;

        // ── Adapter ────────────────────────────────────────────────────────
        let candidates = instance.enumerate_adapters(crate::platform::gpu_backends());
        for a in &candidates {
            let info = a.get_info();
            log::debug!("GPU candidate: {} ({:?})", info.name, info.device_type);
        }

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow::anyhow!("no GPU adapter found"))?;

        log::info!("Using GPU: {}", adapter.get_info().name);

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Alfred GPU Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await?;

        // ── Surface config ──────────────────────────────────────────────────
        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // ── glyphon setup ───────────────────────────────────────────────────
        let mut font_system = FontSystem::new();

        #[cfg(target_os = "windows")]
        {
            let candidates = [
                r"C:\Windows\Fonts\CascadiaCode.ttf",
                r"C:\Windows\Fonts\CascadiaCodePL.ttf",
                r"C:\Windows\Fonts\CascadiaMono.ttf",
                r"C:\Windows\Fonts\CascadiaMonoPL.ttf",
                r"C:\Windows\Fonts\consola.ttf",
                r"C:\Windows\Fonts\consolab.ttf",
                r"C:\Windows\Fonts\consolai.ttf",
                r"C:\Windows\Fonts\consolaz.ttf",
                r"C:\Windows\Fonts\lucon.ttf",
            ];
            let mut loaded_name: Option<&str> = None;
            for path in &candidates {
                if let Ok(bytes) = std::fs::read(path) {
                    font_system.db_mut().load_font_data(bytes);
                    log::debug!("Loaded font: {path}");
                    if loaded_name.is_none() {
                        loaded_name = Some(match *path {
                            p if p.contains("Cascadia") => "Cascadia Code",
                            p if p.contains("consola") => "Consolas",
                            _ => "Lucida Console",
                        });
                    }
                }
            }
            let mono = loaded_name.unwrap_or("Consolas");
            font_system.db_mut().set_monospace_family(mono);
            log::info!("Monospace family set to: {mono}");
        }

        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, surface_format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        Ok(Self {
            _window: window,
            surface,
            device,
            queue,
            config,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            pane_buffers: HashMap::new(),
            scale_factor,
        })
    }

    // ── Public interface ────────────────────────────────────────────────────

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn reconfigure(&mut self) {
        self.surface.configure(&self.device, &self.config);
    }

    /// Render all visible panes in one wgpu pass.
    pub fn render(&mut self, panes: &[PaneView<'_>]) -> Result<(), wgpu::SurfaceError> {
        let default_attrs = Attrs::new().family(TERM_FONT_FAMILY);
        let metrics = Metrics::new(FONT_SIZE * self.scale_factor, LINE_HEIGHT * self.scale_factor);

        // ── Pass 1: update / create a Buffer for each pane ─────────────────
        for pane in panes.iter() {
            // Ensure the buffer exists.
            if !self.pane_buffers.contains_key(&pane.id) {
                let buf = Buffer::new(&mut self.font_system, metrics);
                self.pane_buffers.insert(pane.id, buf);
            }

            // Always resize the buffer when the rect changes.
            let buf = self.pane_buffers.get_mut(&pane.id).unwrap();
            buf.set_size(
                &mut self.font_system,
                Some(pane.rect.w as f32 - PADDING * 2.0),
                Some(pane.rect.h as f32 - PADDING * 2.0),
            );

            // Skip reshaping if the pane's cells have not changed.
            if pane.needs_reshape {
                let rows = build_rows(pane.cells);
                let spans = build_spans(&rows, pane.cursor, pane.is_active);
                buf.set_rich_text(
                    &mut self.font_system,
                    spans.iter().map(|(s, a)| (s.as_str(), *a)),
                    default_attrs,
                    Shaping::Basic,
                );
                buf.shape_until_scroll(&mut self.font_system, false);
            }
        }

        // Drop stale buffers for panes no longer visible.
        let active_ids: std::collections::HashSet<PaneId> =
            panes.iter().map(|p| p.id).collect();
        self.pane_buffers.retain(|id, _| active_ids.contains(id));

        // ── Pass 2: build TextArea slice (immutable refs) ───────────────────
        let text_areas: Vec<TextArea<'_>> = panes
            .iter()
            .map(|pane| {
                let buf = &self.pane_buffers[&pane.id];
                let active_color = if pane.is_active {
                    Color::rgb(235, 219, 178) // Gruvbox fg
                } else {
                    Color::rgb(168, 153, 132) // Gruvbox gray — dim inactive panes
                };
                TextArea {
                    buffer: buf,
                    left: pane.rect.x as f32 + PADDING,
                    top: pane.rect.y as f32 + PADDING,
                    // Metrics are already in physical pixels (×scale_factor),
                    // so we must NOT re-apply scale here (would double-scale).
                    scale: 1.0,
                    bounds: TextBounds {
                        left: pane.rect.x as i32,
                        top: pane.rect.y as i32,
                        right: (pane.rect.x + pane.rect.w) as i32,
                        bottom: (pane.rect.y + pane.rect.h) as i32,
                    },
                    default_color: active_color,
                    custom_glyphs: &[],
                }
            })
            .collect();

        // ── glyphon prepare ─────────────────────────────────────────────────
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );

        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .expect("glyphon prepare");

        // ── wgpu render pass ────────────────────────────────────────────────
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Alfred Encoder"),
                });

        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Alfred Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Gruvbox hard-dark #1d2021 in linear space ≈ 0.010.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.010,
                            g: 0.010,
                            b: 0.010,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut rp)
                .expect("glyphon render");
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        Ok(())
    }
}

// ── Text building helpers ─────────────────────────────────────────────────────

type Row = Vec<TermCell>;

fn build_rows(cells: &[TermCell]) -> Vec<Row> {
    if cells.is_empty() {
        return vec![];
    }
    let max_row = cells.iter().map(|c| c.row).max().unwrap_or(0) as usize;
    let mut rows: Vec<Row> = vec![vec![]; max_row + 1];
    for cell in cells {
        rows[cell.row as usize].push(cell.clone());
    }
    for row in &mut rows {
        row.sort_by_key(|c| c.col);
    }
    rows
}

/// Build `(text, Attrs)` spans for `set_rich_text`.
///
/// Consecutive cells with the same foreground colour are merged into one span.
/// The cursor cell is replaced with `█` and rendered in Gruvbox foreground.
/// When `show_cursor` is false (inactive pane) the cursor is not rendered.
fn build_spans(
    rows: &[Row],
    cursor: (u16, u16),
    show_cursor: bool,
) -> Vec<(String, Attrs<'static>)> {
    let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
    let (cur_row, cur_col) = cursor;
    let default_attrs = Attrs::new().family(Family::Monospace);

    for (row_idx, row) in rows.iter().enumerate() {
        if row.is_empty() {
            spans.push(("\n".to_string(), default_attrs));
            continue;
        }

        let max_col = row.last().map(|c| c.col).unwrap_or(0) as usize;
        let mut row_chars: Vec<(char, [u8; 3])> =
            vec![(' ', [235u8, 219, 178]); max_col + 1];
        for cell in row {
            if (cell.col as usize) < row_chars.len() {
                row_chars[cell.col as usize] = (cell.ch, cell.fg);
            }
        }

        let mut i = 0;
        while i < row_chars.len() {
            let (ch, fg) = row_chars[i];
            let is_cursor =
                show_cursor && row_idx as u16 == cur_row && i as u16 == cur_col;

            let color = if is_cursor {
                Color::rgb(235, 219, 178)
            } else {
                Color::rgb(fg[0], fg[1], fg[2])
            };

            let mut run = String::new();
            if is_cursor {
                run.push('█');
            } else {
                run.push(ch);
            }

            let mut j = i + 1;
            if !is_cursor {
                while j < row_chars.len() {
                    let (nch, nfg) = row_chars[j];
                    let next_cursor =
                        show_cursor && row_idx as u16 == cur_row && j as u16 == cur_col;
                    if nfg == fg && !next_cursor {
                        run.push(nch);
                        j += 1;
                    } else {
                        break;
                    }
                }
            }

            spans.push((run, default_attrs.color(color)));
            i = j;
        }

        if row_idx + 1 < rows.len() {
            spans.push(("\n".to_string(), default_attrs));
        }
    }

    spans
}

use std::sync::Arc;

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::MultisampleState;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::terminal::emulator::TermCell;

const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 18.0;
const PADDING: f32 = 4.0;

pub struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffer: Buffer,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let size = window.inner_size();

        // ── wgpu instance ──────────────────────────────────────────────────
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: crate::platform::gpu_backends(),
            ..Default::default()
        });

        // SAFETY: surface lives as long as `window` which is Arc-owned and
        // lives at least as long as Renderer.
        let surface = instance.create_surface(window.clone())?;

        // ── Adapter — prefer AMD by name to work around iGPU misreport ────
        let adapter = {
            let candidates = instance
                .enumerate_adapters(crate::platform::gpu_backends())
                .collect::<Vec<_>>();

            // First try: AMD adapter by name
            let amd = candidates.iter().find(|a| {
                a.get_info().name.to_lowercase().contains("amd")
                    || a.get_info().name.to_lowercase().contains("radeon")
            });

            if let Some(a) = amd {
                // Can't clone Adapter; re-request it
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::None,
                        compatible_surface: Some(&surface),
                        force_fallback_adapter: false,
                    })
                    .await
                    .ok_or_else(|| anyhow::anyhow!("no AMD adapter"))?
            } else {
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::None,
                        compatible_surface: Some(&surface),
                        force_fallback_adapter: false,
                    })
                    .await
                    .ok_or_else(|| anyhow::anyhow!("no GPU adapter found"))?
            }
        };

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

        // ── Surface config ─────────────────────────────────────────────────
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

        // ── glyphon setup ──────────────────────────────────────────────────
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, surface_format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        let mut text_buffer = Buffer::new_empty(Metrics::new(FONT_SIZE, LINE_HEIGHT));
        // Pre-size to full window
        text_buffer.set_size(
            &mut font_system.clone_for_init(),
            Some(size.width as f32),
            Some(size.height as f32),
        );

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
        })
    }

    // ── Public interface ───────────────────────────────────────────────────

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

    pub fn render(
        &mut self,
        cells: &[TermCell],
        cursor: (u16, u16),
    ) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let w = self.config.width as f32;
        let h = self.config.height as f32;

        // ── Build coloured text from terminal cells ────────────────────────
        // Collect into rows, then build rich-text spans for glyphon.
        let rows = build_rows(cells);
        let spans = build_spans(&rows, cursor);

        self.text_buffer.set_size(
            &mut self.font_system,
            Some(w - PADDING * 2.0),
            Some(h - PADDING * 2.0),
        );

        // set_rich_text replaces all existing content
        self.text_buffer.set_rich_text(
            &mut self.font_system,
            spans.iter().map(|(s, a)| (s.as_str(), *a)),
            Attrs::new().family(Family::Monospace),
            Shaping::Basic,
        );
        self.text_buffer.shape_until_scroll(&mut self.font_system, false);

        // ── glyphon prepare ────────────────────────────────────────────────
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
                [TextArea {
                    buffer: &self.text_buffer,
                    left: PADDING,
                    top: PADDING,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: self.config.width as i32,
                        bottom: self.config.height as i32,
                    },
                    default_color: Color::rgb(204, 204, 204),
                    custom_glyphs: &[],
                }],
                &mut self.swash_cache,
            )
            .expect("glyphon prepare");

        // ── wgpu render pass ───────────────────────────────────────────────
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
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.071,
                            g: 0.071,
                            b: 0.071,
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

// ── Helper: build per-row strings from flat cell list ─────────────────────────

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
    // Sort each row by column
    for row in &mut rows {
        row.sort_by_key(|c| c.col);
    }
    rows
}

// ── Helper: build (String, Attrs) spans for glyphon's set_rich_text ──────────

fn build_spans(rows: &[Row], cursor: (u16, u16)) -> Vec<(String, Attrs<'static>)> {
    let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
    let (cur_row, cur_col) = cursor;

    for (row_idx, row) in rows.iter().enumerate() {
        // Collect chars; fill gaps with spaces
        let max_col = row.last().map(|c| c.col).unwrap_or(0) as usize;
        let mut row_chars: Vec<(char, [u8; 3], [u8; 3])> =
            vec![(' ', [204, 204, 204], [18, 18, 18]); max_col + 1];

        for cell in row {
            let idx = cell.col as usize;
            if idx < row_chars.len() {
                row_chars[idx] = (cell.ch, cell.fg, cell.bg);
            }
        }

        // Group consecutive cells with the same colour into one span
        let mut i = 0;
        while i < row_chars.len() {
            let (ch, fg, _bg) = row_chars[i];
            let is_cursor =
                row_idx as u16 == cur_row && i as u16 == cur_col;

            let color = if is_cursor {
                Color::rgb(18, 18, 18) // cursor: inverted fg
            } else {
                Color::rgb(fg[0], fg[1], fg[2])
            };

            // Extend run while same colour and not cursor boundary
            let mut run = String::new();
            run.push(ch);
            let mut j = i + 1;
            while j < row_chars.len() {
                let (nch, nfg, _) = row_chars[j];
                let next_is_cursor = row_idx as u16 == cur_row && j as u16 == cur_col;
                if nfg == fg && !is_cursor && !next_is_cursor {
                    run.push(nch);
                    j += 1;
                } else {
                    break;
                }
            }

            let attrs = Attrs::new().family(Family::Monospace).color(color);
            spans.push((run, attrs));
            i = j;
        }

        // Newline between rows (except last)
        if row_idx + 1 < rows.len() {
            spans.push(("\n".to_string(), Attrs::new().family(Family::Monospace)));
        }
    }

    spans
}

// ── Trait helpers to make FontSystem work in init context ─────────────────────

trait FontSystemExt {
    fn clone_for_init(&mut self) -> &mut FontSystem;
}

impl FontSystemExt for FontSystem {
    fn clone_for_init(&mut self) -> &mut FontSystem {
        self
    }
}

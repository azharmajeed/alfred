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

/// Font family used for terminal text.
const TERM_FONT_FAMILY: Family<'static> = Family::Monospace;

pub struct Renderer {
    /// Kept alive to ensure the surface remains valid (Arc ownership).
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

        // SAFETY: surface lives as long as window (Arc-owned).
        let surface = instance.create_surface(window.clone())?;

        // ── Adapter ─────────────────────────────────────────────────────
        // Log all candidates so the user can verify AMD iGPU is selected.
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
        let mut font_system = FontSystem::new();

        // Preload monospace fonts so Family::Monospace has good candidates.
        // Load ALL available — cosmic-text will pick the best one.
        #[cfg(target_os = "windows")]
        {
            let candidates = [
                r"C:\Windows\Fonts\CascadiaCode.ttf",
                r"C:\Windows\Fonts\CascadiaCodePL.ttf",
                r"C:\Windows\Fonts\CascadiaMono.ttf",
                r"C:\Windows\Fonts\CascadiaMonoPL.ttf",
                r"C:\Windows\Fonts\consola.ttf",     // Consolas regular
                r"C:\Windows\Fonts\consolab.ttf",    // Consolas bold
                r"C:\Windows\Fonts\consolai.ttf",    // Consolas italic
                r"C:\Windows\Fonts\consolaz.ttf",    // Consolas bold italic
                r"C:\Windows\Fonts\lucon.ttf",       // Lucida Console
            ];
            let mut loaded_name: Option<&str> = None;
            for path in &candidates {
                if let Ok(bytes) = std::fs::read(path) {
                    font_system.db_mut().load_font_data(bytes);
                    log::debug!("Loaded font: {path}");
                    if loaded_name.is_none() {
                        loaded_name = Some(match *path {
                            p if p.contains("Cascadia") => "Cascadia Code",
                            p if p.contains("consola")  => "Consolas",
                            _                            => "Lucida Console",
                        });
                    }
                }
            }

            // Tell fontdb which family to use for Family::Monospace.
            // Without this call, Family::Monospace resolves to nothing on Windows.
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

        // Create an empty buffer; size is set on first render.
        let text_buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));

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

        // ── Build text ─────────────────────────────────────────────────────
        let rows = build_rows(cells);
        let spans = build_spans(&rows, cursor);

        // Debug: log on first few renders so we can confirm data is flowing.
        static RENDER_COUNT: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(0);
        let rc = RENDER_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if rc < 5 {
            let non_space = cells.iter().filter(|c| c.ch != ' ').count();
            log::info!(
                "render #{rc}: {} total cells, {} non-space, {} spans",
                cells.len(), non_space, spans.len()
            );
        }

        self.text_buffer.set_size(
            &mut self.font_system,
            Some(w - PADDING * 2.0),
            Some(h - PADDING * 2.0),
        );

        self.text_buffer.set_rich_text(
            &mut self.font_system,
            spans.iter().map(|(s, a)| (s.as_str(), *a)),
            Attrs::new().family(TERM_FONT_FAMILY),
            Shaping::Basic,
        );
        self.text_buffer
            .shape_until_scroll(&mut self.font_system, false);

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
                    default_color: Color::rgb(235, 219, 178),
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
                        // wgpu clear color is LINEAR.  sRGB #1d2021 (Gruvbox
                        // hard dark) = 29/255 = 0.114 sRGB
                        // → linear ≈ (0.114/12.92) ≈ 0.0088.
                        // Using ~0.010 gives a near-black dark terminal bg.
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

// ── Helpers ───────────────────────────────────────────────────────────────────

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
/// Groups consecutive same-coloured cells into a single span so we minimise
/// the number of spans without sacrificing per-cell colour accuracy.
fn build_spans(rows: &[Row], cursor: (u16, u16)) -> Vec<(String, Attrs<'static>)> {
    let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
    let (cur_row, cur_col) = cursor;
    let default_attrs = Attrs::new().family(Family::Monospace);

    for (row_idx, row) in rows.iter().enumerate() {
        if row.is_empty() {
            spans.push(("\n".to_string(), default_attrs));
            continue;
        }

        // Fill a contiguous array of (char, fg_rgb) so gaps become spaces.
        let max_col = row.last().map(|c| c.col).unwrap_or(0) as usize;
        let mut row_chars: Vec<(char, [u8; 3])> =
            vec![(' ', [235u8, 219, 178]); max_col + 1];
        for cell in row {
            if (cell.col as usize) < row_chars.len() {
                row_chars[cell.col as usize] = (cell.ch, cell.fg);
            }
        }

        // Merge runs with the same colour into one span
        let mut i = 0;
        while i < row_chars.len() {
            let (ch, fg) = row_chars[i];
            let is_cursor = row_idx as u16 == cur_row && i as u16 == cur_col;

            let color = if is_cursor {
                // Inverted cursor block
                Color::rgb(40, 40, 40)
            } else {
                Color::rgb(fg[0], fg[1], fg[2])
            };

            let mut run = String::new();
            run.push(ch);
            let mut j = i + 1;
            if !is_cursor {
                while j < row_chars.len() {
                    let (nch, nfg) = row_chars[j];
                    let next_cursor = row_idx as u16 == cur_row && j as u16 == cur_col;
                    if nfg == fg && !next_cursor {
                        run.push(nch);
                        j += 1;
                    } else {
                        break;
                    }
                }
            }

            let attrs = default_attrs.color(color);
            spans.push((run, attrs));
            i = j;
        }

        if row_idx + 1 < rows.len() {
            spans.push(("\n".to_string(), default_attrs));
        }
    }

    spans
}

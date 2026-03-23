//! Wrapper around `alacritty_terminal` exposing a simple interface.
//!
//! alacritty_terminal v0.24 API notes:
//!   - `Term::new(config, &(rows, cols), listener)` — (usize,usize) implements Dimensions
//!   - `term.renderable_content()` — returns RenderableContent with display_iter + cursor
//!   - `Indexed<Cell>` — .point.line / .point.column / .cell.c / .cell.fg / .cell.bg
//!   - `term.resize(size)` — size implements Dimensions
//!   - Colours live in `vte::ansi::Color` re-exported as `alacritty_terminal::vte::ansi::Color`

use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::{test::TermSize, Config, Term};
use alacritty_terminal::vte::ansi::Processor;

// ── No-op event listener ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventProxy;

impl EventListener for EventProxy {
    fn send_event(&self, _event: alacritty_terminal::event::Event) {}
}

// ── Public cell type passed to the renderer ───────────────────────────────────

#[derive(Clone, Debug)]
pub struct TermCell {
    pub row: u16,
    pub col: u16,
    pub ch: char,
    pub fg: [u8; 3],
    /// Background colour — used for cell-background rendering in Phase 2.
    #[allow(dead_code)]
    pub bg: [u8; 3],
}

// ── Terminal state ────────────────────────────────────────────────────────────

pub struct TerminalState {
    term: Term<EventProxy>,
    processor: Processor,
    cols: u16,
    rows: u16,
}

impl TerminalState {
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TermSize { columns: cols as usize, screen_lines: rows as usize };
        let term = Term::new(Config::default(), &size, EventProxy);
        let processor = Processor::new();
        Self { term, processor, cols, rows }
    }

    /// Feed raw PTY bytes into the VT parser → terminal grid.
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.processor.advance(&mut self.term, b);
        }
    }

    /// Resize the terminal grid.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.term.resize(TermSize { columns: cols as usize, screen_lines: rows as usize });
    }

    /// Current cursor position as `(row, col)`.
    pub fn cursor_pos(&self) -> (u16, u16) {
        let content = self.term.renderable_content();
        let p = content.cursor.point;
        (p.line.0 as u16, p.column.0 as u16)
    }

    /// Collect all visible cells for rendering.
    pub fn collect_cells(&self) -> Vec<TermCell> {
        let content = self.term.renderable_content();
        let mut cells =
            Vec::with_capacity((self.cols as usize) * (self.rows as usize));

        for indexed in content.display_iter {
            cells.push(TermCell {
                row: indexed.point.line.0 as u16,
                col: indexed.point.column.0 as u16,
                ch: indexed.cell.c,
                fg: color_to_rgb(&indexed.cell.fg),
                bg: color_to_rgb(&indexed.cell.bg),
            });
        }

        cells
    }
}

// ── Colour helpers ────────────────────────────────────────────────────────────

fn color_to_rgb(color: &alacritty_terminal::vte::ansi::Color) -> [u8; 3] {
    use alacritty_terminal::vte::ansi::Color;
    match color {
        Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
        Color::Named(name) => named_to_rgb(*name),
        Color::Indexed(idx) => indexed_to_rgb(*idx),
    }
}

fn named_to_rgb(name: alacritty_terminal::vte::ansi::NamedColor) -> [u8; 3] {
    use alacritty_terminal::vte::ansi::NamedColor::*;
    // Gruvbox-inspired defaults
    match name {
        Black | DimBlack => [40, 40, 40],
        Red | DimRed => [204, 36, 29],
        Green | DimGreen => [152, 151, 26],
        Yellow | DimYellow => [215, 153, 33],
        Blue | DimBlue => [69, 133, 136],
        Magenta | DimMagenta => [177, 98, 134],
        Cyan | DimCyan => [104, 157, 106],
        White | DimWhite => [168, 153, 132],
        BrightBlack => [146, 131, 116],
        BrightRed => [251, 73, 52],
        BrightGreen => [184, 187, 38],
        BrightYellow => [250, 189, 47],
        BrightBlue => [131, 165, 152],
        BrightMagenta => [211, 134, 155],
        BrightCyan => [142, 192, 124],
        BrightWhite => [235, 219, 178],
        Foreground | DimForeground | BrightForeground => [235, 219, 178],
        Background => [40, 40, 40],
        Cursor => [235, 219, 178],
    }
}

fn indexed_to_rgb(idx: u8) -> [u8; 3] {
    match idx {
        0 => [40, 40, 40],
        1 => [204, 36, 29],
        2 => [152, 151, 26],
        3 => [215, 153, 33],
        4 => [69, 133, 136],
        5 => [177, 98, 134],
        6 => [104, 157, 106],
        7 => [168, 153, 132],
        8 => [146, 131, 116],
        9 => [251, 73, 52],
        10 => [184, 187, 38],
        11 => [250, 189, 47],
        12 => [131, 165, 152],
        13 => [211, 134, 155],
        14 => [142, 192, 124],
        15 => [235, 219, 178],
        16..=231 => {
            let n = idx - 16;
            let b = n % 6;
            let g = (n / 6) % 6;
            let r = n / 36;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            [scale(r), scale(g), scale(b)]
        }
        232..=255 => {
            let v = 8 + (idx - 232) * 10;
            [v, v, v]
        }
    }
}

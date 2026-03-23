//! Wrapper around `alacritty_terminal` that exposes a simple interface to the
//! rest of Alfred.
//!
//! API note (alacritty_terminal 0.24):
//!   - `Term::new(config, &size, listener)` — creates the terminal grid
//!   - `alacritty_terminal::ansi::Processor::new()` — VT parser
//!   - `processor.advance(&mut term, byte)` — feed one byte
//!   - `term.grid().display_iter()` — iterate visible cells
//!
//! If any import path is wrong for the exact version on crates.io, rustc will
//! show clear "not found" errors. The logic does not change.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::{Config, SizeInfo, Term};
use alacritty_terminal::vte::ansi::Processor;

// ── Event listener (no-op for Phase 1) ───────────────────────────────────────

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
    /// Foreground RGB.
    pub fg: [u8; 3],
    /// Background RGB.
    pub bg: [u8; 3],
}

// ── TerminalState ─────────────────────────────────────────────────────────────

pub struct TerminalState {
    term: Term<EventProxy>,
    processor: Processor,
    cols: u16,
    rows: u16,
}

impl TerminalState {
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = make_size(cols, rows);
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

    /// Resize the terminal grid to match new window dimensions.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.term.resize(make_size(cols, rows));
    }

    /// Current cursor position as (row, col).
    pub fn cursor_pos(&self) -> (u16, u16) {
        let p = self.term.grid().cursor.point;
        (p.line.0 as u16, p.column.0 as u16)
    }

    /// Collect all visible cells for rendering.
    pub fn collect_cells(&self) -> Vec<TermCell> {
        use alacritty_terminal::grid::Indexed;

        let mut cells = Vec::with_capacity((self.cols as usize) * (self.rows as usize));

        for Indexed { point, cell } in self.term.grid().display_iter() {
            cells.push(TermCell {
                row: point.line.0 as u16,
                col: point.column.0 as u16,
                ch: cell.c,
                fg: color_to_rgb(&cell.fg),
                bg: color_to_rgb(&cell.bg),
            });
        }

        cells
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_size(cols: u16, rows: u16) -> SizeInfo {
    const CELL_W: f32 = 9.0;
    const CELL_H: f32 = 18.0;

    SizeInfo::new(
        cols as f32 * CELL_W,  // total width  in px
        rows as f32 * CELL_H,  // total height in px
        CELL_W,
        CELL_H,
        0.0, // padding_x
        0.0, // padding_y
    )
}

/// Convert alacritty_terminal's `Color` to an RGB triple.
///
/// Named / indexed colours fall back to reasonable defaults; full 256/true-
/// colour support will be added in a later phase alongside a colour palette.
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
    match name {
        Black | DimBlack => [18, 18, 18],
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
        Foreground => [204, 204, 204],
        Background => [18, 18, 18],
        _ => [204, 204, 204],
    }
}

fn indexed_to_rgb(idx: u8) -> [u8; 3] {
    // Standard 256-colour palette
    match idx {
        // System colours 0-15 — same as named above
        0 => [18, 18, 18],
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
        // 6×6×6 colour cube (16–231)
        16..=231 => {
            let n = idx - 16;
            let b = n % 6;
            let g = (n / 6) % 6;
            let r = n / 36;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            [scale(r), scale(g), scale(b)]
        }
        // Greyscale ramp (232–255)
        232..=255 => {
            let v = 8 + (idx - 232) * 10;
            [v, v, v]
        }
    }
}

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;

use crate::terminal::emulator::TerminalState;

pub type PaneId = u32;

pub struct Pane {
    pub id: PaneId,
    pub terminal: Arc<Mutex<TerminalState>>,
    pub pty_tx: UnboundedSender<Vec<u8>>,
    pub dirty: Arc<AtomicBool>,
}

impl Pane {
    pub fn new(id: PaneId, cols: u16, rows: u16, pty_tx: UnboundedSender<Vec<u8>>) -> Self {
        let terminal = Arc::new(Mutex::new(TerminalState::new(cols, rows, pty_tx.clone())));
        let dirty = Arc::new(AtomicBool::new(true));
        Self { id, terminal, pty_tx, dirty }
    }
}

//! Bridges rio-vt's event callbacks to goatty: query replies go back to the
//! PTY (only on the terminal that answers queries), everything else becomes
//! a [`UiEvent`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rio_vt::event::{EventListener, RioEvent, WindowId};

use crate::terminal::SharedPalette;

/// Events the terminal emits that the *UI* cares about.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Title(String),
    ResetTitle,
    Bell,
    /// OSC 52 clipboard write from the application.
    ClipboardStore(String),
    CwdChanged,
    /// The shell reported that a command finished (OSC 133;D), with its
    /// exit status when known. Needs shell integration.
    CommandFinished {
        exit: Option<i32>,
    },
    Exit,
}

/// Listener plugged into `Crosswords`. It splits events into two channels:
/// *replies* (bytes that must go back to the PTY, e.g. DA/DSR answers) and
/// *UI events*.
///
/// When the same byte stream is parsed by two terminals (daemon + client),
/// only one of them may answer queries, or the shell sees every reply twice.
/// `answer_queries` selects which.
#[derive(Clone)]
pub(crate) struct Listener {
    pub(crate) replies: flume::Sender<Vec<u8>>,
    pub(crate) ui: flume::Sender<UiEvent>,
    pub(crate) answer_queries: bool,
    pub(crate) palette: SharedPalette,
    pub(crate) size: Arc<AtomicU32>, // packed (cols << 16 | rows)
}

impl EventListener for Listener {
    fn send_event(&self, event: RioEvent, _id: WindowId) {
        match event {
            RioEvent::PtyWrite(_, text) => {
                if self.answer_queries {
                    let _ = self.replies.send(text.into_bytes());
                }
            }
            RioEvent::ColorRequest(_, index, fmt) => {
                if self.answer_queries {
                    let c = self.palette.read().by_index(index);
                    let reply = fmt(rio_vt::config::colors::ColorRgb { r: c.r, g: c.g, b: c.b });
                    let _ = self.replies.send(reply.into_bytes());
                }
            }
            RioEvent::TextAreaSizeRequest(_, fmt) => {
                if self.answer_queries {
                    let packed = self.size.load(Ordering::Relaxed);
                    let ws = rio_vt::event::WindowSize { cols: (packed >> 16) as u16, rows: (packed & 0xffff) as u16, width: 0, height: 0 };
                    let _ = self.replies.send(fmt(ws).into_bytes());
                }
            }
            // OSC 52 reads are a data-exfiltration vector; deny by default.
            RioEvent::ClipboardLoad(..) => {}
            RioEvent::ClipboardStore(_, text) => {
                let _ = self.ui.send(UiEvent::ClipboardStore(text));
            }
            RioEvent::Title(_, t) => {
                let _ = self.ui.send(UiEvent::Title(t));
            }
            RioEvent::ResetTitle => {
                let _ = self.ui.send(UiEvent::ResetTitle);
            }
            RioEvent::Bell(_) => {
                let _ = self.ui.send(UiEvent::Bell);
            }
            RioEvent::CurrentDirectoryChanged(_) => {
                let _ = self.ui.send(UiEvent::CwdChanged);
            }
            RioEvent::Exit | RioEvent::CloseTerminal(_) => {
                let _ = self.ui.send(UiEvent::Exit);
            }
            _ => {}
        }
    }
}

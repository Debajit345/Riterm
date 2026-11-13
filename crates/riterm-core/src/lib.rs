//! Core terminal session management for riterm.
//!
//! Wraps `alacritty_terminal` (VT parsing, grid, scrollback, selection) and a
//! PTY, and adds iTerm2-compatible shell integration (OSC 133 prompt marks).

pub mod session;
pub mod shell_integration;
pub mod size;

pub use session::{
    CommandState, Mark, MarkKind, Session, SessionConfig, SessionEvent, SessionSink,
};
pub use size::TermSize;

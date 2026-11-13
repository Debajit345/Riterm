use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{EventListener, OnResize, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty::{self, EventedReadWrite, Options as PtyOptions, Pty, Shell};
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};

use crate::shell_integration::{OscMark, OscScanner, ScanItem};
use crate::size::TermSize;

/// Callback used by a session to deliver terminal events to the UI.
pub type SessionSink = Arc<dyn Fn(SessionEvent) + Send + Sync + 'static>;

/// Source of globally monotonic command-finished stamps. Starting at 1 lets 0
/// mean "no command has finished yet".
static COMMAND_FINISHED_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Events produced by a session and consumed by the UI event loop.
#[derive(Debug)]
pub enum SessionEvent {
    /// Raw bytes read from the PTY.
    Bytes(Vec<u8>),
    /// Event emitted by the VT emulator (title change, clipboard request, ...).
    Term(alacritty_terminal::event::Event),
    /// The child process exited or the PTY closed.
    Exited,
    /// An I/O error occurred.
    Error(String),
}

/// State of the shell's current command, derived from OSC 133.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommandState {
    #[default]
    Idle,
    Running,
    Finished(Option<i32>),
}

/// Kind of a shell-integration mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    PromptStart,
    PromptEnd,
    CommandStart,
    CommandFinished,
}

/// A position in the scrollback where a shell mark was recorded.
///
/// `history_size` is stored so the mark can be re-anchored if scrollback
/// trimming ever shifts grid coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pub kind: MarkKind,
    pub point: Point,
    pub history_size: usize,
    pub at: Instant,
}

/// Options for spawning a terminal session.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub shell: Option<String>,
    pub args: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub scrollback: usize,
    pub cols: usize,
    pub rows: usize,
    pub cell_width: u16,
    pub cell_height: u16,
    pub session_id: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            shell: None,
            args: Vec::new(),
            working_directory: None,
            env: HashMap::new(),
            scrollback: 10_000,
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
            session_id: "w0t0p0".to_string(),
        }
    }
}

/// Terminal event proxy handed to `alacritty_terminal`.
pub struct TermProxy {
    sink: SessionSink,
}

impl EventListener for TermProxy {
    fn send_event(&self, event: alacritty_terminal::event::Event) {
        (self.sink)(SessionEvent::Term(event));
    }
}

/// A terminal session: PTY + VT emulator + shell-integration state.
pub struct Session {
    pub id: u64,
    pub term: Term<TermProxy>,
    pub size: TermSize,
    pub title: String,
    pub command_state: CommandState,
    pub marks: Vec<Mark>,
    pub dirty: bool,
    pub exited: bool,
    /// Globally monotonic stamp set every time a command finishes. Comparing it
    /// against a tab's acknowledged epoch lets the UI notice failures even when
    /// `D` is immediately followed by `A` in the same read.
    pub command_finished_epoch: u64,
    /// Exit status of the most recent finished command. Retained across the
    /// following `A` mark, which resets `command_state` to [`CommandState::Idle`].
    pub last_exit_code: Option<i32>,

    pty: Pty,
    processor: Processor,
    scanner: OscScanner,
    write_tx: Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
    io_thread: Option<JoinHandle<()>>,
}

impl Session {
    pub fn spawn(id: u64, config: SessionConfig, sink: SessionSink) -> Result<Self> {
        let size = TermSize::new(config.cols, config.rows);

        let mut env = config.env.clone();
        env.entry("TERM".into())
            .or_insert_with(|| "xterm-256color".into());
        env.entry("COLORTERM".into())
            .or_insert_with(|| "truecolor".into());
        env.entry("TERM_PROGRAM".into())
            .or_insert_with(|| "iTerm2".into());
        env.entry("TERM_PROGRAM_VERSION".into())
            .or_insert_with(|| format!("3.6.0 (riterm {})", env!("CARGO_PKG_VERSION")));
        env.entry("LC_TERMINAL".into())
            .or_insert_with(|| "iTerm2".into());
        env.entry("LC_TERMINAL_VERSION".into())
            .or_insert_with(|| env!("CARGO_PKG_VERSION").to_string());
        env.entry("ITERM_SESSION_ID".into())
            .or_insert_with(|| config.session_id.clone());
        env.entry("RITERM".into()).or_insert_with(|| "1".into());

        let shell = config
            .shell
            .as_ref()
            .map(|program| Shell::new(program.clone(), config.args.clone()));

        let pty_options = PtyOptions {
            shell,
            working_directory: config.working_directory.clone(),
            drain_on_exit: false,
            env,
            #[cfg(target_os = "windows")]
            escape_args: true,
        };

        let window_size = WindowSize {
            num_lines: size.rows as u16,
            num_cols: size.cols as u16,
            cell_width: config.cell_width.max(1),
            cell_height: config.cell_height.max(1),
        };

        let mut pty = tty::new(&pty_options, window_size, 0)
            .with_context(|| format!("failed to spawn shell for session {id}"))?;

        let mut reader = pty
            .reader()
            .try_clone()
            .context("failed to clone PTY reader")?;
        let mut writer = pty
            .writer()
            .try_clone()
            .context("failed to clone PTY writer")?;

        let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>();
        let stop = Arc::new(AtomicBool::new(false));
        let io_stop = stop.clone();
        let io_sink = sink.clone();

        let io_thread = thread::Builder::new()
            .name(format!("riterm-pty-{id}"))
            .spawn(move || {
                let mut buf = [0u8; 64 * 1024];
                loop {
                    if io_stop.load(Ordering::Relaxed) {
                        break;
                    }

                    let mut progress = false;

                    // Drain pending writes.
                    while let Ok(mut data) = write_rx.try_recv() {
                        let mut attempts = 0;
                        while !data.is_empty() {
                            match writer.write(&data) {
                                Ok(0) => break,
                                Ok(n) => {
                                    data.drain(..n);
                                }
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                    if attempts > 200 {
                                        break;
                                    }
                                    attempts += 1;
                                    thread::sleep(Duration::from_millis(1));
                                }
                                Err(_) => break,
                            }
                        }
                        progress = true;
                    }

                    match reader.read(&mut buf) {
                        Ok(0) => {
                            (io_sink)(SessionEvent::Exited);
                            break;
                        }
                        Ok(n) => {
                            (io_sink)(SessionEvent::Bytes(buf[..n].to_vec()));
                            progress = true;
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                        Err(e) if e.raw_os_error() == Some(5) => {
                            // EIO: the slave side closed.
                            (io_sink)(SessionEvent::Exited);
                            break;
                        }
                        Err(e) => {
                            (io_sink)(SessionEvent::Error(format!("PTY read failed: {e}")));
                            break;
                        }
                    }

                    if !progress {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            })
            .context("failed to spawn PTY I/O thread")?;

        let proxy = TermProxy { sink };
        let term_config = TermConfig {
            scrolling_history: config.scrollback,
            ..TermConfig::default()
        };
        let term = Term::new(term_config, &size, proxy);

        Ok(Self {
            id,
            term,
            size,
            title: String::new(),
            command_state: CommandState::Idle,
            marks: Vec::new(),
            dirty: true,
            exited: false,
            command_finished_epoch: 0,
            last_exit_code: None,
            pty,
            processor: Processor::new(),
            scanner: OscScanner::new(),
            write_tx,
            stop,
            io_thread: Some(io_thread),
        })
    }

    /// Feed raw PTY bytes through the OSC 133 scanner and the VT parser, in
    /// stream order, so marks land on the correct grid line.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let items = self.scanner.scan(bytes);
        let mut pos = 0;
        for item in items {
            match item {
                ScanItem::Mark { end, mark } => {
                    // The mark sequence itself is harmless to the emulator, so
                    // feed everything up to and including it first.
                    self.processor.advance(&mut self.term, &bytes[pos..end]);
                    pos = end;
                    self.record_mark(mark);
                }
            }
        }
        if pos < bytes.len() {
            self.processor.advance(&mut self.term, &bytes[pos..]);
        }
        self.dirty = true;
    }

    fn record_mark(&mut self, mark: OscMark) {
        let point = self.term.grid().cursor.point;
        let history_size = self.term.grid().history_size();
        let kind = match mark {
            OscMark::PromptStart => MarkKind::PromptStart,
            OscMark::PromptEnd => MarkKind::PromptEnd,
            OscMark::CommandStart => MarkKind::CommandStart,
            OscMark::CommandFinished(_) => MarkKind::CommandFinished,
        };

        self.marks.push(Mark {
            kind,
            point,
            history_size,
            at: Instant::now(),
        });
        // Keep memory bounded in very long-lived sessions.
        if self.marks.len() > 10_000 {
            self.marks.drain(..1_000);
        }

        match mark {
            OscMark::PromptStart => {
                if matches!(self.command_state, CommandState::Finished(_)) {
                    self.command_state = CommandState::Idle;
                }
            }
            OscMark::PromptEnd => {}
            OscMark::CommandStart => {
                self.command_state = CommandState::Running;
            }
            OscMark::CommandFinished(code) => {
                self.command_state = CommandState::Finished(code);
                self.last_exit_code = code;
                self.command_finished_epoch =
                    COMMAND_FINISHED_EPOCH.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.dirty = true;
    }

    /// Queue bytes for the child process.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.write_tx.send(bytes.into());
    }

    /// Resize the emulator and PTY.
    pub fn resize(&mut self, cols: usize, rows: usize, cell_width: u16, cell_height: u16) {
        let size = TermSize::new(cols, rows);
        if size == self.size {
            // Cell size can change with font zoom even when the grid does not.
            self.pty.on_resize(WindowSize {
                num_lines: size.rows as u16,
                num_cols: size.cols as u16,
                cell_width: cell_width.max(1),
                cell_height: cell_height.max(1),
            });
            return;
        }

        self.term.resize(size);
        self.size = size;
        self.pty.on_resize(WindowSize {
            num_lines: size.rows as u16,
            num_cols: size.cols as u16,
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
        });
        self.dirty = true;
    }

    /// Scroll the viewport by `delta` lines (positive = up).
    pub fn scroll(&mut self, delta: i32) {
        self.term
            .scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        self.dirty = true;
    }

    /// Number of lines the viewport is scrolled back.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scroll the viewport all the way back to the live prompt.
    pub fn scroll_to_bottom(&mut self) {
        self.term
            .scroll_display(alacritty_terminal::grid::Scroll::Bottom);
        self.dirty = true;
    }

    /// Drop the scrollback history. The visible screen is kept.
    pub fn clear_scrollback(&mut self) {
        self.term.grid_mut().clear_history();
        self.dirty = true;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.io_thread.take() {
            let _ = handle.join();
        }
        // Dropping `pty` kills the child process with SIGHUP.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn scratch_session() -> Session {
        let sink: SessionSink = Arc::new(|_| {});
        Session::spawn(
            1,
            SessionConfig {
                shell: Some("/bin/sh".into()),
                args: vec!["-c".into(), "exit 0".into()],
                cols: 40,
                rows: 10,
                ..SessionConfig::default()
            },
            sink,
        )
        .expect("spawn session")
    }

    #[test]
    fn exit_code_survives_the_next_prompt_start() {
        let mut session = scratch_session();
        session.feed(b"\x1b]133;D;1\x07");
        assert_eq!(session.command_state, CommandState::Finished(Some(1)));
        assert_eq!(session.last_exit_code, Some(1));
        assert!(session.command_finished_epoch > 0);

        // Bash emits D and A back to back on the next prompt; A resets the
        // live state but must not erase the failure.
        session.feed(b"\x1b]133;A\x07");
        assert_eq!(session.command_state, CommandState::Idle);
        assert_eq!(session.last_exit_code, Some(1));
        assert!(session.command_finished_epoch > 0);
    }

    #[test]
    fn finished_epochs_are_monotonic_across_sessions() {
        let mut first = scratch_session();
        first.feed(b"\x1b]133;D;1\x07");
        let first_epoch = first.command_finished_epoch;

        let mut second = scratch_session();
        second.feed(b"\x1b]133;D;0\x07");
        assert!(second.command_finished_epoch > first_epoch);
        assert_eq!(second.last_exit_code, Some(0));
    }

    #[test]
    fn spawns_shell_and_feeds_output() {
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let received = Arc::clone(&output);
        let exited = Arc::new(AtomicBool::new(false));
        let exited_flag = Arc::clone(&exited);

        let sink: SessionSink = Arc::new(move |event| match event {
            SessionEvent::Bytes(bytes) => received.lock().unwrap().extend_from_slice(&bytes),
            SessionEvent::Exited => exited_flag.store(true, Ordering::SeqCst),
            _ => {}
        });

        let config = SessionConfig {
            shell: Some("/bin/sh".into()),
            args: vec!["-c".into(), "printf 'riterm-test-ok\\n'".into()],
            cols: 40,
            rows: 10,
            ..SessionConfig::default()
        };
        let mut session = Session::spawn(1, config, sink).expect("spawn session");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if exited.load(Ordering::SeqCst) && !output.lock().unwrap().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for shell output"
            );
            thread::sleep(Duration::from_millis(20));
        }

        let bytes = output.lock().unwrap().clone();
        session.feed(&bytes);

        let mut text = String::new();
        for row in 0..session.size.rows {
            for col in 0..session.size.cols {
                let point = Point::new(
                    alacritty_terminal::index::Line(row as i32),
                    alacritty_terminal::index::Column(col),
                );
                text.push(session.term.grid()[point].c);
            }
            text.push('\n');
        }
        assert!(
            text.contains("riterm-test-ok"),
            "terminal grid did not contain the output: {text:?}"
        );
    }
}

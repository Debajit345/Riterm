//! OSC 133 shell-integration scanner.
//!
//! iTerm2's shell integration emits:
//!   OSC 133 ; A ST              prompt start
//!   OSC 133 ; B ST              prompt end / command input start
//!   OSC 133 ; C ST              command output start
//!   OSC 133 ; D [ ; exit ] ST   command finished
//!
//! The scanner is deliberately independent of the VT parser: it only reports
//! byte offsets where a complete 133 sequence ends, so the caller can feed the
//! bytes to the terminal in order and record marks at the right cursor position.

/// A shell-integration mark parsed out of an OSC 133 sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OscMark {
    /// Start of the prompt.
    PromptStart,
    /// End of the prompt; user input begins.
    PromptEnd,
    /// Command was executed; output begins.
    CommandStart,
    /// Command finished, with an optional exit code.
    CommandFinished(Option<i32>),
}

impl OscMark {
    fn parse(payload: &str) -> Option<Self> {
        let rest = payload.strip_prefix("133;")?;
        let mut fields = rest.split(';');
        let kind = fields.next()?.chars().next()?;
        match kind {
            'A' => Some(Self::PromptStart),
            'B' => Some(Self::PromptEnd),
            'C' => Some(Self::CommandStart),
            'D' => Some(Self::CommandFinished(
                fields.next().and_then(|s| s.trim().parse::<i32>().ok()),
            )),
            _ => None,
        }
    }
}

/// One step produced by scanning a data chunk.
#[derive(Clone, Debug)]
pub enum ScanItem {
    /// A complete OSC 133 sequence ends at this byte offset (exclusive).
    Mark { end: usize, mark: OscMark },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

/// Incremental OSC 133 scanner. Safe to feed arbitrary PTY chunks, including
/// sequences split across reads.
#[derive(Debug, Default)]
pub struct OscScanner {
    state: State,
    osc: String,
    /// Maximum OSC payload we track. Longer payloads are not shell marks.
    overflow: bool,
}

impl OscScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan `bytes`, returning marks in stream order. Bytes not referenced by a
    /// mark can be passed straight to the VT parser.
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<ScanItem> {
        let mut items = Vec::new();

        for (i, &b) in bytes.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.osc.clear();
                        self.overflow = false;
                        self.state = State::Osc;
                    } else {
                        self.state = State::Ground;
                    }
                }
                State::Osc => match b {
                    0x07 => {
                        self.finish(&mut items, i + 1);
                    }
                    0x1b => self.state = State::OscEsc,
                    _ => self.push(b),
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish(&mut items, i + 1);
                    } else {
                        // Not a string terminator: keep the ESC as payload.
                        self.push(0x1b);
                        self.push(b);
                        self.state = State::Osc;
                    }
                }
            }
        }

        items
    }

    fn push(&mut self, b: u8) {
        if self.osc.len() < 256 {
            self.osc.push(b as char);
        } else {
            self.overflow = true;
        }
    }

    fn finish(&mut self, items: &mut Vec<ScanItem>, end: usize) {
        self.state = State::Ground;
        if self.overflow {
            return;
        }
        if let Some(mark) = OscMark::parse(&self.osc) {
            items.push(ScanItem::Mark { end, mark });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_marks() {
        let mut s = OscScanner::new();
        let items = s.scan(b"\x1b]133;A\x07hello\x1b]133;B\x07");
        assert_eq!(items.len(), 2);
        assert!(matches!(
            items[0],
            ScanItem::Mark {
                mark: OscMark::PromptStart,
                ..
            }
        ));
    }

    #[test]
    fn parses_st_terminator_and_exit_code() {
        let mut s = OscScanner::new();
        let items = s.scan(b"\x1b]133;D;127\x1b\\");
        assert_eq!(items.len(), 1);
        assert!(matches!(
            items[0],
            ScanItem::Mark {
                mark: OscMark::CommandFinished(Some(127)),
                ..
            }
        ));
    }

    #[test]
    fn handles_split_sequences() {
        let mut s = OscScanner::new();
        assert!(s.scan(b"\x1b]13").is_empty());
        let items = s.scan(b"3;A\x07");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn ignores_other_osc() {
        let mut s = OscScanner::new();
        assert!(s.scan(b"\x1b]0;title\x07\x1b]133;A\x07").len() == 1);
    }
}

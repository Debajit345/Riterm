//! Output trigger scanning.
//!
//! Triggers match printable terminal output rather than raw PTY bytes. The
//! scanner strips ANSI escape sequences and emits complete lines at `\n` or
//! `\r` boundaries (progress output often uses bare carriage returns).

use regex::Regex;
use riterm_config::{Trigger, TriggerAction};

/// A configured trigger with a compiled regular expression.
pub struct CompiledTrigger {
    pub regex: Regex,
    pub action: TriggerAction,
    pub text: Option<String>,
}

/// Compile enabled triggers, skipping empty and invalid expressions.
pub fn compile(triggers: &[Trigger]) -> Vec<CompiledTrigger> {
    let mut compiled = Vec::new();
    for trigger in triggers.iter().filter(|trigger| trigger.enabled) {
        if trigger.regex.trim().is_empty() {
            continue;
        }
        match Regex::new(&trigger.regex) {
            Ok(regex) => compiled.push(CompiledTrigger {
                regex,
                action: trigger.action,
                text: trigger.text.clone(),
            }),
            Err(err) => {
                tracing::warn!("ignoring trigger {:?}: invalid regex: {err}", trigger.regex)
            }
        }
    }
    compiled
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

/// Maximum bytes retained for one unterminated output line.
const MAX_LINE: usize = 64 * 1024;

/// Incremental ANSI-stripping line scanner for trigger matching.
#[derive(Debug, Default)]
pub struct TriggerScanner {
    state: State,
    line: Vec<u8>,
}

impl TriggerScanner {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            self.feed(byte, &mut lines);
        }
        lines
    }

    fn feed(&mut self, byte: u8, lines: &mut Vec<String>) {
        match self.state {
            State::Ground => match byte {
                0x1b => self.state = State::Escape,
                b'\n' | b'\r' => self.finish_line(lines),
                b'\t' => self.line.push(b' '),
                0x00..=0x1f | 0x7f => {}
                _ => self.line.push(byte),
            },
            State::Escape => match byte {
                b'[' => self.state = State::Csi,
                b']' | b'P' | b'^' | b'_' => self.state = State::Osc,
                b'\\' => self.state = State::Ground,
                _ => self.state = State::Ground,
            },
            State::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.state = State::Ground;
                }
            }
            State::Osc => match byte {
                0x07 => self.state = State::Ground,
                0x1b => self.state = State::OscEscape,
                _ => {}
            },
            State::OscEscape => {
                self.state = if byte == b'\\' {
                    State::Ground
                } else {
                    State::Osc
                };
            }
        }

        if self.line.len() > MAX_LINE {
            let keep_from = self.line.len() - MAX_LINE / 2;
            self.line.drain(..keep_from);
        }
    }

    fn finish_line(&mut self, lines: &mut Vec<String>) {
        if self.line.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(&self.line).into_owned();
        self.line.clear();
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riterm_config::TriggerAction;

    #[test]
    fn strips_ansi_and_emits_lines() {
        let mut scanner = TriggerScanner::default();
        let lines = scanner.push(b"\x1b[31merror\x1b[0m: bad\r\nnext\x1b]0;title\x07 line\n");
        assert_eq!(lines, vec!["error: bad", "next line"]);
    }

    #[test]
    fn handles_sequences_split_across_chunks() {
        let mut scanner = TriggerScanner::default();
        assert!(scanner.push(b"\x1b[3").is_empty());
        assert!(scanner.push(b"1mfail").is_empty());
        assert_eq!(scanner.push(b"ed\x1b[0m\n"), vec!["failed"]);
        assert_eq!(scanner.push(b"done\n"), vec!["done"]);
    }

    #[test]
    fn bare_carriage_return_finishes_a_line() {
        let mut scanner = TriggerScanner::default();
        let lines = scanner.push(b"10%\r50%\r100%\n");
        assert_eq!(lines, vec!["10%", "50%", "100%"]);
    }

    #[test]
    fn compiles_enabled_valid_triggers() {
        let triggers = vec![
            Trigger {
                regex: "error".into(),
                ..Trigger::default()
            },
            Trigger {
                regex: "(".into(),
                ..Trigger::default()
            },
            Trigger {
                regex: "warning".into(),
                enabled: false,
                ..Trigger::default()
            },
        ];
        let compiled = compile(&triggers);
        assert_eq!(compiled.len(), 1);
        assert_eq!(compiled[0].action, TriggerAction::Notify);
        assert!(compiled[0].regex.is_match("an error happened"));
    }
}

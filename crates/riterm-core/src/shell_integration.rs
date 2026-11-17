//! OSC 133 shell-integration scanner.
//!
//! iTerm2's shell integration emits:
//!   OSC 133 ; A ST              prompt start
//!   OSC 133 ; B ST              prompt end / command input start
//!   OSC 133 ; C ST              command output start
//!   OSC 133 ; D [ ; exit ] ST   command finished
//!   OSC 7 ; file://host/path ST current working directory (URL-encoded)
//!   OSC 1337 ; CurrentDir=path ST  legacy current working directory
//!
//! The scanner is deliberately independent of the VT parser: it only reports
//! byte offsets where a complete sequence ends, so the caller can feed the
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
    /// A working-directory report (OSC 7 or `1337;CurrentDir`) ends here.
    CurrentDirectory {
        end: usize,
        path: String,
        /// User and host reported by OSC 7's URL authority (absent for the
        /// legacy `1337;CurrentDir` form).
        user: Option<String>,
        host: Option<String>,
    },
}

/// The pieces of an OSC 7 `file://[user@]host/path` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileUrl {
    pub path: String,
    pub user: Option<String>,
    pub host: Option<String>,
}

/// Maximum OSC payload tracked by the scanner. Long enough for a deeply nested
/// working-directory URL while still bounding memory for unrelated OSC data.
const MAX_OSC_PAYLOAD: usize = 8 * 1024;

/// Parse an OSC 7 `file://[user@]host/path` URL into a local path.
///
/// The path is percent-decoded. A query string (`?machineID=...`) and fragment
/// are ignored, matching iTerm2's OSC 7 handling.
pub fn parse_file_url(url: &str) -> Option<FileUrl> {
    let rest = url.strip_prefix("file://")?;
    // The authority ends at the first slash; `file:///path` has an empty one.
    let path_start = rest.find('/')?;
    let authority = &rest[..path_start];
    let path_and_more = &rest[path_start..];
    let end = path_and_more
        .find(|c| c == '?' || c == '#')
        .unwrap_or(path_and_more.len());
    let raw = &path_and_more.as_bytes()[..end];
    if raw.is_empty() {
        return None;
    }
    let decoded = percent_decode(raw);
    let path = match String::from_utf8(decoded) {
        Ok(path) => path,
        // iTerm2 falls back to ISO Latin-1 for paths that are not valid UTF-8.
        Err(err) => err.into_bytes().iter().map(|&b| b as char).collect(),
    };
    if path.is_empty() {
        return None;
    }

    let (user, host) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), Some(host)),
        None => (None, Some(authority)),
    };
    let decode = |value: &str| -> Option<String> {
        let value = percent_decode(value.as_bytes());
        let value = String::from_utf8_lossy(&value).into_owned();
        (!value.is_empty()).then_some(value)
    };
    Some(FileUrl {
        path,
        user: user.and_then(decode),
        // Strip an optional port so the status bar shows a clean host name.
        host: host.and_then(decode).map(|host| strip_port(&host)),
    })
}

fn strip_port(host: &str) -> String {
    if host.starts_with('[') {
        return host.to_string();
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.parse::<u16>().is_ok() => name.to_string(),
        _ => host.to_string(),
    }
}

fn parse_osc7(payload: &str) -> Option<FileUrl> {
    parse_file_url(payload.strip_prefix("7;")?)
}

fn parse_current_dir(payload: &str) -> Option<String> {
    let value = payload.strip_prefix("1337;CurrentDir=")?;
    if value.is_empty() {
        return None;
    }
    if value.starts_with("file://") {
        parse_file_url(value).map(|url| url.path)
    } else {
        Some(value.to_string())
    }
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let mut out = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            if let (Some(high), Some(low)) = (hex(input[index + 1]), hex(input[index + 2])) {
                out.push(high << 4 | low);
                index += 3;
                continue;
            }
        }
        out.push(input[index]);
        index += 1;
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

/// Incremental OSC scanner for shell integration. Safe to feed arbitrary PTY
/// chunks, including sequences split across reads.
#[derive(Debug, Default)]
pub struct OscScanner {
    state: State,
    osc: Vec<u8>,
    /// Set when the current OSC payload exceeds [`MAX_OSC_PAYLOAD`].
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
        if self.osc.len() < MAX_OSC_PAYLOAD {
            self.osc.push(b);
        } else {
            self.overflow = true;
        }
    }

    fn finish(&mut self, items: &mut Vec<ScanItem>, end: usize) {
        self.state = State::Ground;
        if self.overflow {
            self.osc.clear();
            return;
        }
        // OSC payloads are not necessarily UTF-8 (a legacy CurrentDir may hold
        // raw path bytes), so decode leniently instead of rejecting the report.
        let payload = match std::str::from_utf8(&self.osc) {
            Ok(text) => text.to_string(),
            Err(_) => self.osc.iter().map(|&b| b as char).collect(),
        };
        self.osc.clear();

        if let Some(mark) = OscMark::parse(&payload) {
            items.push(ScanItem::Mark { end, mark });
        } else if let Some(url) = parse_osc7(&payload) {
            items.push(ScanItem::CurrentDirectory {
                end,
                path: url.path,
                user: url.user,
                host: url.host,
            });
        } else if let Some(path) = parse_current_dir(&payload) {
            items.push(ScanItem::CurrentDirectory {
                end,
                path,
                user: None,
                host: None,
            });
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

    #[test]
    fn parses_osc7_working_directory() {
        let mut s = OscScanner::new();
        let items = s.scan(b"\x1b]7;file://studio/home/debaxoid/projects/riterm\x07");
        assert_eq!(items.len(), 1);
        match &items[0] {
            ScanItem::CurrentDirectory {
                path,
                end,
                user,
                host,
            } => {
                assert_eq!(path, "/home/debaxoid/projects/riterm");
                assert!(*end > 0);
                assert_eq!(user, &None);
                assert_eq!(host.as_deref(), Some("studio"));
            }
            other => panic!("expected working directory, got {other:?}"),
        }
    }

    #[test]
    fn osc7_decodes_percent_escapes_and_drops_query() {
        let mut s = OscScanner::new();
        let items = s.scan(b"\x1b]7;file://host/home/me/My%20Files/caf%C3%A9?machineID=local\x07");
        assert!(matches!(
            &items[..],
            [ScanItem::CurrentDirectory { path, .. }] if path == "/home/me/My Files/café"
        ));
    }

    #[test]
    fn osc7_reports_user_and_host() {
        let mut s = OscScanner::new();
        let items = s.scan(b"\x1b]7;file://alice@build.example.com:2222/work\x07");
        assert!(matches!(
            &items[..],
            [ScanItem::CurrentDirectory { path, user: Some(user), host: Some(host), .. }]
                if path == "/work" && user == "alice" && host == "build.example.com"
        ));
    }

    #[test]
    fn osc7_may_be_split_across_reads() {
        let mut s = OscScanner::new();
        assert!(s.scan(b"\x1b]7;file://host/tmp/").is_empty());
        let items = s.scan(b"split\x07");
        assert!(matches!(
            &items[..],
            [ScanItem::CurrentDirectory { path, .. }] if path == "/tmp/split"
        ));
    }

    #[test]
    fn parses_legacy_current_dir_kvp() {
        let mut s = OscScanner::new();
        let items = s.scan(b"\x1b]1337;CurrentDir=/var/log\x07");
        assert!(matches!(
            &items[..],
            [ScanItem::CurrentDirectory { path, .. }] if path == "/var/log"
        ));
    }

    #[test]
    fn rejects_osc7_without_a_path() {
        let mut s = OscScanner::new();
        assert!(s.scan(b"\x1b]7;file://host\x07").is_empty());
        assert!(s.scan(b"\x1b]7;https://example.com/x\x07").is_empty());
    }
}

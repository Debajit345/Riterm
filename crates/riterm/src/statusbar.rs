//! iTerm2-style status bar data.
//!
//! Rendering lives in `render.rs`; this module turns a profile's component list
//! plus live session/system state into styled text segments. System values are
//! sampled at most once per second, and git branch lookups are cached per
//! working directory, so drawing on every keypress stays cheap.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use riterm_config::{ResolvedProfile, StatusBarComponent};
use riterm_core::{CommandState, Session};

/// How a status bar segment is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Dim,
    Accent,
    Success,
    Failure,
    Running,
}

/// Width policy for one segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentWidth {
    /// Width of the text itself.
    Auto,
    /// Absorbs a share of the leftover row width.
    Flex,
    /// Fixed width in logical pixels.
    Fixed(u16),
}

/// One item in a rendered status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub tone: Tone,
    pub width: SegmentWidth,
}

impl Segment {
    fn text(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            width: SegmentWidth::Auto,
        }
    }

    fn flex() -> Self {
        Self {
            text: String::new(),
            tone: Tone::Normal,
            width: SegmentWidth::Flex,
        }
    }

    fn fixed(points: u16) -> Self {
        Self {
            text: String::new(),
            tone: Tone::Normal,
            width: SegmentWidth::Fixed(points),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Battery {
    percent: u8,
    charging: bool,
}

#[derive(Clone, Debug, Default)]
struct SystemValues {
    clock: String,
    host: String,
    user: String,
    load: Option<String>,
    memory: Option<String>,
    battery: Option<Battery>,
}

impl SystemValues {
    fn sample() -> Self {
        Self {
            clock: clock_now(),
            host: hostname(),
            user: username(),
            load: read_load_average(),
            memory: read_memory(),
            battery: read_battery(),
        }
    }
}

#[derive(Clone, Debug)]
struct GitCache {
    cwd: PathBuf,
    branch: Option<String>,
    at: Instant,
}

/// Cached status bar state shared by every tab.
#[derive(Debug, Default)]
pub struct StatusBarState {
    system: Option<SystemValues>,
    refreshed_at: Option<Instant>,
    git: Option<GitCache>,
    /// Test/demo mode: system values never change and are set by [`Self::demo`].
    demo: bool,
}

impl StatusBarState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frozen, deterministic values for the offscreen render check.
    pub fn demo() -> Self {
        Self {
            system: Some(SystemValues {
                clock: "11/17 12:34".to_string(),
                host: "studio".to_string(),
                user: "me".to_string(),
                load: Some("0.42 0.38 0.31".to_string()),
                memory: Some("5.2/15.4 GB".to_string()),
                battery: Some(Battery {
                    percent: 84,
                    charging: false,
                }),
            }),
            refreshed_at: Some(Instant::now()),
            git: None,
            demo: true,
        }
    }

    /// Build segments for `profile` and the active session.
    pub fn segments(
        &mut self,
        profile: &ResolvedProfile,
        session: Option<&Session>,
        now: Instant,
    ) -> Vec<Segment> {
        if !self.demo
            && self.refreshed_at.map_or(true, |at| {
                now.saturating_duration_since(at) >= Duration::from_secs(1)
            })
        {
            self.system = Some(SystemValues::sample());
            self.refreshed_at = Some(now);
        }

        let mut segments = Vec::new();
        for component in &profile.status_bar_components {
            let segment = match component {
                StatusBarComponent::Cwd => cwd_segment(session),
                StatusBarComponent::Git => self.git_segment(session, now),
                StatusBarComponent::Command => session.and_then(command_segment),
                StatusBarComponent::Clock => self
                    .system
                    .as_ref()
                    .filter(|system| !system.clock.is_empty())
                    .map(|system| Segment::text(system.clock.clone(), Tone::Dim)),
                StatusBarComponent::Host => session
                    .and_then(|session| session.remote_host.clone())
                    .or_else(|| {
                        self.system
                            .as_ref()
                            .map(|system| system.host.clone())
                            .filter(|host| !host.is_empty())
                    })
                    .map(|host| Segment::text(host, Tone::Dim)),
                StatusBarComponent::User => session
                    .and_then(|session| session.remote_user.clone())
                    .or_else(|| {
                        self.system
                            .as_ref()
                            .map(|system| system.user.clone())
                            .filter(|user| !user.is_empty())
                    })
                    .map(|user| Segment::text(user, Tone::Dim)),
                StatusBarComponent::Load => self
                    .system
                    .as_ref()
                    .and_then(|system| system.load.clone())
                    .map(|load| Segment::text(load, Tone::Normal)),
                StatusBarComponent::Memory => self
                    .system
                    .as_ref()
                    .and_then(|system| system.memory.clone())
                    .map(|memory| Segment::text(memory, Tone::Normal)),
                StatusBarComponent::Battery => self.system.as_ref().and_then(|system| {
                    system.battery.map(|battery| {
                        let text = if battery.charging {
                            format!("{}% +", battery.percent)
                        } else {
                            format!("{}%", battery.percent)
                        };
                        Segment::text(
                            text,
                            if battery.charging {
                                Tone::Success
                            } else {
                                Tone::Normal
                            },
                        )
                    })
                }),
                StatusBarComponent::Session => session
                    .map(|session| session.title.trim())
                    .filter(|title| !title.is_empty())
                    .map(|title| Segment::text(title.to_string(), Tone::Normal)),
                StatusBarComponent::Spacer => Some(Segment::flex()),
                StatusBarComponent::FixedSpacer(points) => Some(Segment::fixed(*points)),
                StatusBarComponent::Custom(text) if !text.is_empty() => {
                    Some(Segment::text(text.clone(), Tone::Normal))
                }
                StatusBarComponent::Custom(_) => None,
            };
            segments.extend(segment);
        }
        segments
    }

    fn git_segment(&mut self, session: Option<&Session>, now: Instant) -> Option<Segment> {
        let cwd = session?.cwd.clone()?;
        let fresh = self.git.as_ref().is_some_and(|cache| {
            cache.cwd == cwd && now.saturating_duration_since(cache.at) < Duration::from_secs(2)
        });
        if !fresh {
            let branch = git_branch(&cwd);
            self.git = Some(GitCache {
                cwd,
                branch,
                at: now,
            });
        }
        let branch = self.git.as_ref()?.branch.clone()?;
        Some(Segment::text(branch, Tone::Accent))
    }
}

fn cwd_segment(session: Option<&Session>) -> Option<Segment> {
    let cwd = session?.cwd.as_deref()?;
    let text = display_path(cwd, 48);
    (!text.is_empty()).then(|| Segment::text(text, Tone::Normal))
}

fn command_segment(session: &Session) -> Option<Segment> {
    match session.command_state {
        CommandState::Running => Some(Segment::text("running", Tone::Running)),
        CommandState::Finished(Some(code)) => Some(exit_segment(code)),
        CommandState::Finished(None) => Some(Segment::text("done", Tone::Dim)),
        CommandState::Idle => session.last_exit_code.map(exit_segment),
    }
}

fn exit_segment(code: i32) -> Segment {
    if code == 0 {
        Segment::text("✓ 0", Tone::Success)
    } else {
        Segment::text(format!("✗ {code}"), Tone::Failure)
    }
}

/// Render a path with `~` for the home directory, shortening long paths from
/// the middle so the final component stays visible.
pub fn display_path(path: &Path, max_chars: usize) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    display_path_with_home(path, home.as_deref(), max_chars)
}

fn display_path_with_home(path: &Path, home: Option<&Path>, max_chars: usize) -> String {
    let (prefix, relative) = match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => ("~".to_string(), rest.to_path_buf()),
        None => (String::new(), path.to_path_buf()),
    };
    let full = if prefix.is_empty() {
        path.display().to_string()
    } else if relative.as_os_str().is_empty() {
        "~".to_string()
    } else {
        format!("~/{}", relative.display())
    };
    let max_chars = max_chars.max(2);
    if full.chars().count() <= max_chars {
        return full;
    }

    let components: Vec<String> = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if components.is_empty() {
        return truncate_middle(&full, max_chars);
    }

    let root = if prefix.is_empty() {
        if path.is_absolute() {
            "/".to_string()
        } else {
            String::new()
        }
    } else {
        format!("{prefix}/")
    };
    let mut tail: Vec<String> = Vec::new();
    for component in components.iter().rev() {
        let mut candidate = vec![component.clone()];
        candidate.extend(tail.iter().cloned());
        let text = format!("{root}…/{}", candidate.join("/"));
        if text.chars().count() > max_chars && !tail.is_empty() {
            break;
        }
        tail = candidate;
    }
    let shortened = format!("{root}…/{}", tail.join("/"));
    truncate_middle(&shortened, max_chars)
}

/// Shorten `text` to `max` characters, keeping both ends and inserting `…`.
pub fn truncate_middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(chars[count - tail..].iter());
    out
}

/// Walk up from `cwd` looking for a `.git` directory or worktree file, then
/// read the branch from `HEAD`.
pub fn git_branch(cwd: &Path) -> Option<String> {
    let mut dir = Some(cwd);
    for _ in 0..64 {
        let current = dir?;
        let dot_git = current.join(".git");
        if dot_git.is_dir() {
            return read_head(&dot_git.join("HEAD"));
        }
        if dot_git.is_file() {
            let contents = fs::read_to_string(&dot_git).ok()?;
            let git_dir = contents.strip_prefix("gitdir:")?.trim();
            let git_dir = if Path::new(git_dir).is_absolute() {
                PathBuf::from(git_dir)
            } else {
                current.join(git_dir)
            };
            return read_head(&git_dir.join("HEAD"));
        }
        dir = current.parent();
    }
    None
}

fn read_head(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let contents = contents.trim();
    if let Some(reference) = contents.strip_prefix("ref:") {
        let reference = reference.trim();
        let branch = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return (!branch.is_empty()).then(|| branch.to_string());
    }
    let hash: String = contents.chars().take(7).collect();
    (!hash.is_empty()).then_some(hash)
}

fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "localhost".to_string())
}

fn username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

fn read_load_average() -> Option<String> {
    parse_loadavg(&fs::read_to_string("/proc/loadavg").ok()?)
}

fn parse_loadavg(text: &str) -> Option<String> {
    let values: Vec<&str> = text.split_whitespace().take(3).collect();
    (values.len() == 3).then(|| values.join(" "))
}

fn read_memory() -> Option<String> {
    parse_meminfo(&fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_meminfo(text: &str) -> Option<String> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total = value.split_whitespace().next()?.parse::<u64>().ok();
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available = value.split_whitespace().next()?.parse::<u64>().ok();
        }
        if total.is_some() && available.is_some() {
            break;
        }
    }
    let total = total?;
    let available = available?;
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(available);
    let gib = 1024.0 * 1024.0;
    Some(format!(
        "{:.1}/{:.1} GB",
        used as f64 / gib,
        total as f64 / gib
    ))
}

fn read_battery() -> Option<Battery> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
        if kind.trim() != "Battery" {
            continue;
        }
        let Ok(capacity) = fs::read_to_string(path.join("capacity")) else {
            continue;
        };
        let Ok(percent) = capacity.trim().parse::<f32>() else {
            continue;
        };
        let status = fs::read_to_string(path.join("status")).unwrap_or_default();
        return Some(Battery {
            percent: percent.round().clamp(0.0, 100.0) as u8,
            charging: status.trim().eq_ignore_ascii_case("charging"),
        });
    }
    None
}

fn clock_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `tm` is a valid, writable `struct tm` and `seconds` is a valid
    // `time_t`; localtime_r writes only into `tm`.
    let result = unsafe { libc::localtime_r(&seconds, &mut tm) };
    if result.is_null() {
        return String::new();
    }
    format_clock(tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// iTerm2's default clock format is `M/dd h:mm` (12-hour, no AM/PM).
fn format_clock(month: i32, day: i32, hour: i32, minute: i32) -> String {
    let hour = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{month}/{day:02} {hour}:{minute:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_matches_iterm2_default_format() {
        assert_eq!(format_clock(11, 7, 0, 5), "11/07 12:05");
        assert_eq!(format_clock(11, 17, 14, 5), "11/17 2:05");
        assert_eq!(format_clock(1, 2, 12, 30), "1/02 12:30");
    }

    #[test]
    fn paths_use_home_and_shorten_in_the_middle() {
        let home = Path::new("/home/me");
        let path = Path::new("/home/me/projects/riterm/crates/riterm");
        assert_eq!(
            display_path_with_home(path, Some(home), 80),
            "~/projects/riterm/crates/riterm"
        );
        assert_eq!(
            display_path_with_home(path, Some(home), 20),
            "~/…/crates/riterm"
        );
        assert_eq!(
            display_path_with_home(Path::new("/var/log"), None, 80),
            "/var/log"
        );
        assert_eq!(
            display_path_with_home(Path::new("/usr/share/doc/package"), None, 12),
            "/…/package"
        );
        assert_eq!(
            display_path_with_home(Path::new("/home/me"), Some(home), 80),
            "~"
        );
    }

    #[test]
    fn truncation_keeps_both_ends() {
        assert_eq!(truncate_middle("abcdefghij", 5), "ab…ij");
        assert_eq!(truncate_middle("short", 10), "short");
        assert_eq!(truncate_middle("abcdef", 1), "…");
    }

    #[test]
    fn finds_git_branch_from_nested_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let git = temp.path().join(".git");
        fs::create_dir_all(&git).expect("create .git");
        fs::write(git.join("HEAD"), "ref: refs/heads/feature/status-bar\n").expect("write HEAD");
        let nested = temp.path().join("crates/riterm");
        fs::create_dir_all(&nested).expect("create nested");

        assert_eq!(git_branch(&nested).as_deref(), Some("feature/status-bar"));
    }

    #[test]
    fn reads_detached_head_and_worktree_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let worktree_git = temp.path().join("actual-git");
        fs::create_dir_all(&worktree_git).expect("create git dir");
        fs::write(worktree_git.join("HEAD"), "0123456789abcdef\n").expect("write HEAD");
        fs::write(
            temp.path().join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .expect("write worktree file");

        assert_eq!(git_branch(temp.path()).as_deref(), Some("0123456"));
    }

    #[test]
    fn parses_linux_system_files() {
        assert_eq!(
            parse_loadavg("0.42 0.38 0.31 1/234 5678\n").as_deref(),
            Some("0.42 0.38 0.31")
        );
        let meminfo = "MemTotal:       16384000 kB\nMemFree: 1000 kB\nMemAvailable: 8192000 kB\n";
        assert_eq!(parse_meminfo(meminfo).as_deref(), Some("7.8/15.6 GB"));
        assert_eq!(parse_meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
    }
}

//! iTerm2 badge interpolation.
//!
//! Badges use iTerm2's "swifty string" syntax: literal text plus `\(...)`
//! expressions. Riterm implements the safe variable subset (`session.*` and
//! `user.*`) that badges use in practice. Function calls and expressions are
//! not evaluated; they render as an empty string.

use std::collections::BTreeMap;
use std::path::Path;

use riterm_config::ResolvedProfile;
use riterm_core::Session;

/// Values available to badge interpolation for one session.
pub struct BadgeContext<'a> {
    pub profile_name: &'a str,
    /// Session title reported by the shell, if any.
    pub title: &'a str,
    pub cwd: Option<&'a Path>,
    /// Remote host from OSC 7, or the local hostname.
    pub host: &'a str,
    /// Remote user from OSC 7, or the local user name.
    pub user: &'a str,
    pub columns: usize,
    pub rows: usize,
    pub session_id: u64,
    pub user_vars: &'a BTreeMap<String, String>,
}

impl BadgeContext<'_> {
    fn variable(&self, name: &str) -> Option<String> {
        let value = match name {
            "session.path" => self.cwd.map(|path| path.display().to_string()),
            "session.hostname" => Some(self.host.to_string()),
            "session.username" => Some(self.user.to_string()),
            "session.name" if self.title.is_empty() => Some(self.profile_name.to_string()),
            "session.name" => Some(self.title.to_string()),
            "session.profileName" => Some(self.profile_name.to_string()),
            "session.columns" => Some(self.columns.to_string()),
            "session.rows" => Some(self.rows.to_string()),
            "session.id" => Some(self.session_id.to_string()),
            _ => {
                let key = name.strip_prefix("user.")?;
                self.user_vars.get(key).cloned()
            }
        };
        value
    }
}

/// Build the badge text for a session, preferring an OSC `SetBadgeFormat`
/// override over the profile's `badge_text`. `host` and `user` are the local
/// fallbacks used when the shell has not reported a remote identity.
pub fn text_for_session(
    profile: &ResolvedProfile,
    session: &Session,
    host: &str,
    user: &str,
) -> String {
    let format = session
        .badge_format
        .as_deref()
        .unwrap_or(&profile.badge_text);
    if format.is_empty() {
        return String::new();
    }
    let context = BadgeContext {
        profile_name: &profile.name,
        title: &session.title,
        cwd: session.cwd.as_deref(),
        host: session.remote_host.as_deref().unwrap_or(host),
        user: session.remote_user.as_deref().unwrap_or(user),
        columns: session.size.cols,
        rows: session.size.rows,
        session_id: session.id,
        user_vars: &session.user_vars,
    };
    interpolate(format, &context)
}

/// Expand a badge template. Unknown variables become empty strings, matching
/// the way a missing iTerm2 variable reads as nothing in a badge.
pub fn interpolate(format: &str, context: &BadgeContext<'_>) -> String {
    let mut out = String::with_capacity(format.len());
    let mut chars = format.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.peek().map(|(_, ch)| *ch) {
            Some('\\') => {
                chars.next();
                out.push('\\');
            }
            Some('(') => {
                chars.next();
                let start = index + 2;
                let mut end = None;
                let mut depth = 1usize;
                while let Some((at, ch)) = chars.next() {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(at);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let Some(end) = end else {
                    // Unterminated expression: keep the rest literally.
                    out.push_str(&format[index..]);
                    break;
                };
                let expression = format[start..end].trim();
                if is_variable(expression) {
                    if let Some(value) = context.variable(expression) {
                        out.push_str(&value);
                    }
                }
                // Function calls, arithmetic, and unknown names are skipped.
            }
            _ => out.push('\\'),
        }
    }
    out
}

fn is_variable(expression: &str) -> bool {
    let mut chars = expression.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn context<'a>(
        user_vars: &'a BTreeMap<String, String>,
        title: &'a str,
        cwd: Option<&'a Path>,
    ) -> BadgeContext<'a> {
        BadgeContext {
            profile_name: "Work",
            title,
            cwd,
            host: "studio",
            user: "me",
            columns: 120,
            rows: 40,
            session_id: 7,
            user_vars,
        }
    }

    #[test]
    fn expands_session_variables() {
        let vars = BTreeMap::new();
        let cwd = PathBuf::from("/home/me/projects/riterm");
        let ctx = context(&vars, "zsh", Some(&cwd));
        assert_eq!(
            interpolate(
                r"\(session.username)@\(session.hostname):\(session.path)",
                &ctx
            ),
            "me@studio:/home/me/projects/riterm"
        );
        assert_eq!(
            interpolate(r"\(session.name) \(session.columns)x\(session.rows)", &ctx),
            "zsh 120x40"
        );
    }

    #[test]
    fn expands_user_variables() {
        let mut vars = BTreeMap::new();
        vars.insert("env".to_string(), "prod".to_string());
        let ctx = context(&vars, "", None);
        assert_eq!(interpolate(r"\(user.env) deploy", &ctx), "prod deploy");
        assert_eq!(interpolate(r"\(user.missing)!", &ctx), "!");
    }

    #[test]
    fn falls_back_to_the_profile_name_for_an_empty_title() {
        let vars = BTreeMap::new();
        let ctx = context(&vars, "", None);
        assert_eq!(interpolate(r"\(session.name)", &ctx), "Work");
    }

    #[test]
    fn leaves_literal_text_and_escaped_backslashes_alone() {
        let vars = BTreeMap::new();
        let ctx = context(&vars, "", None);
        assert_eq!(interpolate(r"plain \\ text", &ctx), r"plain \ text");
        assert_eq!(interpolate("no variables", &ctx), "no variables");
    }

    #[test]
    fn rejects_function_calls_and_unterminated_expressions() {
        let vars = BTreeMap::new();
        let ctx = context(&vars, "", None);
        assert_eq!(interpolate(r"\(session.path())", &ctx), "");
        assert_eq!(
            interpolate(r"before \(session.path", &ctx),
            r"before \(session.path"
        );
    }

    #[test]
    fn session_badge_override_and_user_vars_are_used() {
        use base64::Engine as _;
        use riterm_core::{SessionConfig, SessionSink};
        use std::sync::Arc;

        let sink: SessionSink = Arc::new(|_| {});
        let mut session = Session::spawn(
            9,
            SessionConfig {
                shell: Some("/bin/sh".into()),
                args: vec!["-c".into(), "exit 0".into()],
                cols: 40,
                rows: 12,
                ..SessionConfig::default()
            },
            sink,
        )
        .expect("spawn session");

        let mut profile = ResolvedProfile::default();
        profile.name = "Work".into();
        profile.badge_text = "profile".into();
        assert_eq!(
            text_for_session(&profile, &session, "studio", "me"),
            "profile"
        );

        let b64 = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
        let vars = format!("\x1b]1337;SetUserVar=env={}\x07", b64("prod"));
        session.feed(vars.as_bytes());
        let badge = format!(
            "\x1b]1337;SetBadgeFormat={}\x07",
            b64(r"\(user.env) \(session.rows)")
        );
        session.feed(badge.as_bytes());
        assert_eq!(
            text_for_session(&profile, &session, "studio", "me"),
            "prod 12"
        );
    }
}

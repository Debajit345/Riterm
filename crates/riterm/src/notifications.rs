//! Desktop notifications through the freedesktop `notify-send` utility.
//!
//! Sending happens on a detached thread so a slow notification daemon never
//! blocks the event loop, and the child is reaped there as well.

use std::process::{Command, Stdio};

/// Post a notification. Missing `notify-send` is not an error for the UI.
pub fn send(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let child = Command::new("notify-send")
            .args([
                "--app-name=Riterm",
                "--urgency=normal",
                "--expire-time=6000",
                "--",
                &summary,
                &body,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(err) => tracing::debug!("notify-send unavailable: {err}"),
        }
    });
}

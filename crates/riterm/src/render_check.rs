//! Deterministic offscreen render used to verify the GPU pipeline without a
//! window. Produces a PNG with the tab bar, a split layout, styles, colors, and
//! cursor so rendering regressions are visible in CI.

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use riterm_config::{Config, ItermColors, ResolvedProfile, ThemeVariant};
use riterm_core::{Session, SessionConfig, SessionSink};

use crate::layout::Rect;
use crate::render::{Renderer, TabInfo};

const TAB_BAR_HEIGHT: f32 = 34.0;
const PADDING: f32 = 8.0;

pub fn run(path: &Path, config_path: &Path, width: u32, height: u32) -> Result<()> {
    let config = Config::load(config_path).unwrap_or_default();
    let profile = config.resolve(&config.general.default_profile);
    let secondary = preset_profile(&profile, "Solarized Dark");

    let mut renderer = Renderer::new_offscreen(width, height, 1.0)?;

    let sink: SessionSink = Arc::new(|_| {});
    let mut primary = spawn_scratch_session(1, sink.clone())?;
    let mut alternate = spawn_scratch_session(2, sink)?;
    primary.feed(&primary_screen());
    alternate.feed(&secondary_screen());

    renderer.cancel_frame();

    let tabs = [
        TabInfo {
            title: "Riterm — build".into(),
            active: true,
            bell: false,
            running: false,
        },
        TabInfo {
            title: "tests".into(),
            active: false,
            bell: false,
            running: true,
        },
        TabInfo {
            title: "logs".into(),
            active: false,
            bell: true,
            running: false,
        },
    ];
    renderer.prepare_tab_bar(
        &tabs,
        &profile,
        Rect::new(0.0, 0.0, width as f32, TAB_BAR_HEIGHT),
    );

    let content_top = TAB_BAR_HEIGHT;
    let content_height = (height as f32 - content_top).max(1.0);
    let split = (width as f32 * 0.62).round();
    let left = Rect::new(0.0, content_top, split - 1.0, content_height);
    let right = Rect::new(split, content_top, width as f32 - split, content_height);

    renderer.prepare_session(&mut primary, left, &profile, PADDING, true);
    renderer.prepare_session(&mut alternate, right, &secondary, PADDING, false);
    renderer.push_quad(
        Rect::new(split - 1.0, content_top, 1.0, content_height),
        profile.foreground.mix(profile.background, 0.72),
        1.0,
    );

    let pixels = renderer.render_offscreen(profile.background)?;
    write_png(path, width, height, &pixels)?;
    println!("wrote {} ({}x{}, offscreen)", path.display(), width, height);
    Ok(())
}

fn spawn_scratch_session(id: u64, sink: SessionSink) -> Result<Session> {
    Session::spawn(
        id,
        SessionConfig {
            shell: Some("/bin/sh".into()),
            args: vec!["-c".into(), "exit 0".into()],
            cols: 80,
            rows: 24,
            ..SessionConfig::default()
        },
        sink,
    )
}

fn preset_profile(base: &ResolvedProfile, name: &str) -> ResolvedProfile {
    let preset = riterm_config::presets::preset(name).expect("bundled iTerm2 preset");
    let colors = ItermColors::from_pairs(preset.colors);
    let mut profile = base.clone();
    profile.name = name.into();
    colors.apply_to(&mut profile, ThemeVariant::Dark);
    profile
}

fn primary_screen() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("\x1b[H\x1b[2J");
    s.push_str(
        "\x1b[1m\x1b[38;2;255;107;107mRiterm\x1b[0m\x1b[1m — iTerm2-style terminal for Linux\x1b[0m\r\n",
    );
    s.push_str(
        "\x1b[38;2;80;90;100m══════════════════════════════════════════════════════════════════════\x1b[0m\r\n",
    );
    s.push_str(
        "\x1b[1mbold default\x1b[0m  \x1b[1;31mbold ANSI red\x1b[0m  \x1b[3mitalic\x1b[0m  \x1b[4munderline\x1b[0m  \x1b[9mstrike\x1b[0m  \x1b[2mdim\x1b[0m  \x1b[7mreverse\x1b[0m\r\n",
    );
    s.push_str("Unicode: Ελληνικά Кириллица 日本語 中文 한글 العربية ← ↑ → ↓ ✓ ★ ♥ 😀 🚀 👩‍💻\r\n");
    s.push_str("ANSI  ");
    for index in 0..16 {
        s.push_str(&format!("\x1b[48;5;{index}m  \x1b[0m"));
    }
    s.push_str("\r\n256   ");
    for index in 16..52 {
        s.push_str(&format!("\x1b[48;5;{index}m \x1b[0m"));
    }
    s.push_str("\r\ncolor ");
    for step in 0..24 {
        let r = 255 - step * 10;
        let g = step * 10;
        let b = 128 + step * 5;
        s.push_str(&format!("\x1b[48;2;{r};{g};{b}m \x1b[0m"));
    }
    s.push_str("\r\n\r\n");
    s.push_str("\x1b[38;2;130;170;210m┌──────────────────────────────┐\x1b[0m\r\n");
    s.push_str("\x1b[38;2;130;170;210m│\x1b[0m tabs · splits · profiles   \x1b[38;2;130;170;210m│\x1b[0m\r\n");
    s.push_str("\x1b[38;2;130;170;210m│\x1b[0m OSC 133 · iTerm2 import    \x1b[38;2;130;170;210m│\x1b[0m\r\n");
    s.push_str("\x1b[38;2;130;170;210m└──────────────────────────────┘\x1b[0m\r\n");
    s.push_str("\r\n");
    s.push_str("\x1b[38;5;42m$ \x1b[0mcargo test\r\n");
    s.push_str("   \x1b[38;5;39mCompiling\x1b[0m riterm v0.1.0\r\n");
    s.push_str("    \x1b[38;5;42mFinished\x1b[0m `test` profile [unoptimized + debuginfo]\r\n");
    s.push_str("     \x1b[38;5;39mRunning\x1b[0m unittests src/lib.rs\r\n");
    s.push_str("\x1b[32mtest result: ok.\x1b[0m 15 passed; 0 failed; 0 ignored\r\n");
    s.push_str("\r\n");
    s.push_str("\x1b[38;5;42m$ \x1b[0m");
    s.into_bytes()
}

fn secondary_screen() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("\x1b[H\x1b[2J");
    s.push_str("\x1b[1m\x1b[38;5;136mprofile:\x1b[0m Solarized Dark\r\n");
    s.push_str("\x1b[38;5;136m$\x1b[0m echo $TERM_PROGRAM\r\n");
    s.push_str("iTerm2\r\n");
    s.push_str("\x1b[38;5;136m$\x1b[0m git status\r\n");
    s.push_str("On branch \x1b[38;5;64mmain\x1b[0m\r\n");
    s.push_str("nothing to commit, working tree clean\r\n");
    s.push_str("\x1b[38;5;136m$\x1b[0m ls\r\n");
    s.push_str("\x1b[38;5;33mcrates\x1b[0m  \x1b[38;5;33mdocs\x1b[0m  LICENSE  README.md\r\n");
    s.push_str("shell-integration\r\n");
    s.push_str("\r\n");
    s.push_str("\x1b[38;5;136m$\x1b[0m ");
    s.into_bytes()
}

fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(&mut writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder.write_header().context("writing PNG header")?;
    png.write_image_data(pixels).context("writing PNG pixels")?;
    png.finish().context("finishing PNG")?;
    writer.flush().context("flushing PNG")?;
    Ok(())
}

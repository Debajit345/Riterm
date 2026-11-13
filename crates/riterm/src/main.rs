//! Riterm — an iTerm2-style terminal emulator for Linux.

mod app;
mod input;
mod layout;
mod render;
mod render_check;
mod search;
mod shell_install;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use riterm_config::{Config, Profile};
use tracing_subscriber::EnvFilter;
use winit::event_loop::EventLoop;

use crate::app::{App, UserEvent};

#[derive(Debug, Parser)]
#[command(
    name = "riterm",
    version,
    about = "Riterm — an iTerm2-style terminal emulator for Linux"
)]
struct Cli {
    /// Path to the configuration file (default: ~/.config/riterm/config.toml)
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Import profiles from an iTerm2 preferences plist and exit
    #[arg(long, value_name = "PLIST")]
    import_iterm2: Option<PathBuf>,

    /// Import an iTerm2 .itermcolors preset into the profile defaults and exit
    #[arg(long, value_name = "FILE")]
    import_theme: Option<PathBuf>,

    /// Use a built-in iTerm2 color preset or an .itermcolors file
    #[arg(long, value_name = "NAME_OR_FILE")]
    theme: Option<String>,

    /// List built-in iTerm2 color presets and exit
    #[arg(long)]
    list_themes: bool,

    /// Install shell integration for bash, zsh, fish, or all
    #[arg(
        long,
        value_name = "SHELL",
        num_args = 0..=1,
        default_missing_value = "all"
    )]
    install_shell_integration: Option<String>,

    /// Render a deterministic offscreen frame to a PNG and exit
    #[arg(long, value_name = "FILE")]
    render_check: Option<PathBuf>,

    /// Command to run instead of the login shell
    #[arg(
        short = 'e',
        long = "execute",
        value_name = "COMMAND",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    execute: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();

    if let Some(shell) = cli.install_shell_integration {
        return shell_install::install(&shell);
    }

    if cli.list_themes {
        println!("Built-in iTerm2 color presets:");
        for name in riterm_config::Config::theme_names() {
            println!("  {name}");
        }
        println!("\nSelect one with `theme = \"NAME\"` under [general], or `--theme NAME`.");
        return Ok(());
    }

    let config_path = cli.config.unwrap_or_else(Config::default_path);

    if let Some(path) = cli.render_check {
        return render_check::run(&path, &config_path, 1400, 900);
    }

    if let Some(path) = cli.import_iterm2 {
        import_iterm2(&config_path, &path)?;
        return Ok(());
    }

    if let Some(path) = cli.import_theme {
        import_theme(&config_path, &path)?;
        return Ok(());
    }

    let mut config = Config::load(&config_path)?;
    if let Some(theme) = cli.theme {
        config.set_theme(&theme)?;
    }
    let command = (!cli.execute.is_empty()).then(|| cli.execute.join(" "));

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("could not create the winit event loop")?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy, config_path, config, command);
    event_loop
        .run_app(&mut app)
        .context("event loop terminated with an error")?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn,winit=warn")
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn import_iterm2(config_path: &PathBuf, plist: &PathBuf) -> Result<()> {
    let imported = riterm_config::import_iterm2_prefs(plist)?;
    let mut config = Config::load(config_path).unwrap_or_default();
    for profile in &imported {
        println!("imported profile {:?}", profile.name);
        config
            .profiles
            .insert(profile.name.clone(), profile.profile.clone());
    }
    if let Some(first) = imported.first() {
        config.general.default_profile = first.name.clone();
    }
    config.save(config_path)?;
    println!(
        "wrote {} profile(s) to {}",
        imported.len(),
        config_path.display()
    );
    Ok(())
}

fn import_theme(config_path: &PathBuf, theme_path: &PathBuf) -> Result<()> {
    let colors = riterm_config::parse_itermcolors(theme_path)?;
    let imported = colors.to_profile();
    let mut config = Config::load(config_path)?;
    merge_profile(&mut config.profile, &imported);
    config.save(config_path)?;
    println!(
        "applied theme {} to the default profile in {}",
        theme_path.display(),
        config_path.display()
    );
    Ok(())
}

/// Copy only the fields the imported profile actually sets.
fn merge_profile(target: &mut Profile, source: &Profile) {
    macro_rules! copy {
        ($($field:ident),+ $(,)?) => {
            $(
                if source.$field.is_some() {
                    target.$field = source.$field.clone();
                }
            )+
        };
    }
    copy!(
        font_family,
        font_fallbacks,
        non_ascii_font_family,
        use_non_ascii_font,
        font_features,
        use_bold_font,
        use_italic_font,
        use_bright_bold,
        font_size,
        line_height,
        foreground,
        background,
        bold,
        cursor,
        cursor_text,
        selection,
        selection_text,
        link,
        cursor_guide,
        cursor_guide_enabled,
        badge,
        palette,
        cursor_shape,
        cursor_blink,
        scrollback,
        background_opacity,
        background_image,
        minimum_contrast,
        shell,
        working_directory,
    );
}

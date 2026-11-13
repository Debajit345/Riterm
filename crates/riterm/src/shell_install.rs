//! Install riterm's shell integration into bash, zsh, or fish.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use riterm_config::Config;

const MARKER: &str = "# riterm shell integration";

struct ShellScript {
    name: &'static str,
    file: &'static str,
    script: &'static str,
}

const SHELLS: &[ShellScript] = &[
    ShellScript {
        name: "bash",
        file: "riterm.bash",
        script: include_str!("../../../shell-integration/riterm.bash"),
    },
    ShellScript {
        name: "zsh",
        file: "riterm.zsh",
        script: include_str!("../../../shell-integration/riterm.zsh"),
    },
    ShellScript {
        name: "fish",
        file: "riterm.fish",
        script: include_str!("../../../shell-integration/riterm.fish"),
    },
];

/// Install integration scripts and hook them into the given shell's rc file.
/// `which` may be a shell name or `"all"`.
pub fn install(which: &str) -> Result<()> {
    let which = which.trim().to_ascii_lowercase();
    let selected: Vec<&ShellScript> = if which == "all" || which.is_empty() {
        SHELLS.iter().collect()
    } else {
        SHELLS.iter().filter(|shell| shell.name == which).collect()
    };
    if selected.is_empty() {
        anyhow::bail!("unknown shell {which:?}; expected bash, zsh, fish, or all");
    }

    let dir = Config::config_dir().join("shell-integration");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    for shell in selected {
        let path = dir.join(shell.file);
        std::fs::write(&path, shell.script)
            .with_context(|| format!("writing {}", path.display()))?;

        match rc_path(shell.name) {
            Some(rc) => {
                hook_rc(&rc, shell.name, &path)?;
                println!(
                    "Installed {} integration: {} (sourced from {})",
                    shell.name,
                    path.display(),
                    rc.display()
                );
            }
            None => {
                println!("Installed {} integration: {}", shell.name, path.display());
                println!("  Add this to your {} startup file:", shell.name);
                println!("    {}", source_line(shell.name, &path));
            }
        }
    }

    println!();
    println!("Start a new shell (or open a new tab) to activate it.");
    Ok(())
}

fn rc_path(shell: &str) -> Option<PathBuf> {
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    match shell {
        "bash" => Some(home.join(".bashrc")),
        "zsh" => Some(
            std::env::var_os("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or(home)
                .join(".zshrc"),
        ),
        "fish" => Some(home.join(".config").join("fish").join("config.fish")),
        _ => None,
    }
}

fn source_line(shell: &str, script: &Path) -> String {
    let path = script.display();
    match shell {
        "fish" => format!("test -f \"{path}\"; and source \"{path}\""),
        _ => format!(". \"{path}\""),
    }
}

fn hook_rc(rc: &Path, shell: &str, script: &Path) -> Result<()> {
    if let Some(parent) = rc.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let existing = std::fs::read_to_string(rc).unwrap_or_default();
    if existing.contains(MARKER) {
        // Already hooked; the script file itself was just refreshed above.
        return Ok(());
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(rc)
        .with_context(|| format!("opening {}", rc.display()))?;
    writeln!(file, "\n{MARKER}")?;
    writeln!(file, "{}", source_line(shell, script))?;
    Ok(())
}

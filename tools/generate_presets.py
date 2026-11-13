#!/usr/bin/env python3
"""Regenerate crates/riterm-config/src/presets.rs from iTerm2's plist.

Usage: python3 tools/generate_presets.py
"""

from __future__ import annotations

import json
import plistlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLIST = ROOT / "third_party" / "iterm2" / "plists" / "ColorPresets.plist"
OUT = ROOT / "crates" / "riterm-config" / "src" / "presets.rs"


def component(value: float | str) -> int:
    if isinstance(value, str):
        value = float(value)
    return max(0, min(255, round(value * 255)))


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def main() -> None:
    with PLIST.open("rb") as handle:
        presets = plistlib.load(handle)

    lines = [
        "//! Built-in iTerm2 color presets.",
        "//!",
        "//! Generated from iTerm2 `plists/ColorPresets.plist` at commit",
        "//! 2a8d67e06111e8246c6af16d87546060b572fb21 (2026-11-11). iTerm2 is",
        "//! GPL-3.0-or-later; see `third_party/iterm2/README.md` for provenance.",
        "//!",
        "//! Regenerate with `python3 tools/generate_presets.py`.",
        "",
        "use crate::colors::Rgb;",
        "",
        "/// A color preset bundled with iTerm2.",
        "pub struct BuiltinPreset {",
        "    pub name: &'static str,",
        "    /// iTerm2 color keys, including optional `(Dark)`/`(Light)` variants.",
        "    pub colors: &'static [(&'static str, Rgb)],",
        "}",
        "",
        "/// All built-in iTerm2 presets, in iTerm2's menu order.",
        "pub const BUILTIN_PRESETS: &[BuiltinPreset] = &[",
    ]

    for name, colors in presets.items():
        lines.extend(
            [
                "    BuiltinPreset {",
                f"        name: {rust_string(name)},",
                "        colors: &[",
            ]
        )
        for key in sorted(colors):
            value = colors[key]
            if not isinstance(value, dict) or "Red Component" not in value:
                continue
            red = component(value["Red Component"])
            green = component(value["Green Component"])
            blue = component(value["Blue Component"])
            lines.append(
                f"            ({rust_string(key)}, Rgb::new({red}, {green}, {blue})),"
            )
        lines.extend(["        ],", "    },"])

    lines.extend(
        [
            "];",
            "",
            "/// Look up a built-in preset by exact iTerm2 name.",
            "pub fn preset(name: &str) -> Option<&'static BuiltinPreset> {",
            "    BUILTIN_PRESETS.iter().find(|preset| preset.name == name)",
            "}",
            "",
            "/// Names accepted by the `theme` option, excluding imported files.",
            "pub fn names() -> impl Iterator<Item = &'static str> {",
            "    BUILTIN_PRESETS.iter().map(|preset| preset.name)",
            "}",
            "",
        ]
    )

    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)} ({len(presets)} presets)")


if __name__ == "__main__":
    main()

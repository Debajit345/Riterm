# iTerm2 source provenance

Riterm studies iTerm2's public source to match its defaults, color presets, and
configuration formats. iTerm2 is licensed under **GPL-3.0-or-later**; Riterm is
therefore also GPL-3.0-or-later. See the repository `LICENSE`.

- Upstream: <https://github.com/gnachman/iTerm2>
- Revision studied: `2a8d67e06111e8246c6af16d87546060b572fb21` (2026-11-11)
- Copied files:
  - `plists/ColorPresets.plist` — all 11 built-in color presets, including
    `(Dark)` and `(Light)` variants.
  - `plists/DefaultBookmark.plist` — the default profile's colors, fonts,
    cursor, and text defaults.

These files are copied verbatim for reproducibility. They are not modified by
Riterm at runtime, and Riterm never writes iTerm2 preference files.

`crates/riterm-config/src/presets.rs` is generated from
`plists/ColorPresets.plist` by `tools/generate_presets.py`. To regenerate:

```sh
python3 tools/generate_presets.py
```

The default profile values in `crates/riterm-config/src/lib.rs`
(`ResolvedProfile::default`) come from `plists/DefaultBookmark.plist`. The most
important values are:

| Setting | Dark value |
|---|---|
| Foreground | `#dcdcdc` |
| Background | `#15191f` |
| Bold | `#ffffff` |
| Cursor | `#ffffff` |
| Cursor text | `#000000` |
| Selection | `#b3d7ff` |
| Selected text | `#000000` |
| Link | `#328eee` |
| ANSI 0–15 | `#14191e #b43c2a #00c200 #c7c400 #2744c7 #c040be #00c5c7 #c7c7c7` `#686868 #dd7975 #58e790 #ece100 #a7abf2 #e17ee1 #60fdff #ffffff` |

iTerm2's default dark profile is identical to the built-in **Smoooooth**
preset. Riterm therefore defaults to those exact colors and can switch to any
bundled preset by name (`theme = "Solarized Dark"`).

iTerm2 is a trademark of its respective owners. Riterm is an independent
project and is not affiliated with or endorsed by iTerm2.

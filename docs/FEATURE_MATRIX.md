# Riterm × iTerm2 feature matrix

This matrix compares iTerm2's major feature areas with Riterm's current build.
It is a roadmap document, not a claim of parity. Status values:

- ✅ **Implemented** — works in the current build.
- 🟡 **Partial** — core behavior exists, but notable iTerm2 options are missing.
- ⏳ **Planned** — designed for a later milestone.
- ➖ **Not applicable** — macOS-specific or not meaningful on Linux as-is.

## Terminal emulation

| Feature | iTerm2 | Riterm |
|---|---|---|
| VT/ANSI parsing | Full xterm-compatible parser | ✅ via `alacritty_terminal`/`vte` |
| 256-color and true color | Yes | ✅ |
| Scrollback | Configurable, unlimited option | ✅ configurable `[general].scrollback` |
| Alternate screen | Yes | ✅ |
| Wide characters | Yes | ✅ wide cells and spacers |
| Unicode font fallback | Yes | ✅ primary → fallback chain → monospace |
| Separate non-ASCII font | Yes | ✅ `non_ascii_font_family` |
| Emoji / ZWJ clusters | Yes, with color emoji | 🟡 clusters and color emoji render; some sequences fall back to separate glyphs |
| RTL scripts | Yes, full bidi | 🟡 Arabic/Hebrew words are shaped as RTL runs and join; full paragraph bidi order is not implemented |
| OpenType features | Optional ligatures, stylistic sets | 🟡 feature tags are parsed and applied; ASCII grid runs still use basic shaping, so ligatures are not guaranteed |
| Bracketed paste | Yes | ✅ |
| Mouse reporting | X10, normal, button, any-event, SGR | ✅ modes inherited from `alacritty_terminal`; focused report encoding |
| Cursor shapes | Block, beam, underline, hollow | ✅ |
| Cursor blinking | Configurable | ✅ `cursor_blink`, iTerm2's 0.5 s interval; resets on input |
| Cursor text color | Yes | ✅ |
| Bold color / bright bold | Yes | ✅ default-foreground bold color and ANSI 0–7 brightening |
| Selected text color | Yes | ✅ |
| Minimum contrast | Yes | ✅ iTerm2's perceived-brightness algorithm |
| Cursor guide | Yes | 🟡 color parsed, guide rendering not exposed yet |
| Sixel / inline images | iTerm2 image protocol and sixel | ⏳ |
| OSC 8 hyperlinks | Yes | 🟡 parsed/rendered as text; click handling planned |
| OSC 52 clipboard | Yes | ✅ via terminal events |
| Title changes | Yes | ✅ |
| Bell / attention | Visual and audible options | 🟡 system attention request and tab dot; no sound or configurable visual flash |
| Sync output (DEC 2026) | Yes | ✅ inherited from the emulator |
| Terminal size reports | CSI 14/16/18 | ✅ `TextAreaSizeRequest` handled |

## Tabs and panes

| Feature | iTerm2 | Riterm |
|---|---|---|
| Tabs | Yes | ✅ |
| Recursive horizontal/vertical splits | Yes | ✅ |
| Focus by direction | Yes | ✅ |
| Pane zoom | Yes | 🟡 font zoom, not pane maximize |
| Tab reordering / drag | Yes | ⏳ |
| Tab colors, titles, icons | Yes | 🟡 live titles, activity/failure dots; no colors/icons |
| Tab bar position / auto-hide | Yes | ⏳ fixed top tab bar |
| Window arrangements / restore | Yes | ⏳ |
| tmux `-CC` integration | Yes | ⏳ protocol adapter planned |
| Broadcast input | Yes | ⏳ |

## Profiles and configuration

| Feature | iTerm2 | Riterm |
|---|---|---|
| Named profiles | Yes | ✅ |
| Profile inheritance | Yes | ✅ `[profile]` base + overrides |
| Font family, size, line height | Yes | ✅ |
| Font fallback chain | Yes | ✅ |
| Non-ASCII font | Yes | ✅ |
| Bold/italic font toggles | Yes | ✅ |
| OpenType feature list | Yes | 🟡 parsed and passed to shaping |
| ANSI palette, fg/bg/bold/cursor/selection/link | Yes | ✅ |
| Cursor text / selected text / cursor guide / badge colors | Yes | 🟡 parsed and applied except cursor guide/badge rendering |
| Built-in iTerm2 color presets | 11 | ✅ all 11, dark/light variants, `--list-themes` |
| Dark/light/auto appearance | Yes | ✅ `theme_variant` with `RITERM_THEME_VARIANT`/`COLORFGBG` |
| `.itermcolors` import | Yes | ✅ `--import-theme` |
| iTerm2 preferences import | Yes | ✅ `--import-iterm2` (`New Bookmarks`) |
| iTerm2 default profile values | Yes | ✅ Default Bookmark / Smoooooth colors |
| Live config reload | Yes | ⏳ restart required |
| Per-profile key mappings | Yes | 🟡 global `[keybindings]` only |
| Dynamic profiles | Yes | ⏳ |
| Automatic profile switching | Yes | ⏳ |
| Working directory policy | Yes | 🟡 fixed startup directory; OSC 7 parsed by scripts |
| Background image / transparency / blur | Yes | ⏳ |
| Minimum contrast | Yes | ✅ |
| Color preset UI | Yes | 🟡 CLI selection/import; no settings window |

## Shell integration

| Feature | iTerm2 | Riterm |
|---|---|---|
| OSC 133 A/B/C/D marks | Yes | ✅ |
| Command exit status | Yes | ✅ |
| Prompt marks stored per session | Yes | ✅ (bounded at 10k marks) |
| Tab running / failure indicator | Yes | ✅ |
| Current directory (OSC 7) | Yes | 🟡 scripts emit it; URL is not yet parsed into session state |
| bash/zsh/fish installer | Yes | ✅ `--install-shell-integration` |
| Marks navigation and selection | Yes | ⏳ |
| Per-command "copy output" | Yes | ⏳ |
| Utilities (`imgcat`, `it2copy`, `it2dl`, ...) | Yes | ⏳ |
| `it2ssh`, remote host integration | Yes | ⏳ |
| User vars / badges (OSC 1337) | Yes | ⏳ |

## Selection, clipboard, search

| Feature | iTerm2 | Riterm |
|---|---|---|
| Drag selection | Yes | ✅ |
| Copy / paste | Yes | ✅ |
| Copy on select | Optional | ✅ `[general].copy_on_select` |
| Block / semantic / line selection | Yes | ⏳ simple selection only |
| Middle-click paste | Configurable | ⏳ |
| Search scrollback | Yes | ⏳ |
| Copy mode (vi keys) | Yes | ⏳ |
| Autocomplete / suggestions | Yes | ⏳ |
| Word/URL click actions | Yes | ⏳ |

## Windows and desktop integration

| Feature | iTerm2 | Riterm |
|---|---|---|
| HiDPI scaling | Yes | ✅ per-monitor scale factor |
| Native fullscreen | Yes | 🟡 compositor-native default |
| Hotkey window (drop-down) | Yes | ⏳ |
| Window transparency | Yes | ⏳ |
| Tabs in separate windows | Yes | ⏳ |
| Notifications | Yes | 🟡 `request_user_attention` only |
| Session restoration | Yes | ⏳ |
| Multiple windows | Yes | ⏳ one window, many tabs |
| macOS Touch Bar / Services / Keychain / Sparkle | Yes | ➖ not applicable |
| AppleScript / Apple Events | Yes | ➖ replaced by a native scripting API (⏳) |

## Scripting and automation

| Feature | iTerm2 | Riterm |
|---|---|---|
| Python scripting API | Yes | ⏳ native API planned; no Python embedding |
| AppleScript | Yes | ➖ |
| Trigger actions (regex → action) | Yes | ⏳ |
| Annotations / notes | Yes | ⏳ |
| Status bar components | Yes | ⏳ |
| Badges | Yes | ⏳ |
| AI chat / Composer | Yes | ⏳ deferred |
| Browser panes | Yes | ⏳ deferred |
| Password manager | Yes | ⏳ deferred |

## Compatibility summary

The current build covers the daily-driver foundation and a meaningful slice of
iTerm2's appearance system: exact default colors, all built-in presets, font
fallback, bold/selection/cursor colors, minimum contrast, and blinking cursors.
The largest remaining gaps are search/copy mode, tmux control mode, triggers,
inline images, the status bar, hotkey windows, background effects, and
scripting. See [ROADMAP.md](ROADMAP.md) for sequencing.

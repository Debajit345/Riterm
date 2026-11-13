# Roadmap

The goal is not a line-by-line port of iTerm2. It is a terminal that feels like
iTerm2 for the workflows Linux users actually have, with iTerm2 protocols and
configuration as the compatibility layer.

## Milestone 1 — MVP foundation (complete)

- [x] Workspace, GPL-3.0-or-later licensing, docs
- [x] PTY sessions, VT emulation, scrollback, true color
- [x] GPU renderer (wgpu + glyphon), cursor shapes, selection
- [x] Tabs, recursive splits, directional focus, font zoom
- [x] Profiles and inheritance
- [x] `.itermcolors` and iTerm2 profile import
- [x] OSC 133 shell integration scanner, command state, tab indicators
- [x] bash/zsh/fish installer and OSC 7 reporting
- [x] Clipboard, mouse selection/reporting, bracketed paste
- [x] CLI: `--config`, `-e`, importers, shell installer
- [x] Unit tests and a real PTY integration test
- [x] All 11 built-in iTerm2 color presets, exact default profile colors
- [x] Font fallback chain, separate non-ASCII font, emoji, wide characters
- [x] RTL shaping for Arabic/Hebrew runs
- [x] Bold color, bright-bold ANSI, cursor/selection text colors
- [x] iTerm2 minimum-contrast algorithm
- [x] Blinking cursor with configurable shape and blink toggle
- [x] Deterministic offscreen `--render-check` for visual regression checks

Remaining MVP limitations:

- ASCII grid runs use basic shaping, so ligatures are not guaranteed
- One window; no session restore
- Fixed top tab bar; no drag-reorder
- No inline images

## Milestone 2 — daily-driver polish (next)

Target: replace a normal terminal for everyday work.

- [x] Search scrollback with incremental highlighting
- [x] Copy mode with vi-style navigation and keyboard selection
- [x] Scrollbar / scroll position indicator
- [x] Cursor blinking (`cursor_blink`, iTerm2's 0.5 s phase)
- [ ] Per-profile key mappings and modifier remapping
- [ ] Window transparency, background image, blur (where compositor allows)
- [ ] Multiple windows and session restore
- [ ] Tab drag-reorder, tab colors, rename, close buttons
- [ ] Hotkey window (drop-down) via compositor keybind + layer surface
- [x] Config hot reload (file polling)
- [ ] Better PTY I/O: `poll(2)`/`epoll` instead of sleep polling
- [x] Cell-accurate fallback for missing glyphs; color emoji renders
- [x] OSC 8 link hit-testing and open-in-browser
- [x] Word/URL selection and middle-click paste
- [x] Desktop notifications for command finish and BEL

## Milestone 3 — iTerm2 power features

Target: the features that make iTerm2 distinctive.

- [ ] tmux `-CC` control-mode client (windows, panes, layouts, titles)
- [x] OSC 133 marks UI: jump to previous/next prompt, select command output
- [x] Latest-command "copy output"
- [ ] Per-command history UI, "re-run", exit-status actions
- [x] Triggers: regex match → notify or tab bell
- [ ] Triggers: highlight, annotate, run command
- [ ] Inline images: iTerm2 image protocol, then sixel
- [ ] Status bar with components (cwd, git, command status, clock)
- [ ] Badges and user variables (OSC 1337 SetUserVar)
- [ ] Shell utilities: `imgcat`, `it2copy`, `it2dl`, `it2getvar`
- [ ] Semantic/block selection
- [ ] Automatic profile switching (host, user, directory)
- [ ] Dynamic profiles in a separate directory
- [ ] Broadcast input across panes/tabs
- [ ] Password manager integration (libsecret, optional)

## Milestone 4 — platform and automation

Target: a platform, not just an app.

- [ ] Native scripting API (JSON-RPC over a Unix socket) with a stable schema
- [ ] CLI client for scripting and window/pane control
- [ ] Plugin host (Lua or WASM) for custom status bar components and triggers
- [ ] D-Bus interfaces for desktop integration
- [ ] Browser panes (WebKitGTK) — optional build feature
- [ ] AI assistant / Composer equivalents — optional, privacy-first
- [ ] Accessibility: screen-reader text model, high-contrast themes
- [ ] Packaged builds: Arch/AUR, Flatpak, Nix
- [ ] Session crash recovery and scrollback persistence

## Explicit non-goals

- Bit-for-bit UI parity with macOS AppKit behavior
- AppleScript, Touch Bar, Keychain, Sparkle, macOS Services
- Running macOS-only iTerm2 plugins unchanged
- Proprietary components or non-GPL linking

## Compatibility principles

1. Prefer public protocols (OSC 133, OSC 7, OSC 52, tmux control mode) over
   imitation.
2. Import iTerm2 configuration; never write back to iTerm2 files.
3. Keep `TERM_PROGRAM=iTerm2` behind a profile option if it causes tool
   conflicts; `RITERM=1` is always the reliable identity.
4. Any code translated from iTerm2 remains GPL-3.0-or-later.

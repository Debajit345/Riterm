# Architecture

Riterm is a Cargo workspace with three crates:

```text
riterm/
├── crates/
│   ├── riterm-config/   TOML config, profiles, iTerm2 importers, presets
│   ├── riterm-core/     PTY sessions, VT state, OSC 133 scanner
│   └── riterm/          winit app, wgpu renderer, search/copy mode, CLI
├── shell-integration/   bash/zsh/fish integration scripts
├── third_party/iterm2/  copied iTerm2 plists and provenance
├── tools/               preset generator
└── docs/
```

## Crates

### `riterm-config`

- `lib.rs` — `Config`, `General`, `Profile`, `ResolvedProfile`, loading/saving,
  profile inheritance, and the default commented config. The default profile
  mirrors iTerm2's Default Bookmark in dark mode.
- `colors.rs` — `Rgb` parsing/serialization and color mixing.
- `iterm.rs` — importers for `.itermcolors` presets and iTerm2 preference
  plists (`New Bookmarks`), including iTerm2 font-name parsing, light/dark
  color variants, cursor shape, scrollback, and working directories.
- `presets.rs` — all 11 built-in iTerm2 presets, generated from iTerm2's
  `ColorPresets.plist` by `tools/generate_presets.py`.

Configuration is deliberately a plain TOML tree. A named profile is resolved by
starting from the built-in defaults, applying an optional theme, then
`[profile]`, then `[profiles.NAME]`, and finally filling shell and working
directory from `[general]`.

Themes can be a built-in preset name or an `.itermcolors` path. Presets carry
base, `(Dark)`, and `(Light)` color sets. `ThemeMode` selects dark, light, or
auto; auto consults `RITERM_THEME_VARIANT`, then `COLORFGBG`, then dark.

The app polls the config file's modification time once a second while the
event loop is idle. A successful reload replaces the config and rebuilt
keybindings, marks every session dirty, and redraws; a malformed file leaves
the running configuration untouched. Shell and scrollback changes apply to
new sessions, while appearance changes apply immediately.

### `riterm-core`

- `session.rs` — owns a `Pty`, a `Term<TermProxy>` from
  `alacritty_terminal`, a `vte::ansi::Processor`, and a background I/O thread.
  The thread reads PTY bytes and writes queued input; events are delivered to
  the UI through a `SessionSink` closure.
- `shell_integration.rs` — incremental OSC 133 scanner. It is independent of
  the VT parser and returns the end offset of each complete mark, so `feed()`
  can advance the emulator in stream order and record the mark at the correct
  cursor position.
- `size.rs` — the `Dimensions` implementation used by `Term::new` and
  `Term::resize`.

`Session::feed` scans bytes for marks, feeds everything up to and including each
mark to the VT parser, records the mark at the resulting cursor position, then
feeds the remaining bytes. This keeps marks anchored to the prompt even when
escape sequences are split across PTY reads.

### `riterm`

- `main.rs` — clap CLI, tracing setup, event loop creation, theme selection,
  import and shell-integration subcommands.
- `app.rs` — `ApplicationHandler<UserEvent>`: tabs, split trees, focus, mouse
  selection/reporting, clipboard, session events, cursor blink scheduling, and
  drawing.
- `layout.rs` — `PaneNode` split tree, rectangle layout, split/remove, tests.
- `input.rs` — default keybindings, chord canonicalization, action lookup, key
  encoding for the PTY (application cursor keys, CSI modifiers, function keys),
  bracketed paste.
- `search.rs` — case-insensitive scrollback search. It folds each grid cell
  to lowercase, maps matches back to absolute grid points (including negative
  history lines), and provides vi-style word motions for copy mode.
- `render.rs` — wgpu device/surface, quad pipeline, glyphon text atlas, font
  family resolution and metrics caches, segment shaping, cell
  background/selection/search-highlight/cursor rendering, tab bar, and the
  search/copy overlay line.
- `quads.wgsl` — minimal instanced-free quad shader.
- `shell_install.rs` — installs scripts and hooks rc files.

## Event flow

```text
keyboard ──► App::handle_key ──► Action or encode_key ──► Session::write
                                                              │
                                                              ▼
                                                       PTY write channel
                                                              │
                                                              ▼
                                                        child process
                                                              │
                                                              ▼
                                             PTY reader thread (SessionSink)
                                                              │
                                              EventLoopProxy<UserEvent>
                                                              ▼
                                            App::user_event ──► Session::feed
                                                              │
                                                              ▼
                                      vte::ansi::Processor ──► Term
                                                              │
                                                              ▼
                                                     WindowEvent::Redraw
```

`Session::feed` also updates `command_state` (`Idle`, `Running`,
`Finished(code)`) and `last_exit_code` from OSC 133, plus a globally monotonic
`command_finished_epoch`. The app acknowledges the newest epoch per tab, so a
failure is still reported when bash emits `D;1` and the next `A` in the same
read (the `A` resets the live state to `Idle`).

## Rendering

Each frame:

1. `Renderer::cancel_frame` clears per-frame quads and text areas.
2. For every visible pane, `prepare_session` computes the grid size from the
   pane rectangle and font metrics, resizes the PTY/terminal if needed, and
   rebuilds the pane's segments only when the session is dirty, metrics/size
   changed, or cursor visibility changed.
3. Cell backgrounds, selection, cursor shapes, dividers, and the tab bar are
   pushed as colored quads.
4. Quads are uploaded to a vertex buffer and drawn with the `quads.wgsl`
   pipeline; glyphon prepares text areas and renders them in the same pass.
5. `queue.submit` and `queue.present`.

### Fonts

The renderer resolves a profile's primary family by checking the configured
family and then each fallback against `fontdb`, ending at generic monospace.
The separate non-ASCII family is used when enabled. Family resolution and font
metrics are cached by a hash of the family list, features, size, line height,
and scale factor. Cell height is the configured line height; cell width is
measured by shaping twenty `M` glyphs and dividing.

### Segments and shaping

A terminal row is split into independently positioned segments:

- ASCII runs use `Shaping::Basic` and start at the first cell of the run.
- A non-ASCII cluster (base character plus zero-width marks) is shaped with
  `Shaping::Advanced` in a buffer exactly one or two cells wide. This keeps a
  fallback font from shifting later columns.
- Right-to-left scripts (Arabic, Hebrew, and related ranges) are grouped into a
  run so letters join and the run is right-aligned in its cells. Full paragraph
  bidi reordering is not implemented.

Each segment records its row, starting column, cell width, and x offset, and
gets its own `TextArea` and clipping bounds. The cursor is drawn as a quad;
block-cursor text is recolored with `Cursor Text Color` and selected text with
`Selected Text Color`.

Search hits are drawn as translucent selection-colored quads before the text
pass; the current hit is also installed as the terminal selection so it uses
the exact selection and selected-text colors. Copy mode draws a hollow cursor
at its vi-style position and reuses the terminal selection while an anchor is
active. When the grid has scrollback history, a thin scrollbar track and thumb
are drawn at the right edge of the pane.

The optional cursor guide is a one-pixel horizontal line in the profile's
guide color across the cursor's row. OSC 8 hyperlinks are stored by the
terminal core in each cell and rendered in the profile's link color with an
underline. `Ctrl+click` resolves the cell's
hyperlink first, then scans the line for an `http(s)://`, `ftp://`, or
`file://` URL, and opens it with `xdg-open`. Middle-click paste and
double/triple-click word/line selection are handled in the mouse path.

Ligatures are not forced on: ASCII runs use basic shaping so columns stay
aligned. Feature tags in `font_features` are passed to shaping, but ligature
rendering remains dependent on the shaping mode and is listed as partial in the
feature matrix.

### Colors

The palette is resolved in this order: the cell's explicit color, the terminal
colors table, then the profile palette. Riterm implements iTerm2's semantics
for:

- bold color, applied only to bold default-foreground text;
- bright-bold ANSI colors 0–7;
- cursor and cursor-text colors;
- selection and selected-text colors;
- minimum contrast, using iTerm2's perceived-brightness formula
  (`0.30 R + 0.59 G + 0.11 B`) and its black/white adjustment.

Frames render into **non-sRGB unorm targets** (`Rgba8Unorm` offscreen,
`Bgra8Unorm`/`Rgba8Unorm` surfaces where available). Palette values are sRGB
bytes and are written directly; choosing an `*Srgb` target would make wgpu
linearise them a second time and wash out every color.

### Cursor blinking

The app keeps a 0.5 s blink phase and schedules `ControlFlow::WaitUntil`. The
phase resets to visible on key input and focus gain. The renderer is told the
current visibility, and a pane rebuilds when visibility changes so block-cursor
text follows the cursor.

## Threading

- UI and rendering run on the winit main thread.
- Each session has one I/O thread that polls the PTY with a short sleep when
  idle. Reads are sent to the UI thread; writes go through an mpsc channel.
- Dropping a `Session` sets a stop flag, joins the I/O thread, and drops the
  `Pty`, which closes the master side and sends SIGHUP to the child.

The current I/O loop uses non-blocking reads with a 2 ms sleep rather than
`poll(2)` registration. This is simple and correct but trades CPU for
simplicity; replacing it with `polling`/`epoll` is a planned optimization.

## Compatibility choices

- The terminal type is `xterm-256color`; `COLORTERM=truecolor`.
- `TERM_PROGRAM=iTerm2` and `LC_TERMINAL=iTerm2` are exported so tools that
  detect iTerm2 enable their integrations. `RITERM=1` identifies Riterm itself.
- OSC 133 is the only shell-integration protocol interpreted by the core.
  OSC 1337 features (badges, user vars, inline images) are not implemented yet.
- iTerm2 plists are read, never modified. Import writes native TOML.

## Testing

- `riterm-core`: OSC 133 scanner tests (split sequences, ST vs BEL, exit codes)
  and a real PTY integration test that spawns `/bin/sh`, collects output, feeds
  it into the terminal, and asserts the grid content.
- `riterm-config`: color parsing, preset variants, theme resolution, profile
  merging, and font-name parsing.
- `riterm` (binary crate): split tree layout, key encoding, Unicode shaping,
  and command-failure bookkeeping.
- `--render-check` writes a deterministic offscreen PNG that exercises presets,
  styles, fallback glyphs, wide characters, RTL, emoji, and the tab bar.

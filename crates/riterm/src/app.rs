//! Window/application state: tabs, splits, input, and session events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{TermMode, viewport_to_point};
use alacritty_terminal::vte::ansi::Rgb as AnsiRgb;
use anyhow::Result;
use riterm_config::{Config, ResolvedProfile, Rgb, expand_home};
use riterm_core::{Session, SessionConfig, SessionEvent, SessionSink};
use tracing::{debug, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, ModifiersState};
use winit::window::{UserAttentionType, Window};

use crate::input::{Action, action_for, default_bindings, encode_key, paste_bytes};
use crate::layout::{PaneNode, Rect, SplitDir};
use crate::render::{Renderer, TabInfo};

#[cfg(all(unix, not(target_os = "macos")))]
use winit::platform::wayland::WindowAttributesExtWayland;

/// Events delivered from session threads to the UI loop.
#[derive(Debug)]
pub enum UserEvent {
    Session(u64, SessionEvent),
}

pub struct Tab {
    pub layout: PaneNode,
    pub sessions: HashMap<u64, Session>,
    pub active: u64,
    pub profile: String,
    pub zoom: f32,
    pub bell: bool,
    /// Highest command-finished epoch already accounted for in this tab.
    pub bell_epoch: u64,
}

impl Tab {
    /// Acknowledge newly finished commands and report whether any of them
    /// failed. This checks `last_exit_code` rather than `command_state`, because
    /// bash's next `A` mark resets the state to `Idle` before the UI may look.
    fn observe_command_failures(&mut self) -> bool {
        observe_finished_commands(
            &mut self.bell_epoch,
            self.sessions
                .values()
                .map(|session| (session.command_finished_epoch, session.last_exit_code)),
        )
    }
}

/// Advance `acknowledged` past finished commands and return whether any of the
/// newly observed commands exited nonzero.
fn observe_finished_commands(
    acknowledged: &mut u64,
    finished: impl Iterator<Item = (u64, Option<i32>)>,
) -> bool {
    let previously_acknowledged = *acknowledged;
    let mut latest = previously_acknowledged;
    let mut failed = false;
    for (epoch, code) in finished {
        latest = latest.max(epoch);
        if epoch > previously_acknowledged && code.is_some_and(|code| code != 0) {
            failed = true;
        }
    }
    *acknowledged = latest;
    failed
}

struct DragSelection {
    session_id: u64,
    start: Point,
}

pub struct App {
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    config_path: PathBuf,
    config: Config,
    command: Option<String>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    tabs: Vec<Tab>,
    active_tab: usize,
    next_session_id: u64,
    bindings: HashMap<String, Action>,
    modifiers: ModifiersState,
    clipboard: Option<arboard::Clipboard>,
    mouse_position: PhysicalPosition<f64>,
    drag: Option<DragSelection>,
    focused: bool,
    scale: f64,
    current_title: String,
    cursor_blink_on: bool,
    cursor_blink_next: Instant,
}

impl App {
    pub fn new(
        proxy: winit::event_loop::EventLoopProxy<UserEvent>,
        config_path: PathBuf,
        config: Config,
        command: Option<String>,
    ) -> Self {
        let mut bindings = default_bindings();
        for (chord, name) in &config.keybindings {
            match Action::from_name(name) {
                Some(action) => {
                    bindings.insert(chord.to_lowercase(), action);
                }
                None => warn!("unknown action {name:?} for keybinding {chord:?}"),
            }
        }

        Self {
            proxy,
            config_path,
            config,
            command,
            window: None,
            renderer: None,
            tabs: Vec::new(),
            active_tab: 0,
            next_session_id: 1,
            bindings,
            modifiers: ModifiersState::empty(),
            clipboard: None,
            mouse_position: PhysicalPosition::new(0.0, 0.0),
            drag: None,
            focused: true,
            scale: 1.0,
            current_title: String::new(),
            cursor_blink_on: true,
            cursor_blink_next: Instant::now() + Duration::from_millis(500),
        }
    }

    fn tab_bar_height(&self) -> f32 {
        self.config.general.tab_bar_height * self.scale as f32
    }

    fn restart_cursor_blink(&mut self) {
        self.cursor_blink_on = true;
        self.cursor_blink_next = Instant::now() + Duration::from_millis(500);
    }

    fn content_rect(&self) -> Rect {
        let Some(renderer) = &self.renderer else {
            return Rect::new(0.0, 0.0, 800.0, 600.0);
        };
        let (w, h) = renderer.surface_size();
        let bar = self.tab_bar_height().min(h as f32);
        Rect::new(0.0, bar, w as f32, (h as f32 - bar).max(1.0))
    }

    fn profile_for_index(&self, index: usize) -> ResolvedProfile {
        let name = self
            .tabs
            .get(index)
            .map(|t| t.profile.as_str())
            .unwrap_or(self.config.general.default_profile.as_str());
        let mut profile = self.config.resolve(name);
        if let Some(tab) = self.tabs.get(index) {
            profile.font_size = (profile.font_size * tab.zoom).clamp(4.0, 96.0);
        }
        profile
    }

    fn pane_rects(&self, tab: &Tab) -> (Vec<(u64, Rect)>, Vec<Rect>) {
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        tab.layout
            .layout(self.content_rect(), 1.0, &mut panes, &mut dividers);
        (panes, dividers)
    }

    fn create_tab(&mut self) {
        let profile_name = self.config.general.default_profile.clone();
        let profile = self.config.resolve(&profile_name);
        let (cell_w, cell_h) = match self.renderer.as_mut() {
            Some(renderer) => renderer.cell_size(&profile),
            None => (8.0, 16.0),
        };
        let content = self.content_rect();
        let padding = self.config.general.padding * self.scale as f32;
        let inner = content.inset(padding);
        let cols = ((inner.w / cell_w).floor() as usize).max(1);
        let rows = ((inner.h / cell_h).floor() as usize).max(1);

        match self.spawn_session(&profile, cols, rows, cell_w, cell_h) {
            Ok((session_id, session)) => {
                let tab = Tab {
                    layout: PaneNode::leaf(session_id),
                    sessions: HashMap::from([(session_id, session)]),
                    active: session_id,
                    profile: profile_name,
                    zoom: 1.0,
                    bell: false,
                    bell_epoch: 0,
                };
                self.tabs.push(tab);
                self.active_tab = self.tabs.len() - 1;
                self.update_title();
                self.request_redraw();
            }
            Err(err) => {
                tracing::error!("could not start shell: {err:#}");
            }
        }
    }

    fn spawn_session(
        &mut self,
        profile: &ResolvedProfile,
        cols: usize,
        rows: usize,
        cell_w: f32,
        cell_h: f32,
    ) -> Result<(u64, Session)> {
        let id = self.next_session_id;
        self.next_session_id += 1;

        let (shell, args) = if let Some(command) = &self.command {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            (Some(shell), vec!["-lc".to_string(), command.clone()])
        } else {
            (
                profile.shell.clone(),
                self.config.general.shell_args.clone(),
            )
        };

        let working_directory = profile
            .working_directory
            .as_deref()
            .or(self.config.general.working_directory.as_deref())
            .map(expand_home);

        let config = SessionConfig {
            shell,
            args,
            working_directory,
            env: HashMap::new(),
            scrollback: self.config.general.scrollback,
            cols,
            rows,
            cell_width: cell_w.round().max(1.0) as u16,
            cell_height: cell_h.round().max(1.0) as u16,
            session_id: format!("w0t0p{id}"),
        };

        let proxy = self.proxy.clone();
        let sink: SessionSink = Arc::new(move |event| {
            let _ = proxy.send_event(UserEvent::Session(id, event));
        });

        let session = Session::spawn(id, config, sink)?;
        Ok((id, session))
    }

    fn split_active(&mut self, dir: SplitDir) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let active = tab.active;
        let profile_name = tab.profile.clone();
        let panes = self.pane_rects(tab).0;
        let Some(rect) = panes
            .iter()
            .find(|(id, _)| *id == active)
            .map(|(_, rect)| *rect)
        else {
            return;
        };

        let profile = self.profile_for_index(self.active_tab);
        let (cell_w, cell_h) = match self.renderer.as_mut() {
            Some(renderer) => renderer.cell_size(&profile),
            None => (8.0, 16.0),
        };
        let padding = self.config.general.padding * self.scale as f32;
        let inner = rect.inset(padding);
        let cols = ((inner.w / 2.0 / cell_w).floor() as usize).max(1);
        let rows = ((inner.h / 2.0 / cell_h).floor() as usize).max(1);

        match self.spawn_session(&profile, cols, rows, cell_w, cell_h) {
            Ok((session_id, session)) => {
                let tab = &mut self.tabs[self.active_tab];
                if tab.layout.split_leaf(active, session_id, dir) {
                    tab.sessions.insert(session_id, session);
                    tab.active = session_id;
                    tab.profile = profile_name;
                    self.mark_all_dirty();
                    self.request_redraw();
                }
            }
            Err(err) => tracing::error!("could not split pane: {err:#}"),
        }
    }

    fn close_active_pane(&mut self, event_loop: &ActiveEventLoop) {
        if self.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        let tab_index = self.active_tab;
        let tab = &mut self.tabs[tab_index];
        let target = tab.active;
        let new_layout = tab.layout.remove_leaf(target);
        tab.sessions.remove(&target);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.remove_session(target);
        }

        match new_layout {
            Some(layout) => {
                tab.layout = layout;
                tab.active = tab.layout.first_leaf();
                self.mark_all_dirty();
            }
            None => {
                self.tabs.remove(tab_index);
                if self.tabs.is_empty() {
                    event_loop.exit();
                    return;
                }
                self.active_tab = self.active_tab.min(self.tabs.len() - 1);
                self.mark_all_dirty();
            }
        }
        self.update_title();
        self.request_redraw();
    }

    fn close_session(&mut self, id: u64, _event_loop: &ActiveEventLoop) {
        if let Some(index) = self.tabs.iter().position(|t| t.sessions.contains_key(&id)) {
            self.active_tab = index;
            self.tabs[index].active = id;
        }
    }

    fn next_tab(&mut self) {
        if self.tabs.len() > 1 {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.tabs[self.active_tab].bell = false;
            self.mark_all_dirty();
            self.update_title();
            self.request_redraw();
        }
    }

    fn prev_tab(&mut self) {
        if self.tabs.len() > 1 {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
            self.tabs[self.active_tab].bell = false;
            self.mark_all_dirty();
            self.update_title();
            self.request_redraw();
        }
    }

    fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
            self.tabs[index].bell = false;
            self.mark_all_dirty();
            self.update_title();
            self.request_redraw();
        }
    }

    fn focus_direction(&mut self, dx: f32, dy: f32) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let panes = self.pane_rects(tab).0;
        let Some((active, active_rect)) = panes.iter().find(|(id, _)| *id == tab.active) else {
            return;
        };
        let ax = active_rect.x + active_rect.w / 2.0;
        let ay = active_rect.y + active_rect.h / 2.0;
        let mut best: Option<(u64, f32)> = None;
        for (id, rect) in &panes {
            if id == active {
                continue;
            }
            let cx = rect.x + rect.w / 2.0;
            let cy = rect.y + rect.h / 2.0;
            let in_direction = (dx > 0.0 && cx > ax)
                || (dx < 0.0 && cx < ax)
                || (dy > 0.0 && cy > ay)
                || (dy < 0.0 && cy < ay);
            if !in_direction {
                continue;
            }
            let distance = (cx - ax).abs() + (cy - ay).abs();
            if best.map(|(_, d)| distance < d).unwrap_or(true) {
                best = Some((*id, distance));
            }
        }
        if let Some((id, _)) = best {
            self.tabs[self.active_tab].active = id;
            self.mark_all_dirty();
            self.request_redraw();
        }
    }

    fn mark_all_dirty(&mut self) {
        for tab in &mut self.tabs {
            for session in tab.sessions.values_mut() {
                session.dirty = true;
            }
        }
    }

    fn active_session(&self) -> Option<&Session> {
        let tab = self.tabs.get(self.active_tab)?;
        tab.sessions.get(&tab.active)
    }

    fn active_session_mut(&mut self) -> Option<&mut Session> {
        let tab = self.tabs.get_mut(self.active_tab)?;
        let id = tab.active;
        tab.sessions.get_mut(&id)
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn update_title(&mut self) {
        let title = self
            .active_session()
            .map(|s| s.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Riterm".to_string());
        let title = format!("{title} — riterm");
        if title != self.current_title {
            if let Some(window) = &self.window {
                window.set_title(&title);
            }
            self.current_title = title;
        }
    }

    fn set_clipboard(&mut self, text: String) {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        if let Some(clipboard) = self.clipboard.as_mut() {
            if let Err(err) = clipboard.set_text(text) {
                warn!("clipboard write failed: {err}");
            }
        }
    }

    fn get_clipboard(&mut self) -> Option<String> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard
            .as_mut()
            .and_then(|clipboard| clipboard.get_text().ok())
    }

    fn copy_selection(&mut self) {
        let text = self
            .active_session()
            .and_then(|session| session.term.selection_to_string());
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.set_clipboard(text);
        }
    }

    fn paste(&mut self) {
        let Some(text) = self.get_clipboard() else {
            return;
        };
        if let Some(session) = self.active_session_mut() {
            session.scroll_to_bottom();
            let bracketed = session.term.mode().contains(TermMode::BRACKETED_PASTE);
            session.write(paste_bytes(&text, bracketed));
        }
    }

    fn perform_action(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        match action {
            Action::NewTab => self.create_tab(),
            Action::ClosePane => self.close_active_pane(event_loop),
            Action::NextTab => self.next_tab(),
            Action::PrevTab => self.prev_tab(),
            Action::SelectTab(index) => self.select_tab(index),
            Action::SplitSideBySide => self.split_active(SplitDir::SideBySide),
            Action::SplitStacked => self.split_active(SplitDir::Stacked),
            Action::FocusLeft => self.focus_direction(-1.0, 0.0),
            Action::FocusRight => self.focus_direction(1.0, 0.0),
            Action::FocusUp => self.focus_direction(0.0, -1.0),
            Action::FocusDown => self.focus_direction(0.0, 1.0),
            Action::Copy => self.copy_selection(),
            Action::Paste => self.paste(),
            Action::ZoomIn => self.zoom_active(0.1),
            Action::ZoomOut => self.zoom_active(-0.1),
            Action::ZoomReset => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.zoom = 1.0;
                }
                self.mark_all_dirty();
                self.request_redraw();
            }
            Action::ScrollPageUp => self.scroll_active(10),
            Action::ScrollPageDown => self.scroll_active(-10),
            Action::ScrollTop => self.scroll_active(1_000_000),
            Action::ScrollBottom => self.scroll_active(-1_000_000),
            Action::ClearScrollback => {
                if let Some(session) = self.active_session_mut() {
                    session.clear_scrollback();
                }
                self.request_redraw();
            }
        }
    }

    fn zoom_active(&mut self, delta: f32) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.zoom = (tab.zoom + delta).clamp(0.5, 3.0);
        }
        self.mark_all_dirty();
        self.request_redraw();
    }

    fn scroll_active(&mut self, lines: i32) {
        if let Some(session) = self.active_session_mut() {
            session.scroll(lines);
        }
        self.request_redraw();
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, event: &winit::event::KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.modifiers.super_key() {
            return;
        }

        self.restart_cursor_blink();

        if let Some(action) = action_for(&self.bindings, self.modifiers, &event.logical_key) {
            self.perform_action(action, event_loop);
            return;
        }

        let mode = self
            .active_session()
            .map(|session| *session.term.mode())
            .unwrap_or(TermMode::empty());
        let Some(bytes) = encode_key(&event.logical_key, self.modifiers, mode) else {
            return;
        };
        if let Some(session) = self.active_session_mut() {
            session.scroll_to_bottom();
            session.write(bytes);
        }
        self.request_redraw();
    }

    fn handle_term_event(&mut self, id: u64, event: TermEvent, event_loop: &ActiveEventLoop) {
        match event {
            TermEvent::Title(title) => {
                if let Some(session) = self.find_session_mut(id) {
                    session.title = title;
                }
                self.update_title();
            }
            TermEvent::ResetTitle => {
                if let Some(session) = self.find_session_mut(id) {
                    session.title.clear();
                }
                self.update_title();
            }
            TermEvent::PtyWrite(text) => {
                if let Some(session) = self.find_session_mut(id) {
                    session.write(text.into_bytes());
                }
            }
            TermEvent::ClipboardStore(_, text) => self.set_clipboard(text),
            TermEvent::ClipboardLoad(_, formatter) => {
                if let Some(text) = self.get_clipboard() {
                    if let Some(session) = self.find_session_mut(id) {
                        session.write(formatter(&text).into_bytes());
                    }
                }
            }
            TermEvent::ColorRequest(index, formatter) => {
                let profile = self.profile_for_index(self.active_tab);
                let rgb = color_for_index(index, &profile);
                if let Some(session) = self.find_session_mut(id) {
                    session.write(
                        formatter(AnsiRgb {
                            r: rgb.r,
                            g: rgb.g,
                            b: rgb.b,
                        })
                        .into_bytes(),
                    );
                }
            }
            TermEvent::TextAreaSizeRequest(formatter) => {
                let profile = self.profile_for_index(self.active_tab);
                let (cell_w, cell_h) = match self.renderer.as_mut() {
                    Some(renderer) => renderer.cell_size(&profile),
                    None => (8.0, 16.0),
                };
                if let Some(session) = self.find_session_mut(id) {
                    let window_size = WindowSize {
                        num_lines: session.size.rows as u16,
                        num_cols: session.size.cols as u16,
                        cell_width: cell_w.round().max(1.0) as u16,
                        cell_height: cell_h.round().max(1.0) as u16,
                    };
                    session.write(formatter(window_size).into_bytes());
                }
            }
            TermEvent::Bell => {
                if let Some(index) = self.tabs.iter().position(|t| t.sessions.contains_key(&id)) {
                    self.tabs[index].bell = true;
                }
                if let Some(window) = &self.window {
                    window.request_user_attention(Some(UserAttentionType::Informational));
                }
                self.request_redraw();
            }
            TermEvent::Exit => self.close_session(id, event_loop),
            TermEvent::ChildExit(_) => {
                if let Some(session) = self.find_session_mut(id) {
                    session.exited = true;
                }
                self.request_redraw();
            }
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
                self.request_redraw()
            }
        }
    }

    fn find_session_mut(&mut self, id: u64) -> Option<&mut Session> {
        for tab in &mut self.tabs {
            if let Some(session) = tab.sessions.get_mut(&id) {
                return Some(session);
            }
        }
        None
    }

    fn draw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        let profile = self.profile_for_index(self.active_tab);
        let tab_bar_h = self.tab_bar_height().min(size.height as f32);
        let tab_bar = Rect::new(0.0, 0.0, size.width as f32, tab_bar_h);
        let content = Rect::new(
            0.0,
            tab_bar_h,
            size.width as f32,
            (size.height as f32 - tab_bar_h).max(1.0),
        );

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.resize(size.width, size.height);
        renderer.set_scale(self.scale);
        renderer.set_cursor_visible(!profile.cursor_blink || self.cursor_blink_on);
        renderer.cancel_frame();

        if self.tabs.is_empty() {
            let _ = renderer.render(Rgb::BLACK);
            return;
        }

        let infos: Vec<TabInfo> = self
            .tabs
            .iter()
            .map(|tab| {
                let title = tab
                    .sessions
                    .get(&tab.active)
                    .map(|session| session.title.clone())
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| "Terminal".to_string());
                let running = tab
                    .sessions
                    .values()
                    .any(|session| session.command_state == riterm_core::CommandState::Running);
                TabInfo {
                    title,
                    active: false,
                    bell: tab.bell,
                    running,
                }
            })
            .map(|mut info| {
                info.active = false;
                info
            })
            .collect();

        let mut infos = infos;
        if let Some(info) = infos.get_mut(self.active_tab) {
            info.active = true;
        }

        let (panes, dividers) = {
            let tab = &self.tabs[self.active_tab];
            let mut panes = Vec::new();
            let mut dividers = Vec::new();
            tab.layout.layout(content, 1.0, &mut panes, &mut dividers);
            (panes, dividers)
        };

        let focused_session = {
            let tab = &self.tabs[self.active_tab];
            tab.active
        };

        let padding = self.config.general.padding;
        {
            let tab = &mut self.tabs[self.active_tab];
            for (id, rect) in &panes {
                if let Some(session) = tab.sessions.get_mut(id) {
                    renderer.prepare_session(
                        session,
                        *rect,
                        &profile,
                        padding,
                        self.focused && *id == focused_session,
                    );
                }
            }
        }

        for divider in &dividers {
            renderer.push_quad(
                *divider,
                profile.background.mix(profile.foreground, 0.18),
                1.0,
            );
        }

        renderer.prepare_tab_bar(&infos, &profile, tab_bar);

        if let Some(session) = self.tabs[self.active_tab].sessions.get(&focused_session) {
            if !session.title.is_empty() && !self.current_title.starts_with(&session.title) {
                let title = format!("{} — Riterm", session.title);
                window.set_title(&title);
                self.current_title = title;
            }
        }

        if let Err(err) = renderer.render(profile.background) {
            warn!("render failed: {err:#}");
        }
    }

    fn tab_at(&self, x: f32) -> Option<usize> {
        if self.tabs.is_empty() {
            return None;
        }
        let (w, _) = self.renderer.as_ref()?.surface_size();
        let tab_w = (w as f32 / self.tabs.len() as f32).min(220.0 * self.scale as f32);
        if tab_w <= 0.0 {
            return None;
        }
        let index = (x / tab_w).floor() as usize;
        (index < self.tabs.len()).then_some(index)
    }

    fn mouse_cell(
        padding: f32,
        session: &Session,
        rect: Rect,
        x: f64,
        y: f64,
        cell_w: f32,
        cell_h: f32,
    ) -> Option<Point> {
        let inner = rect.inset(padding);
        if !inner.contains(x as f32, y as f32) {
            return None;
        }
        let col = ((x as f32 - inner.x) / cell_w).floor() as usize;
        let row = ((y as f32 - inner.y) / cell_h).floor() as usize;
        let col = col.min(session.size.cols.saturating_sub(1));
        let row = row.min(session.size.rows.saturating_sub(1));
        let display_offset = session.term.grid().display_offset();
        Some(viewport_to_point(
            display_offset,
            Point::new(row, Column(col)),
        ))
    }

    fn mouse_down(&mut self, button: MouseButton) {
        let x = self.mouse_position.x;
        let y = self.mouse_position.y;

        if y < self.tab_bar_height() as f64 {
            if button == MouseButton::Left {
                if let Some(index) = self.tab_at(x as f32) {
                    self.select_tab(index);
                }
            }
            return;
        }

        let profile = self.profile_for_index(self.active_tab);
        let (cell_w, cell_h) = match self.renderer.as_mut() {
            Some(renderer) => renderer.cell_size(&profile),
            None => return,
        };
        let padding = self.config.general.padding * self.scale as f32;
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let panes = self.pane_rects(tab).0;
        let Some((session_id, rect)) = panes
            .iter()
            .find(|(_, rect)| rect.contains(x as f32, y as f32))
            .copied()
        else {
            return;
        };

        self.tabs[self.active_tab].active = session_id;
        self.mark_all_dirty();

        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(session) = tab.sessions.get_mut(&session_id) else {
            return;
        };

        let mode = *session.term.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            let Some(point) = Self::mouse_cell(padding, session, rect, x, y, cell_w, cell_h) else {
                return;
            };
            let button_code = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => return,
            };
            let bytes = encode_mouse(&mode, button_code, true, point, self.modifiers, false);
            session.write(bytes);
            self.request_redraw();
            return;
        }

        if button == MouseButton::Left {
            let Some(point) = Self::mouse_cell(padding, session, rect, x, y, cell_w, cell_h) else {
                return;
            };
            session.term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
            self.drag = Some(DragSelection {
                session_id,
                start: point,
            });
            self.request_redraw();
        }
        self.request_redraw();
    }

    fn mouse_up(&mut self, button: MouseButton) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if button != MouseButton::Left {
            self.drag = Some(drag);
            return;
        }
        if self.config.general.copy_on_select {
            let text = self
                .find_session_ref(drag.session_id)
                .and_then(|session| session.term.selection_to_string());
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                self.set_clipboard(text);
            }
        }
        self.mark_all_dirty();
        self.request_redraw();
    }

    fn find_session_ref(&self, id: u64) -> Option<&Session> {
        self.tabs.iter().find_map(|tab| tab.sessions.get(&id))
    }

    fn mouse_drag(&mut self) {
        let Some(drag) = &self.drag else {
            return;
        };
        let session_id = drag.session_id;
        let start = drag.start;
        let profile = self.profile_for_index(self.active_tab);
        let (cell_w, cell_h) = match self.renderer.as_mut() {
            Some(renderer) => renderer.cell_size(&profile),
            None => return,
        };
        let padding = self.config.general.padding * self.scale as f32;
        let x = self.mouse_position.x;
        let y = self.mouse_position.y;

        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let panes = self.pane_rects(tab).0;
        let Some((_, rect)) = panes.iter().find(|(id, _)| *id == session_id).copied() else {
            return;
        };

        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(session) = tab.sessions.get_mut(&session_id) else {
            return;
        };
        let Some(point) = Self::mouse_cell(padding, session, rect, x, y, cell_w, cell_h) else {
            return;
        };
        if let Some(selection) = session.term.selection.as_mut() {
            let side = if point < start {
                Side::Left
            } else {
                Side::Right
            };
            selection.update(point, side);
        }
        self.request_redraw();
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
            MouseScrollDelta::PixelDelta(pos) => (pos.y / 40.0).round() as i32,
        };
        if lines == 0 {
            return;
        }

        let x = self.mouse_position.x;
        let y = self.mouse_position.y;
        let profile = self.profile_for_index(self.active_tab);
        let (cell_w, cell_h) = match self.renderer.as_mut() {
            Some(renderer) => renderer.cell_size(&profile),
            None => return,
        };
        let padding = self.config.general.padding * self.scale as f32;
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let panes = self.pane_rects(tab).0;
        let Some((session_id, rect)) = panes
            .iter()
            .find(|(_, rect)| rect.contains(x as f32, y as f32))
            .copied()
        else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(session) = tab.sessions.get_mut(&session_id) else {
            return;
        };

        let mode = *session.term.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            if let Some(point) = Self::mouse_cell(padding, session, rect, x, y, cell_w, cell_h) {
                let button = if lines > 0 { 64 } else { 65 };
                for _ in 0..lines.unsigned_abs().min(5) {
                    let bytes = encode_mouse(&mode, button, true, point, self.modifiers, false);
                    session.write(bytes);
                }
            }
        } else {
            session.scroll(lines * 3);
        }
        self.request_redraw();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attributes = Window::default_attributes()
            .with_title("Riterm")
            .with_inner_size(LogicalSize::new(1100.0, 700.0));
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            attributes = attributes.with_name("riterm", "riterm");
        }

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                tracing::error!("could not create window: {err}");
                event_loop.exit();
                return;
            }
        };
        self.scale = window.scale_factor();
        self.window = Some(window.clone());

        match Renderer::new(Arc::clone(&window), event_loop, self.scale) {
            Ok(renderer) => self.renderer = Some(renderer),
            Err(err) => {
                tracing::error!("could not initialize renderer: {err:#}");
                event_loop.exit();
                return;
            }
        }

        self.create_tab();
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_scale(scale_factor);
                }
                self.mark_all_dirty();
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if focused {
                    self.restart_cursor_blink();
                }
                self.mark_all_dirty();
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(event_loop, &event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_position = position;
                self.mouse_drag();
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => self.mouse_down(button),
                ElementState::Released => self.mouse_up(button),
            },
            WindowEvent::MouseWheel { delta, .. } => self.mouse_wheel(delta),
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Session(id, SessionEvent::Bytes(data)) => {
                let mut tab_index = None;
                for (index, tab) in self.tabs.iter_mut().enumerate() {
                    if let Some(session) = tab.sessions.get_mut(&id) {
                        session.feed(&data);
                        tab_index = Some(index);
                        break;
                    }
                }
                if let Some(index) = tab_index {
                    let failed = self.tabs[index].observe_command_failures();
                    if failed && index != self.active_tab {
                        self.tabs[index].bell = true;
                    }
                }
                self.request_redraw();
            }
            UserEvent::Session(id, SessionEvent::Term(event)) => {
                self.handle_term_event(id, event, event_loop);
            }
            UserEvent::Session(id, SessionEvent::Exited) => {
                if let Some(session) = self.find_session_mut(id) {
                    session.exited = true;
                    if session.title.is_empty() {
                        session.title = "shell exited".to_string();
                    }
                }
                self.update_title();
                self.request_redraw();
            }
            UserEvent::Session(id, SessionEvent::Error(err)) => {
                debug!("session {id} error: {err}");
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let _ = &self.config_path;
        let profile = self.profile_for_index(self.active_tab);
        if !profile.cursor_blink || !self.focused || self.tabs.is_empty() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }

        let now = Instant::now();
        if now >= self.cursor_blink_next {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.cursor_blink_next = now + Duration::from_millis(500);
            self.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.cursor_blink_next));
    }
}

fn encode_mouse(
    mode: &TermMode,
    button: u8,
    pressed: bool,
    point: Point,
    modifiers: ModifiersState,
    motion: bool,
) -> Vec<u8> {
    let mut code = button;
    if modifiers.shift_key() {
        code += 4;
    }
    if modifiers.alt_key() {
        code += 8;
    }
    if modifiers.control_key() {
        code += 16;
    }
    if motion {
        code += 32;
    }

    let x = point.column.0 as u32 + 1;
    let y = point.line.0.max(0) as u32 + 1;

    if mode.contains(TermMode::SGR_MOUSE) {
        format!("\x1b[<{code};{x};{y}{}", if pressed { 'M' } else { 'm' }).into_bytes()
    } else {
        let final_code = if pressed {
            code
        } else {
            3 + (code & 0b1110_0000)
        };
        vec![
            0x1b,
            b'[',
            b'M',
            (32 + final_code).min(255),
            (32 + x as u8).min(255),
            (32 + y as u8).min(255),
        ]
    }
}

fn color_for_index(index: usize, profile: &ResolvedProfile) -> Rgb {
    if index < 16 {
        profile.palette[index]
    } else if index == 256 {
        profile.foreground
    } else if index == 257 {
        profile.background
    } else if index == 258 {
        profile.cursor
    } else if index < 256 {
        let index = index as u8;
        if index < 232 {
            let n = index - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let component = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb::new(component(r), component(g), component(b))
        } else {
            let level = 8 + (index - 232) * 10;
            Rgb::new(level, level, level)
        }
    } else {
        profile.foreground
    }
}

/// Keep the compiler honest about unused imports in early builds.
#[allow(dead_code)]
fn _typecheck(_: Key) {}

#[cfg(test)]
mod tests {
    use super::observe_finished_commands;

    #[test]
    fn new_failure_is_reported_once() {
        let mut acknowledged = 0;
        assert!(observe_finished_commands(
            &mut acknowledged,
            [(1, None), (2, Some(0)), (3, Some(1))].into_iter()
        ));
        assert_eq!(acknowledged, 3);

        // Re-observing the same sessions must not re-trigger the indicator.
        assert!(!observe_finished_commands(
            &mut acknowledged,
            [(1, None), (2, Some(0)), (3, Some(1))].into_iter()
        ));
        assert_eq!(acknowledged, 3);
    }

    #[test]
    fn success_only_advances_the_epoch() {
        let mut acknowledged = 0;
        assert!(!observe_finished_commands(
            &mut acknowledged,
            [(4, Some(0))].into_iter()
        ));
        assert_eq!(acknowledged, 4);
    }

    #[test]
    fn never_finished_sessions_are_ignored() {
        let mut acknowledged = 0;
        assert!(!observe_finished_commands(
            &mut acknowledged,
            [(0, None), (0, None)].into_iter()
        ));
        assert_eq!(acknowledged, 0);
    }
}

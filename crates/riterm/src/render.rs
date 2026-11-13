//! GPU renderer: cell backgrounds/cursor/selection as quads, text via glyphon.

use std::collections::HashMap;
use std::sync::Arc;

use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{TermMode, point_to_viewport, viewport_to_point};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor};
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glyphon::cosmic_text::{FeatureTag, FontFeatures, UnderlineStyle};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};
use riterm_config::{ResolvedProfile, Rgb};
use riterm_core::Session;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::layout::Rect;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct QuadVertex {
    position: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct Quad {
    rect: Rect,
    color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FontKey {
    font_signature: u64,
    font_size_bits: u64,
    line_height_bits: u64,
    scale_bits: u64,
}

impl FontKey {
    fn new(profile: &ResolvedProfile, scale: f64) -> Self {
        Self {
            font_signature: font_signature(profile),
            font_size_bits: u64::from(profile.font_size.to_bits()),
            line_height_bits: u64::from(profile.line_height.to_bits()),
            scale_bits: scale.to_bits(),
        }
    }
}

fn font_signature(profile: &ResolvedProfile) -> u64 {
    let mut hash = 1469598103934665603u64;
    let mut add = |text: &str| {
        for byte in text.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(1099511628211);
    };
    if let Some(family) = &profile.font_family {
        add(family);
    }
    for family in &profile.font_fallbacks {
        add(family);
    }
    if let Some(family) = &profile.non_ascii_font_family {
        add(family);
    }
    if profile.use_non_ascii_font {
        add("non-ascii");
    }
    for feature in &profile.font_features {
        add(feature);
    }
    hash
}

#[derive(Clone, Copy, Debug)]
pub struct FontMetrics {
    pub cell_w: f32,
    pub cell_h: f32,
    pub font_px: f32,
    pub line_px: f32,
    pub metrics: Metrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AreaKind {
    Session { id: u64, segment: usize },
    TabBar,
    Overlay,
}

#[derive(Clone, Debug)]
struct AreaSpec {
    kind: AreaKind,
    left: f32,
    top: f32,
    bounds: TextBounds,
    default_color: Color,
}

/// One shaped piece of a terminal row. ASCII stays in runs; non-ASCII glyphs
/// are shaped separately so fallback fonts cannot shift later columns.
struct Segment {
    buffer: Buffer,
    row: usize,
    col: usize,
    width_cells: usize,
    x_offset: f32,
}

struct SessionBuffer {
    segments: Vec<Segment>,
    metrics: Metrics,
    cols: usize,
    rows: usize,
    cursor_visible: bool,
}

/// The concrete font families chosen for a profile.
#[derive(Clone, Debug, Default)]
struct ResolvedFamilies {
    primary: Option<String>,
    non_ascii: Option<String>,
}

/// Tab information used to draw the tab bar.
pub struct TabInfo {
    pub title: String,
    pub active: bool,
    pub bell: bool,
    pub running: bool,
}

pub struct Renderer {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<wgpu::Surface<'static>>,
    surface_config: wgpu::SurfaceConfiguration,
    window: Option<Arc<Window>>,

    font_system: FontSystem,
    swash_cache: SwashCache,
    _cache: Cache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,

    quad_pipeline: wgpu::RenderPipeline,
    quad_buffer: Option<wgpu::Buffer>,
    quad_capacity: usize,
    quads: Vec<Quad>,
    text_specs: Vec<AreaSpec>,

    session_buffers: HashMap<u64, SessionBuffer>,
    tab_buffer: Option<Buffer>,
    overlay_buffer: Option<Buffer>,
    metrics_cache: HashMap<FontKey, FontMetrics>,
    families_cache: HashMap<u64, ResolvedFamilies>,
    cursor_visible: bool,
    scale: f64,
    surface_size: (u32, u32),
}

impl Renderer {
    pub fn new(window: Arc<Window>, event_loop: &ActiveEventLoop, scale: f64) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .context("could not create render surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no GPU adapter available")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .context("could not create GPU device")?;
        let caps = surface.get_capabilities(&adapter);
        // Render sRGB bytes directly into a unorm target. Picking an *Srgb
        // format would make wgpu linearise our colors a second time.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(TextureFormatExt::is_unorm)
            .or_else(|| caps.formats.iter().copied().find(TextureFormatExt::is_srgb))
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &surface_config);

        Ok(Self::from_parts(
            instance,
            device,
            queue,
            Some(surface),
            Some(window),
            surface_config,
            scale,
        ))
    }

    /// Build a renderer that draws into an offscreen texture instead of a
    /// window surface. Used by `--render-check` and tests.
    pub fn new_offscreen(width: u32, height: u32, scale: f64) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no GPU adapter available")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .context("could not create GPU device")?;
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Rgba8Unorm,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };

        Ok(Self::from_parts(
            instance,
            device,
            queue,
            None,
            None,
            surface_config,
            scale,
        ))
    }

    fn from_parts(
        instance: wgpu::Instance,
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: Option<wgpu::Surface<'static>>,
        window: Option<Arc<Window>>,
        surface_config: wgpu::SurfaceConfiguration,
        scale: f64,
    ) -> Self {
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let format = surface_config.format;
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let quad_pipeline = create_quad_pipeline(&device, format);

        let mut renderer = Self {
            instance,
            device,
            queue,
            surface,
            surface_config,
            window,
            font_system,
            swash_cache,
            _cache: cache,
            viewport,
            atlas,
            text_renderer,
            quad_pipeline,
            quad_buffer: None,
            quad_capacity: 0,
            quads: Vec::new(),
            text_specs: Vec::new(),
            session_buffers: HashMap::new(),
            tab_buffer: None,
            overlay_buffer: None,
            metrics_cache: HashMap::new(),
            families_cache: HashMap::new(),
            cursor_visible: true,
            scale,
            surface_size: (0, 0),
        };
        renderer.surface_size = (
            renderer.surface_config.width,
            renderer.surface_config.height,
        );
        let default_profile = ResolvedProfile::default();
        renderer.metrics_for(&default_profile, scale);
        renderer
    }

    pub fn surface_size(&self) -> (u32, u32) {
        self.surface_size
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if (width, height) == self.surface_size {
            return;
        }
        self.surface_size = (width, height);
        self.surface_config.width = width;
        self.surface_config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.surface_config);
        }
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
    }

    /// Whether the blinking cursor is currently in its visible phase. A
    /// non-blinking cursor is always drawn by [`Self::prepare_session`].
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    /// Cell size for a profile at the current scale factor.
    pub fn cell_size(&mut self, profile: &ResolvedProfile) -> (f32, f32) {
        let metrics = self.metrics_for(profile, self.scale);
        (metrics.cell_w, metrics.cell_h)
    }

    /// Resolve the primary and non-ASCII families against the system font
    /// database. iTerm2 falls back through the profile's font list; on Linux
    /// we do the same and end at the generic monospace family.
    fn families_for(&mut self, profile: &ResolvedProfile) -> ResolvedFamilies {
        let key = font_signature(profile);
        if let Some(families) = self.families_cache.get(&key) {
            return families.clone();
        }

        let families = {
            let db = self.font_system.db();
            let available = |name: &str| family_available(db, name);
            let primary = profile
                .font_family
                .as_deref()
                .filter(|name| available(name))
                .map(str::to_string)
                .or_else(|| {
                    profile
                        .font_fallbacks
                        .iter()
                        .find(|name| available(name))
                        .cloned()
                });
            let non_ascii = if profile.use_non_ascii_font {
                profile
                    .non_ascii_font_family
                    .as_deref()
                    .filter(|name| available(name))
                    .map(str::to_string)
            } else {
                None
            };
            ResolvedFamilies { primary, non_ascii }
        };

        self.families_cache.insert(key, families.clone());
        families
    }

    fn metrics_for(&mut self, profile: &ResolvedProfile, scale: f64) -> FontMetrics {
        let key = FontKey::new(profile, scale);
        if let Some(metrics) = self.metrics_cache.get(&key) {
            return *metrics;
        }

        let font_px = (profile.font_size * scale as f32).max(4.0);
        let line_px = (font_px * profile.line_height).max(font_px);
        let metrics = Metrics::new(font_px, line_px);
        let families = self.families_for(profile);
        let family = family_of(families.primary.as_deref());
        let features = parse_font_features(profile);

        let mut buffer = Buffer::new_empty(metrics);
        buffer.set_size(None, None);
        let attrs = Attrs::new()
            .family(family)
            .color(Color::rgb(255, 255, 255))
            .font_features(features);
        buffer.set_text("MMMMMMMMMMMMMMMMMMMM", &attrs, Shaping::Basic, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let cell_w = buffer
            .layout_runs()
            .next()
            .map(|run| run.line_w / 20.0)
            .filter(|w| w.is_finite() && *w > 1.0)
            .unwrap_or(font_px * 0.6);

        let metrics = FontMetrics {
            cell_w,
            cell_h: line_px,
            font_px,
            line_px,
            metrics,
        };
        self.metrics_cache.insert(key, metrics);
        metrics
    }

    pub fn cancel_frame(&mut self) {
        self.quads.clear();
        self.text_specs.clear();
    }

    pub fn push_quad(&mut self, rect: Rect, color: Rgb, alpha: f32) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        self.quads.push(Quad {
            rect,
            color: color.to_rgba_f32(alpha),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_session(
        &mut self,
        session: &mut Session,
        rect: Rect,
        profile: &ResolvedProfile,
        padding_logical: f32,
        focused: bool,
        highlights: &[(Point, Point)],
        copy_cursor: Option<Point>,
    ) {
        let metrics = self.metrics_for(profile, self.scale);
        let padding = (padding_logical * self.scale as f32).max(0.0);
        let inner = rect.inset(padding);
        if inner.w < 4.0 || inner.h < 4.0 {
            return;
        }

        let cols = ((inner.w / metrics.cell_w).floor() as usize).max(1);
        let rows = ((inner.h / metrics.cell_h).floor() as usize).max(1);
        session.resize(
            cols,
            rows,
            metrics.cell_w.round().max(1.0) as u16,
            metrics.cell_h.round().max(1.0) as u16,
        );

        // Terminal background.
        self.push_quad(rect, profile.background, 1.0);

        // Rebuild text only when content changed or metrics changed.
        let needs_rebuild = session.dirty
            || self
                .session_buffers
                .get(&session.id)
                .map(|buf| {
                    buf.metrics != metrics.metrics
                        || buf.cols != cols
                        || buf.rows != rows
                        || buf.cursor_visible != self.cursor_visible
                })
                .unwrap_or(true);
        if needs_rebuild {
            let families = self.families_for(profile);
            let segments = build_term_segments(
                &mut self.font_system,
                session,
                profile,
                &families,
                metrics,
                cols,
                rows,
                focused && self.cursor_visible,
            );
            self.session_buffers.insert(
                session.id,
                SessionBuffer {
                    segments,
                    metrics: metrics.metrics,
                    cols,
                    rows,
                    cursor_visible: self.cursor_visible,
                },
            );
            session.dirty = false;
        }

        self.append_cell_quads(
            session,
            inner,
            metrics,
            profile,
            focused,
            highlights,
            copy_cursor,
        );

        let mut specs = Vec::new();
        if let Some(buffer) = self.session_buffers.get(&session.id) {
            for (index, segment) in buffer.segments.iter().enumerate() {
                let left = inner.x + segment.col as f32 * metrics.cell_w;
                let top = inner.y + segment.row as f32 * metrics.cell_h;
                let width = segment.width_cells as f32 * metrics.cell_w;
                specs.push(AreaSpec {
                    kind: AreaKind::Session {
                        id: session.id,
                        segment: index,
                    },
                    left: left + segment.x_offset,
                    top,
                    bounds: TextBounds {
                        left: left.round() as i32,
                        top: top.round() as i32,
                        right: (left + width).round() as i32,
                        bottom: (top + metrics.cell_h).round() as i32,
                    },
                    default_color: color(profile.foreground),
                });
            }
        }
        self.text_specs.extend(specs);
    }

    #[allow(clippy::too_many_arguments)]
    fn append_cell_quads(
        &mut self,
        session: &Session,
        inner: Rect,
        metrics: FontMetrics,
        profile: &ResolvedProfile,
        focused: bool,
        highlights: &[(Point, Point)],
        copy_cursor: Option<Point>,
    ) {
        let term = &session.term;
        let mode = *term.mode();
        let colors = *term.colors();
        let display_offset = term.grid().display_offset();
        let selection = term.selection.as_ref().and_then(|s| s.to_range(term));

        let cursor = term.grid().cursor.point;
        let cursor_viewport = point_to_viewport(display_offset, cursor);
        let cursor_shape = term.cursor_style().shape;
        let show_cursor = focused
            && self.cursor_visible
            && mode.contains(TermMode::SHOW_CURSOR)
            && cursor_shape != CursorShape::Hidden;
        let cursor_block =
            show_cursor && matches!(cursor_shape, CursorShape::Block | CursorShape::HollowBlock);

        // Background runs per row.
        for row in 0..session.size.rows {
            let mut run_start: Option<usize> = None;
            let mut run_color = profile.background;
            for col in 0..session.size.cols {
                let abs = viewport_to_point(
                    display_offset,
                    alacritty_terminal::index::Point::new(row, Column(col)),
                );
                let cell = &term.grid()[abs];
                let on_cursor = cursor_block
                    && cursor_viewport
                        .map(|p| p.line == row && p.column == col)
                        .unwrap_or(false);
                let selected = selection.map(|sel| sel.contains(abs)).unwrap_or(false);
                let (_, bg) = cell_style(cell, profile, &colors, selected, on_cursor);

                match run_start {
                    Some(_) if bg == run_color => {}
                    Some(start) => {
                        self.flush_run(inner, metrics, row, start, col, run_color);
                        run_start = if bg == profile.background {
                            None
                        } else {
                            Some(col)
                        };
                        run_color = bg;
                    }
                    None => {
                        if bg != profile.background {
                            run_start = Some(col);
                            run_color = bg;
                        }
                    }
                }
            }
            if let Some(start) = run_start {
                self.flush_run(inner, metrics, row, start, session.size.cols, run_color);
            }
        }

        // Search hits are translucent overlays. The current hit is also the
        // terminal selection and gets the full selection colors.
        for (start, end) in highlights {
            let viewport_start = point_to_viewport(display_offset, *start);
            let viewport_end = point_to_viewport(display_offset, *end);
            let (Some(start), Some(end)) = (viewport_start, viewport_end) else {
                continue;
            };
            for line in start.line..=end.line {
                if line >= session.size.rows {
                    break;
                }
                let first = if line == start.line {
                    start.column.0
                } else {
                    0
                };
                let last = if line == end.line {
                    (end.column.0 + 1).min(session.size.cols)
                } else {
                    session.size.cols
                };
                if last > first {
                    self.push_quad(
                        Rect::new(
                            inner.x + first as f32 * metrics.cell_w,
                            inner.y + line as f32 * metrics.cell_h,
                            (last - first) as f32 * metrics.cell_w,
                            metrics.cell_h,
                        ),
                        profile.selection,
                        0.32,
                    );
                }
            }
        }

        if let Some(copy_cursor) = copy_cursor {
            if let Some(point) = point_to_viewport(display_offset, copy_cursor) {
                if point.line < session.size.rows && point.column < session.size.cols {
                    let x = inner.x + point.column.0 as f32 * metrics.cell_w;
                    let y = inner.y + point.line as f32 * metrics.cell_h;
                    let thickness = (metrics.cell_w * 0.10).clamp(1.0, 2.0);
                    for rect in [
                        Rect::new(x, y, metrics.cell_w, thickness),
                        Rect::new(x, y + metrics.cell_h - thickness, metrics.cell_w, thickness),
                        Rect::new(x, y, thickness, metrics.cell_h),
                        Rect::new(x + metrics.cell_w - thickness, y, thickness, metrics.cell_h),
                    ] {
                        self.push_quad(rect, profile.cursor, 0.9);
                    }
                }
            }
        }

        if !show_cursor {
            return;
        }
        let Some(cursor_viewport) = cursor_viewport else {
            return;
        };
        if cursor_viewport.line >= session.size.rows || cursor_viewport.column >= session.size.cols
        {
            return;
        }

        let x = inner.x + cursor_viewport.column.0 as f32 * metrics.cell_w;
        let y = inner.y + cursor_viewport.line as f32 * metrics.cell_h;

        match cursor_shape {
            CursorShape::Block => {}
            CursorShape::HollowBlock => {
                let thickness = (metrics.cell_w * 0.08).clamp(1.0, 2.0);
                self.push_quad(
                    Rect::new(x, y, metrics.cell_w, thickness),
                    profile.cursor,
                    1.0,
                );
                self.push_quad(
                    Rect::new(x, y + metrics.cell_h - thickness, metrics.cell_w, thickness),
                    profile.cursor,
                    1.0,
                );
                self.push_quad(
                    Rect::new(x, y, thickness, metrics.cell_h),
                    profile.cursor,
                    1.0,
                );
                self.push_quad(
                    Rect::new(x + metrics.cell_w - thickness, y, thickness, metrics.cell_h),
                    profile.cursor,
                    1.0,
                );
            }
            CursorShape::Beam => {
                let thickness = (metrics.cell_w * 0.12).clamp(1.0, 3.0);
                self.push_quad(
                    Rect::new(x, y, thickness, metrics.cell_h),
                    profile.cursor,
                    1.0,
                );
            }
            CursorShape::Underline => {
                let thickness = (metrics.cell_h * 0.08).clamp(1.0, 3.0);
                self.push_quad(
                    Rect::new(x, y + metrics.cell_h - thickness, metrics.cell_w, thickness),
                    profile.cursor,
                    1.0,
                );
            }
            CursorShape::Hidden => {}
        }
    }

    fn flush_run(
        &mut self,
        inner: Rect,
        metrics: FontMetrics,
        row: usize,
        start: usize,
        end: usize,
        color: Rgb,
    ) {
        if end <= start {
            return;
        }
        self.push_quad(
            Rect::new(
                inner.x + start as f32 * metrics.cell_w,
                inner.y + row as f32 * metrics.cell_h,
                (end - start) as f32 * metrics.cell_w,
                metrics.cell_h,
            ),
            color,
            1.0,
        );
    }

    /// Draw a one-line status/search prompt at the bottom of `rect`.
    pub fn prepare_overlay(
        &mut self,
        text: &str,
        profile: &ResolvedProfile,
        rect: Rect,
        accent: Rgb,
    ) {
        if rect.w < 8.0 || rect.h < 8.0 {
            return;
        }
        let metrics = self.metrics_for(profile, self.scale);
        let font_px = (metrics.font_px * 0.86).max(8.0);
        let overlay_metrics = Metrics::new(font_px, (font_px * 1.35).max(font_px));
        let families = self.families_for(profile);
        let family = family_of(families.primary.as_deref());
        let features = parse_font_features(profile);
        let padding = 10.0 * self.scale as f32;
        let max_chars =
            (((rect.w - padding * 2.0) / (metrics.cell_w * 0.86).max(1.0)).floor() as usize).max(4);
        let mut line: String = text.chars().take(max_chars).collect();
        if text.chars().count() > max_chars {
            line.pop();
            line.push('…');
        }

        let bar_bg = profile.background.mix(profile.foreground, 0.10);
        self.push_quad(rect, bar_bg, 0.97);
        self.push_quad(Rect::new(rect.x, rect.y, rect.w, 1.0), accent, 0.9);

        let attrs = Attrs::new()
            .family(family)
            .color(color(profile.foreground))
            .font_features(features);
        let mut buffer = Buffer::new(&mut self.font_system, overlay_metrics);
        buffer.set_size(Some(rect.w.max(1.0)), Some(rect.h.max(1.0)));
        buffer.set_text(&line, &attrs, Shaping::Basic, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.overlay_buffer = Some(buffer);

        let text_top = rect.y + (rect.h - overlay_metrics.line_height) / 2.0;
        self.text_specs.push(AreaSpec {
            kind: AreaKind::Overlay,
            left: rect.x + padding,
            top: text_top.max(rect.y),
            bounds: text_bounds(rect),
            default_color: color(profile.foreground),
        });
    }

    pub fn prepare_tab_bar(&mut self, tabs: &[TabInfo], profile: &ResolvedProfile, rect: Rect) {
        if tabs.is_empty() {
            return;
        }
        let metrics = self.metrics_for(profile, self.scale);
        let bar_bg = profile.background.mix(profile.foreground, 0.06);
        let active_bg = profile.background.mix(profile.foreground, 0.16);
        let border = profile.background.mix(profile.foreground, 0.22);

        self.push_quad(rect, bar_bg, 1.0);
        self.push_quad(
            Rect::new(rect.x, rect.bottom() - 1.0, rect.w, 1.0),
            border,
            1.0,
        );

        let tab_w = (rect.w / tabs.len() as f32).min(220.0 * self.scale as f32);
        let tab_font_px = (metrics.font_px * 0.82).max(8.0);
        let tab_metrics = Metrics::new(tab_font_px, (tab_font_px * 1.3).max(tab_font_px));
        let char_w = (metrics.cell_w * 0.82).max(1.0);
        let chars_per_tab = ((tab_w - 18.0 * self.scale as f32) / char_w)
            .floor()
            .max(3.0) as usize;
        let families = self.families_for(profile);
        let family_name = families.primary.clone();
        let family = family_of(family_name.as_deref());
        let features = parse_font_features(profile);
        let active_fg = color(profile.foreground);
        let inactive_fg = color(profile.foreground.mix(profile.background, 0.45));

        for (index, tab) in tabs.iter().enumerate() {
            let x = rect.x + index as f32 * tab_w;
            if tab.active {
                self.push_quad(Rect::new(x, rect.y, tab_w, rect.h - 1.0), active_bg, 1.0);
                self.push_quad(Rect::new(x, rect.y, tab_w, 2.0), profile.cursor, 0.9);
            }
            if tab.bell || tab.running {
                let dot = (4.0 * self.scale as f32).max(3.0);
                self.push_quad(
                    Rect::new(
                        x + tab_w - dot - 8.0 * self.scale as f32,
                        rect.y + (rect.h - dot) / 2.0,
                        dot,
                        dot,
                    ),
                    if tab.bell {
                        Rgb::new(255, 120, 120)
                    } else {
                        profile.cursor
                    },
                    1.0,
                );
            }
        }

        // Build the tab title text.
        let mut spans: Vec<(String, Attrs)> = Vec::new();
        for (index, tab) in tabs.iter().enumerate() {
            let mut title = format!(" {}  {}", index + 1, sanitize_title(&tab.title));
            let mut chars: Vec<char> = title.chars().collect();
            if chars.len() > chars_per_tab {
                chars.truncate(chars_per_tab.saturating_sub(1));
                chars.push('…');
            }
            while chars.len() < chars_per_tab {
                chars.push(' ');
            }
            title = chars.into_iter().collect();
            let attrs = Attrs::new()
                .family(family)
                .color(if tab.active { active_fg } else { inactive_fg })
                .font_features(features.clone())
                .weight(if tab.active {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                });
            spans.push((title, attrs));
        }
        let mut buffer = Buffer::new(&mut self.font_system, tab_metrics);
        buffer.set_size(Some(rect.w.max(1.0)), Some(rect.h.max(1.0)));
        let default_attrs = Attrs::new()
            .family(family)
            .color(active_fg)
            .font_features(features);
        let refs: Vec<(&str, Attrs)> = spans.iter().map(|(s, a)| (s.as_str(), a.clone())).collect();
        buffer.set_rich_text(refs, &default_attrs, Shaping::Basic, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.tab_buffer = Some(buffer);

        let text_top = rect.y + (rect.h - tab_metrics.line_height) / 2.0;
        self.text_specs.push(AreaSpec {
            kind: AreaKind::TabBar,
            left: rect.x + 8.0 * self.scale as f32,
            top: text_top.max(rect.y),
            bounds: text_bounds(rect),
            default_color: active_fg,
        });
    }

    pub fn remove_session(&mut self, id: u64) {
        self.session_buffers.remove(&id);
    }

    pub fn render(&mut self, clear: Rgb) -> Result<()> {
        let width = self.surface_config.width;
        let height = self.surface_config.height;
        if width == 0 || height == 0 {
            return Ok(());
        }

        let Some(surface) = self.surface.as_ref() else {
            anyhow::bail!("render() called on an offscreen renderer");
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                if let Some(window) = &self.window {
                    let surface = self
                        .instance
                        .create_surface(window.clone())
                        .context("could not recreate surface")?;
                    surface.configure(&self.device, &self.surface_config);
                    self.surface = Some(surface);
                }
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error");
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("riterm frame"),
            });
        self.draw(&mut encoder, &view, clear)?;
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.atlas.trim();
        Ok(())
    }

    /// Render the queued frame into an offscreen texture and return its RGBA
    /// pixels, top row first.
    pub fn render_offscreen(&mut self, clear: Rgb) -> Result<Vec<u8>> {
        let width = self.surface_config.width;
        let height = self.surface_config.height;
        if width == 0 || height == 0 {
            anyhow::bail!("cannot render an empty frame");
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("riterm offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.surface_config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("riterm offscreen frame"),
            });
        self.draw(&mut encoder, &view, clear)?;

        let unpadded_bytes_per_row = width * 4;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("riterm readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        self.atlas.trim();

        let slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("waiting for the GPU readback")?;
        rx.recv()
            .context("GPU readback was cancelled")?
            .context("mapping the readback buffer")?;

        let mapped = slice
            .get_mapped_range()
            .context("accessing the mapped readback buffer")?;
        let bgra = matches!(
            self.surface_config.format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm
        );
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * padded_bytes_per_row) as usize;
            let end = start + unpadded_bytes_per_row as usize;
            let data = &mapped[start..end];
            if bgra {
                for pixel in data.chunks_exact(4) {
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            } else {
                pixels.extend_from_slice(data);
            }
        }
        drop(mapped);
        output_buffer.unmap();
        Ok(pixels)
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: Rgb,
    ) -> Result<()> {
        let width = self.surface_config.width;
        let height = self.surface_config.height;

        // Upload quads.
        let mut vertices = Vec::with_capacity(self.quads.len() * 6);
        for quad in &self.quads {
            let x0 = quad.rect.x / width as f32 * 2.0 - 1.0;
            let x1 = (quad.rect.x + quad.rect.w) / width as f32 * 2.0 - 1.0;
            let y0 = 1.0 - quad.rect.y / height as f32 * 2.0;
            let y1 = 1.0 - (quad.rect.y + quad.rect.h) / height as f32 * 2.0;
            let corners = [
                ([x0, y0], [x1, y0], [x1, y1]),
                ([x0, y0], [x1, y1], [x0, y1]),
            ];
            for triangle in corners {
                for position in [triangle.0, triangle.1, triangle.2] {
                    vertices.push(QuadVertex {
                        position,
                        color: quad.color,
                    });
                }
            }
        }

        if !vertices.is_empty() {
            if self.quad_buffer.is_none() || self.quad_capacity < vertices.len() {
                let capacity = vertices.len().next_power_of_two().max(4096);
                self.quad_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("riterm quads"),
                    size: (capacity * std::mem::size_of::<QuadVertex>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                self.quad_capacity = capacity;
            }
            if let Some(buffer) = &self.quad_buffer {
                self.queue
                    .write_buffer(buffer, 0, bytemuck::cast_slice(&vertices));
            }
        }

        self.viewport
            .update(&self.queue, Resolution { width, height });

        // Prepare text areas.
        {
            let buffers = &self.session_buffers;
            let tab_buffer = self.tab_buffer.as_ref();
            let overlay_buffer = self.overlay_buffer.as_ref();
            let areas = self.text_specs.iter().filter_map(|spec| {
                let buffer = match spec.kind {
                    AreaKind::Session { id, segment } => buffers
                        .get(&id)
                        .and_then(|buffer| buffer.segments.get(segment))
                        .map(|segment| &segment.buffer),
                    AreaKind::TabBar => tab_buffer,
                    AreaKind::Overlay => overlay_buffer,
                }?;
                Some(TextArea {
                    buffer,
                    left: spec.left,
                    top: spec.top,
                    scale: 1.0,
                    bounds: spec.bounds,
                    default_color: spec.default_color,
                    custom_glyphs: &[],
                })
            });
            match self.text_renderer.prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            ) {
                Ok(()) => {}
                Err(glyphon::PrepareError::AtlasFull) => {
                    self.atlas.trim();
                    // Draw backgrounds this frame; text comes back next frame.
                }
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("riterm pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear.r as f64 / 255.0,
                            g: clear.g as f64 / 255.0,
                            b: clear.b as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if !vertices.is_empty() {
                if let Some(buffer) = &self.quad_buffer {
                    pass.set_pipeline(&self.quad_pipeline);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..vertices.len() as u32, 0..1);
                }
            }

            let _ = self
                .text_renderer
                .render(&self.atlas, &self.viewport, &mut pass);
        }

        Ok(())
    }
}

trait TextureFormatExt {
    fn is_srgb(&self) -> bool;
    fn is_unorm(&self) -> bool;
}

impl TextureFormatExt for wgpu::TextureFormat {
    fn is_srgb(&self) -> bool {
        matches!(
            self,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8UnormSrgb
        )
    }

    fn is_unorm(&self) -> bool {
        matches!(
            self,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
        )
    }
}

fn text_bounds(rect: Rect) -> TextBounds {
    TextBounds {
        left: rect.x.round() as i32,
        top: rect.y.round() as i32,
        right: rect.right().round() as i32,
        bottom: rect.bottom().round() as i32,
    }
}

fn family_of(family: Option<&str>) -> Family<'_> {
    family.map(Family::Name).unwrap_or(Family::Monospace)
}

fn family_available(db: &glyphon::fontdb::Database, name: &str) -> bool {
    db.faces().any(|face| {
        face.families
            .iter()
            .any(|(family, _)| family.eq_ignore_ascii_case(name))
    })
}

/// Parse profile features such as `liga`, `+calt`, `-dlig`, or `ss01=2`.
fn parse_font_features(profile: &ResolvedProfile) -> FontFeatures {
    let mut features = FontFeatures::new();
    for spec in &profile.font_features {
        let spec = spec.trim();
        let (tag, value) = if let Some(tag) = spec.strip_prefix('-') {
            (tag, 0)
        } else if let Some(tag) = spec.strip_prefix('+') {
            (tag, 1)
        } else if let Some((tag, value)) = spec.split_once('=') {
            (tag, value.trim().parse::<u32>().unwrap_or(1))
        } else {
            (spec, 1)
        };
        let bytes = tag.as_bytes();
        if bytes.len() == 4 {
            features.set(
                FeatureTag::new(&[bytes[0], bytes[1], bytes[2], bytes[3]]),
                value,
            );
        }
    }
    features
}

fn color(rgb: Rgb) -> Color {
    Color::rgb(rgb.r, rgb.g, rgb.b)
}

fn sanitize_title(title: &str) -> String {
    let title = title.replace(['\n', '\r', '\t'], " ");
    if title.is_empty() {
        "Terminal".to_string()
    } else if title.chars().count() > 64 {
        title.chars().take(64).collect()
    } else {
        title
    }
}

fn cell_style(
    cell: &Cell,
    profile: &ResolvedProfile,
    colors: &Colors,
    selected: bool,
    cursor: bool,
) -> (Rgb, Rgb) {
    let default_foreground = matches!(
        cell.fg,
        AnsiColor::Named(NamedColor::Foreground | NamedColor::BrightForeground)
    );
    let mut fg = resolve_color(cell.fg, colors, profile);
    let mut bg = resolve_color(cell.bg, colors, profile);

    // "Use Bright Bold": SGR 1 on ANSI colors 0-7 selects the bright palette.
    if cell.flags.contains(Flags::BOLD)
        && profile.use_bright_bold
        && !cell.flags.contains(Flags::INVERSE)
    {
        if let AnsiColor::Named(named) = cell.fg {
            if (named as usize) < 8 {
                fg = resolve_color(AnsiColor::Named(named.to_bright()), colors, profile);
            }
        }
    }

    // iTerm2 uses the profile's bold color only for bold text that uses the
    // default foreground. Explicit colors, reverse video, selection and the
    // cursor each override it.
    if cell.flags.contains(Flags::BOLD)
        && default_foreground
        && profile.use_bold_font
        && !cell.flags.contains(Flags::INVERSE)
    {
        fg = profile.bold;
    }

    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.contains(Flags::DIM) {
        fg = fg.mix(profile.background, 0.45);
    }
    if selected {
        fg = profile.selection_text;
        bg = profile.selection;
    }
    if cursor {
        fg = profile.cursor_text;
        bg = profile.cursor;
    }
    if profile.minimum_contrast > 0.0 {
        fg = ensure_contrast(fg, bg, profile.minimum_contrast);
    }
    (fg, bg)
}

/// iTerm2's `getComponents:forComponents:withContrastAgainstComponents:`.
/// Brightness is the same weighted RGB measure iTerm2 uses, and colors are
/// moved toward black or white until the requested brightness difference is
/// reached.
fn ensure_contrast(fg: Rgb, bg: Rgb, minimum: f32) -> Rgb {
    fn brightness(r: f32, g: f32, b: f32) -> f32 {
        0.30 * r + 0.59 * g + 0.11 * b
    }
    fn unit(value: u8) -> f32 {
        value as f32 / 255.0
    }

    let main = [unit(fg.r), unit(fg.g), unit(fg.b)];
    let other = [unit(bg.r), unit(bg.g), unit(bg.b)];
    let main_brightness = brightness(main[0], main[1], main[2]);
    let other_brightness = brightness(other[0], other[1], other[2]);
    let difference = (main_brightness - other_brightness).abs();
    let minimum = minimum.clamp(0.0, 1.0);
    if difference >= minimum {
        return fg;
    }

    let error = (difference - minimum).abs();
    let mut target = main_brightness;
    if main_brightness < other_brightness {
        target -= error;
        if target < 0.0 {
            let alternative = other_brightness + minimum;
            let base_contrast = other_brightness;
            let alt_contrast = alternative.min(1.0) - other_brightness;
            if alt_contrast > base_contrast {
                target = alternative;
            }
        }
    } else {
        target += error;
        if target > 1.0 {
            let alternative = other_brightness - minimum;
            let base_contrast = 1.0 - other_brightness;
            let alt_contrast = other_brightness - alternative.max(0.0);
            if alt_contrast > base_contrast {
                target = alternative;
            }
        }
    }
    target = target.clamp(0.0, 1.0);

    let endpoint = if main_brightness < target { 1.0 } else { 0.0 };
    let denominator = brightness(endpoint - main[0], endpoint - main[1], endpoint - main[2]);
    if denominator.abs() < f32::EPSILON {
        return fg;
    }
    let p = ((target - main_brightness) / denominator).clamp(0.0, 1.0);
    let channel = |index: usize| {
        let value = p * endpoint + (1.0 - p) * main[index];
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Rgb::new(channel(0), channel(1), channel(2))
}

fn resolve_color(color: AnsiColor, colors: &Colors, profile: &ResolvedProfile) -> Rgb {
    match color {
        AnsiColor::Spec(rgb) => Rgb::new(rgb.r, rgb.g, rgb.b),
        AnsiColor::Named(named) => {
            if let Some(rgb) = colors[named as usize] {
                return Rgb::new(rgb.r, rgb.g, rgb.b);
            }
            match named {
                NamedColor::Foreground | NamedColor::BrightForeground => profile.foreground,
                NamedColor::Background => profile.background,
                NamedColor::Cursor => profile.cursor,
                NamedColor::DimForeground => profile.foreground.mix(profile.background, 0.45),
                _ if (named as usize) >= (NamedColor::DimBlack as usize)
                    && (named as usize) <= (NamedColor::DimWhite as usize) =>
                {
                    let index = (named as usize) - (NamedColor::DimBlack as usize);
                    profile.palette[index.min(7)]
                }
                _ => {
                    let index = (named as usize).min(15);
                    profile.palette[index]
                }
            }
        }
        AnsiColor::Indexed(index) => {
            if let Some(rgb) = colors[index as usize] {
                return Rgb::new(rgb.r, rgb.g, rgb.b);
            }
            if (index as usize) < 16 {
                profile.palette[index as usize]
            } else {
                indexed_color(index)
            }
        }
    }
}

fn indexed_color(index: u8) -> Rgb {
    if index < 16 {
        return Rgb::new(0, 0, 0);
    }
    if index < 232 {
        let index = index - 16;
        let r = index / 36;
        let g = (index % 36) / 6;
        let b = index % 6;
        let component = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        Rgb::new(component(r), component(g), component(b))
    } else {
        let level = 8 + (index - 232) * 10;
        Rgb::new(level, level, level)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextStyle {
    fg: Rgb,
    bold: bool,
    italic: bool,
    underline: UnderlineStyle,
    strike: bool,
}

/// Right-to-left scripts are shaped as a run so Arabic letters can join.
/// Everything else is shaped one cluster at a time to keep grid alignment.
fn is_rtl_cluster(text: &str) -> bool {
    text.chars().next().is_some_and(|ch| {
        matches!(
            ch as u32,
            0x0590..=0x08FF
                | 0xFB1D..=0xFDFF
                | 0xFE70..=0xFEFF
                | 0x10800..=0x10FFF
                | 0x1E800..=0x1EFFF
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn build_term_segments(
    font_system: &mut FontSystem,
    session: &Session,
    profile: &ResolvedProfile,
    families: &ResolvedFamilies,
    metrics: FontMetrics,
    cols: usize,
    rows: usize,
    cursor_visible: bool,
) -> Vec<Segment> {
    let term = &session.term;
    let mode = *term.mode();
    let colors = *term.colors();
    let display_offset = term.grid().display_offset();
    let selection = term
        .selection
        .as_ref()
        .and_then(|selection| selection.to_range(term));
    let cursor = term.grid().cursor.point;
    let cursor_viewport = point_to_viewport(display_offset, cursor);
    let cursor_shape = term.cursor_style().shape;
    let cursor_block = cursor_visible
        && mode.contains(TermMode::SHOW_CURSOR)
        && matches!(cursor_shape, CursorShape::Block);

    let features = parse_font_features(profile);
    let mut segments = Vec::new();

    for row in 0..rows {
        let mut run = String::new();
        let mut run_col = 0usize;
        let mut run_width = 0usize;
        let mut run_style: Option<TextStyle> = None;
        let mut rtl_run = String::new();
        let mut rtl_col = 0usize;
        let mut rtl_width = 0usize;
        let mut rtl_style: Option<TextStyle> = None;

        for col in 0..cols {
            let abs = viewport_to_point(
                display_offset,
                alacritty_terminal::index::Point::new(row, Column(col)),
            );
            let cell = &term.grid()[abs];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let on_cursor = cursor_block
                && cursor_viewport
                    .map(|point| point.line == row && point.column == col)
                    .unwrap_or(false);
            let selected = selection.map(|range| range.contains(abs)).unwrap_or(false);
            let (fg, _) = cell_style(cell, profile, &colors, selected, on_cursor);

            let style = TextStyle {
                fg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
                    UnderlineStyle::Double
                } else if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                    UnderlineStyle::Single
                } else {
                    UnderlineStyle::None
                },
                strike: cell.flags.contains(Flags::STRIKEOUT),
            };

            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let width_cells = if wide { 2 } else { 1 };
            let mut text = String::new();
            if cell.flags.contains(Flags::HIDDEN) {
                text.push(' ');
            } else if cell.c.is_control() {
                text.push(' ');
            } else {
                text.push(cell.c);
                if let Some(zerowidth) = cell.zerowidth() {
                    text.extend(zerowidth.iter().copied());
                }
            }
            let ascii = text.len() == 1 && text.is_ascii();

            if ascii {
                if !rtl_run.is_empty() {
                    push_text_segment(
                        font_system,
                        &mut segments,
                        &rtl_run,
                        rtl_style.expect("run has a style"),
                        false,
                        false,
                        profile,
                        families,
                        &features,
                        metrics,
                        cols,
                        row,
                        rtl_col,
                        rtl_width,
                    );
                    rtl_run.clear();
                    rtl_width = 0;
                }
                if !run.is_empty() && run_style != Some(style) {
                    push_text_segment(
                        font_system,
                        &mut segments,
                        &run,
                        run_style.expect("run has a style"),
                        true,
                        false,
                        profile,
                        families,
                        &features,
                        metrics,
                        cols,
                        row,
                        run_col,
                        run_width,
                    );
                    run.clear();
                    run_width = 0;
                }
                if run.is_empty() {
                    run_col = col;
                    run_style = Some(style);
                }
                run.push_str(&text);
                run_width += width_cells;
            } else if is_rtl_cluster(&text) {
                if !run.is_empty() {
                    push_text_segment(
                        font_system,
                        &mut segments,
                        &run,
                        run_style.expect("run has a style"),
                        true,
                        false,
                        profile,
                        families,
                        &features,
                        metrics,
                        cols,
                        row,
                        run_col,
                        run_width,
                    );
                    run.clear();
                    run_width = 0;
                }
                if !rtl_run.is_empty() && rtl_style != Some(style) {
                    push_text_segment(
                        font_system,
                        &mut segments,
                        &rtl_run,
                        rtl_style.expect("run has a style"),
                        false,
                        false,
                        profile,
                        families,
                        &features,
                        metrics,
                        cols,
                        row,
                        rtl_col,
                        rtl_width,
                    );
                    rtl_run.clear();
                    rtl_width = 0;
                }
                if rtl_run.is_empty() {
                    rtl_col = col;
                    rtl_style = Some(style);
                }
                rtl_run.push_str(&text);
                rtl_width += width_cells;
            } else {
                if !run.is_empty() {
                    push_text_segment(
                        font_system,
                        &mut segments,
                        &run,
                        run_style.expect("run has a style"),
                        true,
                        false,
                        profile,
                        families,
                        &features,
                        metrics,
                        cols,
                        row,
                        run_col,
                        run_width,
                    );
                    run.clear();
                    run_width = 0;
                }
                if !rtl_run.is_empty() {
                    push_text_segment(
                        font_system,
                        &mut segments,
                        &rtl_run,
                        rtl_style.expect("run has a style"),
                        false,
                        false,
                        profile,
                        families,
                        &features,
                        metrics,
                        cols,
                        row,
                        rtl_col,
                        rtl_width,
                    );
                    rtl_run.clear();
                    rtl_width = 0;
                }
                push_text_segment(
                    font_system,
                    &mut segments,
                    &text,
                    style,
                    false,
                    width_cells > 1,
                    profile,
                    families,
                    &features,
                    metrics,
                    cols,
                    row,
                    col,
                    width_cells,
                );
            }
        }

        if !run.is_empty() {
            push_text_segment(
                font_system,
                &mut segments,
                &run,
                run_style.expect("run has a style"),
                true,
                false,
                profile,
                families,
                &features,
                metrics,
                cols,
                row,
                run_col,
                run_width,
            );
        }
        if !rtl_run.is_empty() {
            push_text_segment(
                font_system,
                &mut segments,
                &rtl_run,
                rtl_style.expect("run has a style"),
                false,
                false,
                profile,
                families,
                &features,
                metrics,
                cols,
                row,
                rtl_col,
                rtl_width,
            );
        }
    }

    segments
}

#[allow(clippy::too_many_arguments)]
fn push_text_segment(
    font_system: &mut FontSystem,
    segments: &mut Vec<Segment>,
    text: &str,
    style: TextStyle,
    ascii: bool,
    centered: bool,
    _profile: &ResolvedProfile,
    families: &ResolvedFamilies,
    features: &FontFeatures,
    metrics: FontMetrics,
    _cols: usize,
    row: usize,
    col: usize,
    width_cells: usize,
) {
    let family_name = if ascii {
        families.primary.as_deref()
    } else {
        families
            .non_ascii
            .as_deref()
            .or(families.primary.as_deref())
    };
    let attrs = text_attrs(
        family_of(family_name),
        color(style.fg),
        style.bold,
        style.italic,
        style.underline,
        style.strike,
        features,
    );
    let target = width_cells as f32 * metrics.cell_w;
    // RTL clusters are aligned to the right edge of their buffer, so a
    // non-ASCII cluster must be shaped in a buffer no wider than its cells.
    // ASCII runs get a little extra room to avoid spurious wrapping.
    let buffer_width = if ascii {
        (target + metrics.cell_w * 2.0).max(1.0)
    } else {
        target.max(1.0)
    };
    let mut buffer = Buffer::new(font_system, metrics.metrics);
    buffer.set_size(Some(buffer_width), Some(metrics.line_px.max(1.0)));
    buffer.set_text(
        text,
        &attrs,
        if ascii {
            Shaping::Basic
        } else {
            Shaping::Advanced
        },
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    let line_w = buffer
        .layout_runs()
        .next()
        .map(|run| run.line_w)
        .unwrap_or(0.0);
    let x_offset = if centered {
        ((target - line_w) / 2.0).max(0.0)
    } else {
        0.0
    };
    segments.push(Segment {
        buffer,
        row,
        col,
        width_cells,
        x_offset,
    });
}

fn text_attrs<'a>(
    family: Family<'a>,
    fg: Color,
    bold: bool,
    italic: bool,
    underline: UnderlineStyle,
    strike: bool,
    features: &FontFeatures,
) -> Attrs<'a> {
    let mut attrs = Attrs::new()
        .family(family)
        .color(fg)
        .font_features(features.clone());
    if bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if italic {
        attrs = attrs.style(Style::Italic);
    }
    if underline != UnderlineStyle::None {
        attrs = attrs.underline(underline);
    }
    if strike {
        attrs = attrs.strikethrough();
    }
    attrs
}

fn create_quad_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("riterm quads shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("quads.wgsl").into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("riterm quads pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<QuadVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 8,
                        shader_location: 1,
                    },
                ],
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod shaping_tests {
    use super::*;

    fn shape(text: &str) -> (f32, usize, bool) {
        let mut font_system = FontSystem::new();
        let metrics = Metrics::new(16.0, 19.2);
        let mut buffer = Buffer::new(&mut font_system, metrics);
        buffer.set_size(Some(400.0), Some(19.2));
        let attrs = Attrs::new()
            .family(Family::Monospace)
            .color(Color::rgb(255, 255, 255));
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut font_system, false);

        let mut glyphs = 0;
        let mut width: f32 = 0.0;
        let mut saw_rtl = false;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            glyphs += run.glyphs.len();
            saw_rtl |= run.glyphs.iter().any(|glyph| glyph.level.is_rtl());
        }
        (width, glyphs, saw_rtl)
    }

    #[test]
    fn shapes_unicode_clusters() {
        for text in ["ا", "ب", "العربية", "Ελληνικά", "日本語", "😀"] {
            let (width, glyphs, _) = shape(text);
            assert!(glyphs > 0, "{text:?} produced no glyphs");
            assert!(width > 0.0, "{text:?} produced no width");
            assert!(width.is_finite(), "{text:?} produced a non-finite width");
        }
    }

    #[test]
    fn arabic_shapes_right_to_left() {
        let (_, glyphs, saw_rtl) = shape("العربية");
        assert!(glyphs > 1, "Arabic should join into a shaped run");
        assert!(saw_rtl, "Arabic glyphs should be flagged right-to-left");
    }
}

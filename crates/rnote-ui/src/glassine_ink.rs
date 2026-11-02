//! Pen preview support for the Glassine e-ink session (glassine_ink_v1). The panel
//! engine draws the pen's stroke straight
//! from the pen's events, long before Rnote's frame arrives; this tells the
//! compositor where Rnote's canvas is (minus its floating toolbars) and which
//! brush matches the current tool, so the preview draws only there and looks
//! like Rnote's stroke. Does nothing on other compositors (no glassine_ink_manager_v1
//! global) or off Wayland. Updated after every engine event (pointer hover
//! included), sent only when it changed. Version 3 compositors get brush
//! objects (grey, pressure curve, Rnote's textured dots as tip masks); version
//! 2 ones the black-or-white set_brush.

use crate::{RnAppWindow, RnCanvas};
use gdk4_wayland::prelude::WaylandSurfaceExtManual;
use gtk4::{gdk, glib, graphene, prelude::*};
use rnote_compose::Color;
use rnote_compose::style::PressureCurve;
use rnote_compose::style::textured::TexturedDotsDistribution;
use rnote_engine::pens::pensconfig::brushconfig::BrushStyle;
use rnote_engine::pens::pensconfig::eraserconfig::EraserStyle;
use rnote_engine::pens::pensconfig::PensConfig;
use rnote_engine::pens::{PenMode, PenStyle};
use std::cell::RefCell;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_compositor, wl_region, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

#[allow(non_upper_case_globals, non_camel_case_types, dead_code, clippy::all)]
mod protocol {
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("src/glassine-ink-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("src/glassine-ink-v1.xml");
}
use protocol::{glassine_ink_brush_v1, glassine_ink_manager_v1::GlassineInkManagerV1, glassine_ink_surface_v1};
use protocol::glassine_ink_brush_v1::GlassineInkBrushV1;
use protocol::glassine_ink_surface_v1::GlassineInkSurfaceV1;

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents,
        _: &Connection, _: &QueueHandle<Self>) {}
}
wayland_client::delegate_noop!(State: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(State: ignore wl_region::WlRegion);
wayland_client::delegate_noop!(State: ignore GlassineInkManagerV1);
wayland_client::delegate_noop!(State: ignore GlassineInkSurfaceV1);
wayland_client::delegate_noop!(State: ignore GlassineInkBrushV1);

/// (kind, width_min, width_max, pressure_exponent) in surface coordinates;
/// kind as glassine_ink_surface_v1.kind: 1 none, 2 pen, 3 erase
type Brush = (u32, f64, f64, f64);
const NONE: Brush = (1, 0.0, 0.0, 1.0);

/// A version 3 brush's look (sizes relative to the stroke: zoom-free)
#[derive(Clone, PartialEq)]
enum Look {
    None,
    /// a round dab: Rnote's solid pen and marker, the eraser (erase: whitens); a marker darkens, a pen replaces (any colour, white on black too)
    Disc { grey: u8, erase: bool, marker: bool, curve: [u8; 16] },
    /// Rnote's textured pen: elliptic dots in our own tip masks
    Dots { grey: u8, curve: [u8; 16], dots: Dots },
}

#[derive(Clone, Copy, PartialEq)]
struct Dots {
    width: f64,
    density: f64,
    distribution: TexturedDotsDistribution,
}

/// a look and its diameter range (width x zoom) for one end of the pen
#[derive(Clone, PartialEq)]
struct Preview {
    look: Look,
    width: f64,
}

/// one end of the pen on a version 3 surface
#[derive(Default)]
struct Slot {
    /// the brush object and the look it holds
    obj: Option<(GlassineInkBrushV1, Look)>,
    sent: Option<Preview>,
}

struct Conn {
    conn: Connection,
    queue: EventQueue<State>,
    mgr: GlassineInkManagerV1,
    comp: wl_compositor::WlCompositor,
    version: u32,
}

struct Win {
    win: glib::WeakRef<RnAppWindow>,
    surface: wl_surface::WlSurface,
    ink: GlassineInkSurfaceV1,
    areas: Vec<(i32, i32, i32, i32)>,
    /// version 2: what set_brush sent
    brushes: [Option<Brush>; 2],
    /// version 3
    slots: [Slot; 2],
}

impl Win {
    fn forget(&mut self, ink: bool) {
        if ink {
            self.ink.destroy();
        }
        for s in &mut self.slots {
            if let Some((b, _)) = s.obj.take() {
                b.destroy();
            }
        }
    }
}

thread_local! {
    /// None inside: tried, not available (not our compositor)
    static CONN: RefCell<Option<Option<Conn>>> = const { RefCell::new(None) };
    static WINS: RefCell<Vec<Win>> = const { RefCell::new(Vec::new()) };
}

fn connect(display: &gdk::Display) -> Option<Conn> {
    let display = display.downcast_ref::<gdk4_wayland::WaylandDisplay>()?;
    let wl_display = display.wl_display()?;
    let conn = Connection::from_backend(wl_display.backend().upgrade()?);
    // our own queue: GTK's dispatching never sees these objects
    let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = queue.handle();
    let mgr = globals.bind::<GlassineInkManagerV1, _, _>(&qh, 2..=3, ()).ok()?;
    let comp = globals.bind::<wl_compositor::WlCompositor, _, _>(&qh, 1..=4, ()).ok()?;
    let version = mgr.version();
    Some(Conn { conn, queue, mgr, comp, version })
}

fn luminance(c: &Color) -> f64 {
    // composited over white paper
    let l = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    l * c.a + (1.0 - c.a)
}

/// width x zoom, the pressure curve as an exponent (Rnote: width x curve(pressure))
fn smooth(width: f64, color: Option<Color>, curve: PressureCurve, zoom: f64) -> Brush {
    match color {
        Some(c) if luminance(&c) < 0.6 => {}
        _ => return NONE, // light colours: the e-ink preview is black only
    }
    let w = width * zoom;
    let exp = match curve {
        PressureCurve::Const => return (2, w, w, 1.0),
        PressureCurve::Linear => 1.0,
        PressureCurve::Sqrt => 0.5,
        PressureCurve::Cbrt => 1.0 / 3.0,
        PressureCurve::Pow2 => 2.0,
        PressureCurve::Pow3 => 3.0,
    };
    (2, 0.0, w, exp)
}

/// the pen style `mode` (pen or eraser end) uses now, Rnote's pen settings and the zoom
fn with_style<R>(
    appwindow: &RnAppWindow,
    canvas: &RnCanvas,
    mode: PenMode,
    f: impl FnOnce(PenStyle, &PensConfig, f64) -> R,
) -> R {
    let engine = canvas.engine_ref();
    let config = appwindow.engine_config();
    let config = config.read();
    let pc = &config.pens_config;
    let mut state = engine.penholder.pen_mode_state();
    let current = state.pen_mode() == mode;
    state.set_pen_mode(mode);
    let style = if current {
        state.current_style_w_override(pc)
    } else {
        state.style(pc)
    };
    f(style, pc, engine.camera.total_zoom())
}

/// version 2: a black pen or a white eraser
fn brush_for(appwindow: &RnAppWindow, canvas: &RnCanvas, mode: PenMode) -> Brush {
    with_style(appwindow, canvas, mode, |style, pc, zoom| match style {
        PenStyle::Brush => match pc.brush_config.style {
            BrushStyle::Marker => NONE, // a highlighter
            BrushStyle::Solid => {
                let o = &pc.brush_config.solid_options;
                smooth(o.stroke_width, o.stroke_color, o.pressure_curve, zoom)
            }
            BrushStyle::Textured => {
                let o = &pc.brush_config.textured_options;
                smooth(o.stroke_width, o.stroke_color, o.pressure_curve, zoom)
            }
        },
        PenStyle::Eraser => match pc.eraser_config.style {
            // removes what it touches: whiten its width
            EraserStyle::SplitCollidingStrokes => {
                let w = pc.eraser_config.width * zoom;
                (3, w, w, 1.0)
            }
            // removes whole strokes: the preview can't know which
            EraserStyle::TrashCollidingStrokes => NONE,
        },
        // shapes, text, selection, tools: nothing to preview
        _ => NONE,
    })
}

/// the colour's grey over white paper (0 black .. 255 white); None: not drawn
fn grey(color: Option<Color>) -> Option<u8> {
    color.map(|c| (luminance(&c).clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Rnote's pressure curve (width x curve(pressure)) at 0, 1/15 .. 1
fn curve16(curve: PressureCurve) -> [u8; 16] {
    std::array::from_fn(|i| (curve.apply(1.0, i as f64 / 15.0).clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// version 3: the look of Rnote's current pen style
fn preview_for(appwindow: &RnAppWindow, canvas: &RnCanvas, mode: PenMode) -> Preview {
    let none = Preview { look: Look::None, width: 0.0 };
    with_style(appwindow, canvas, mode, |style, pc, zoom| match style {
        PenStyle::Brush => {
            let b = &pc.brush_config;
            // solid and marker: a disc in the colour's grey; darken keeps the
            // text under a marker visible (the engine dithers greys)
            let o = match b.style {
                BrushStyle::Marker => &*b.marker_options,
                BrushStyle::Solid => &*b.solid_options,
                BrushStyle::Textured => {
                    let o = &b.textured_options;
                    let Some(grey) = grey(o.stroke_color) else { return none };
                    let dots = Dots { width: o.stroke_width, density: o.density, distribution: o.distribution };
                    let look = Look::Dots { grey, curve: curve16(o.pressure_curve), dots };
                    return Preview { look, width: dots.span() * zoom };
                }
            };
            let Some(grey) = grey(o.stroke_color) else { return none };
            let look = Look::Disc { grey, erase: false, marker: matches!(b.style, BrushStyle::Marker), curve: curve16(o.pressure_curve) };
            Preview { look, width: o.stroke_width * zoom }
        }
        PenStyle::Eraser => match pc.eraser_config.style {
            EraserStyle::SplitCollidingStrokes => {
                let look = Look::Disc { grey: 255, erase: true, marker: false, curve: [255; 16] };
                Preview { look, width: pc.eraser_config.width * zoom }
            }
            EraserStyle::TrashCollidingStrokes => none,
        },
        _ => none,
    })
}

const TIP: usize = 48;
const VARIANTS: usize = 8;

impl Dots {
    /// Rnote's dot radii (they grow with the width) at most
    fn radii(&self) -> (f64, f64) {
        let k = 1.0 + self.width * 0.1;
        (1.2 * k * 1.25, 0.3 * k * 1.25)
    }

    /// a mask's side in document units: the stroke's width plus room for the dots
    /// that stick out of it
    fn span(&self) -> f64 {
        self.width.max(0.1) + 2.0 * self.radii().0
    }
}

/// Rnote's textured dots (rnote-compose style/textured): elliptic dots, rotated
/// up to 22.5 degrees off the stroke's direction (mask x = the direction),
/// along the stroke uniform, across it by the distribution, centres inside the
/// stroke's width. Dabs are spaced half a mask apart, so each mask holds the
/// dots of half a mask's length of stroke (Rnote's 0.1 x density per unit area).
fn dot_atlas(d: &Dots) -> Vec<u8> {
    use rand::RngExt;
    use std::f64::consts::FRAC_PI_8;
    let (w, m) = (d.width.max(0.1), d.span());
    let px = TIP as f64 / m; // mask pixels per document unit
    let (rx, ry) = d.radii();
    let (rx, ry) = (rx / 1.25, ry / 1.25);
    let n = (0.5 * m * w * 0.1 * d.density).round().clamp(1.0, 4000.0) as usize;
    let mut rng = rnote_compose::utils::new_rng_default_pcg64(Some(0x6e61_3169));
    let mut atlas = vec![0u8; TIP * TIP * VARIANTS];
    for mask in atlas.chunks_mut(TIP * TIP) {
        for _ in 0..n {
            let x = rng.random_range(-m / 4.0..m / 4.0);
            let y = d.distribution.sample_for_range_symmetrical_clipped(&mut rng, -w / 2.0..w / 2.0);
            let rot = rng.random_range(-FRAC_PI_8..FRAC_PI_8);
            let a = rng.random_range(rx * 0.8..rx * 1.25) * px;
            let b = rng.random_range(ry * 0.8..ry * 1.25) * px;
            // dots below half a pixel would vanish from the mask
            ellipse(mask, (x + m / 2.0) * px, (y + m / 2.0) * px, a.max(0.5), b.max(0.5), rot);
        }
    }
    atlas
}

/// adds a rotated ellipse's coverage (4x4 samples a pixel) to a mask
fn ellipse(mask: &mut [u8], cx: f64, cy: f64, a: f64, b: f64, rot: f64) {
    let (sin, cos) = rot.sin_cos();
    let r = a.max(b);
    let range = |c: f64| (c - r).floor().max(0.0) as usize..((c + r).ceil().max(0.0) as usize).min(TIP);
    for py in range(cy) {
        for px in range(cx) {
            let mut hits = 0;
            for s in 0..16 {
                let dx = px as f64 + (s % 4) as f64 / 4.0 + 0.125 - cx;
                let dy = py as f64 + (s / 4) as f64 / 4.0 + 0.125 - cy;
                let (u, v) = ((dx * cos + dy * sin) / a, (dy * cos - dx * sin) / b);
                if u * u + v * v <= 1.0 {
                    hits += 1;
                }
            }
            let m = &mut mask[py * TIP + px];
            *m = m.saturating_add((hits * 255 / 16) as u8);
        }
    }
}

/// a read-only-mappable fd with the bytes (a memfd)
fn memfd(bytes: &[u8]) -> Option<OwnedFd> {
    use std::os::unix::fs::FileExt;
    // SAFETY: a fresh fd we own (or -1)
    let fd = unsafe { libc::memfd_create(c"glassine-ink-tip".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    // at offset 0, the file position stays at the start
    file.write_all_at(bytes, 0).ok()?;
    Some(file.into())
}

/// a new brush object holding the look
fn create_brush(mgr: &GlassineInkManagerV1, qh: &QueueHandle<State>, look: &Look) -> GlassineInkBrushV1 {
    use glassine_ink_brush_v1::{AngleMode, Blend, Curve, InkKind};
    let brush = mgr.create_brush(qh, ());
    let (grey, erase, curve, blend) = match look {
        Look::Disc { grey, erase, marker, curve } => {
            brush.set_shape(1.0, 0.2, 1.0, 0.0, AngleMode::Fixed);
            (*grey, *erase, curve, if *marker { Blend::Darken } else { Blend::Replace })
        }
        Look::Dots { grey, curve, dots } => {
            brush.set_shape(1.0, 0.5, 1.0, 0.0, AngleMode::Direction);
            if let Some(fd) = memfd(&dot_atlas(dots)) {
                brush.set_tip(fd.as_fd(), TIP as u32, VARIANTS as u32);
            }
            (*grey, false, curve, Blend::Replace)
        }
        Look::None => return brush, // (not selected: set_brush none instead)
    };
    let kind = if erase { InkKind::Erase } else { InkKind::Ink };
    brush.set_ink(kind, grey as u32, 255, blend);
    brush.set_curve(Curve::PressureSize, curve.to_vec());
    brush
}

/// version 3: select the preview for one end of the pen if it changed; true if sent
fn send_preview(conn: &Conn, qh: &QueueHandle<State>, w: &mut Win, k: usize, p: Preview) -> bool {
    let slot = &mut w.slots[k];
    if slot.sent.as_ref() == Some(&p) {
        return false;
    }
    let tool = if k == 0 {
        glassine_ink_surface_v1::Tool::Tip
    } else {
        glassine_ink_surface_v1::Tool::Eraser
    };
    if p.look == Look::None {
        w.ink.set_brush(tool, glassine_ink_surface_v1::Kind::None, 0.0, 0.0, 1.0);
    } else {
        // a changed look gets a fresh object (the compositor keeps the old
        // one's snapshot until the new one is selected)
        let old = match &slot.obj {
            Some((_, look)) if *look == p.look => None,
            _ => {
                let obj = (create_brush(&conn.mgr, qh, &p.look), p.look.clone());
                slot.obj.replace(obj)
            }
        };
        let brush = &slot.obj.as_ref().unwrap().0;
        w.ink.set_brush_object(tool, Some(brush), 0.0, p.width);
        if let Some((b, _)) = old {
            b.destroy();
        }
    }
    slot.sent = Some(p);
    true
}

/// a widget's box in the window's surface coordinates
fn surface_box(w: &impl IsA<gtk4::Widget>, root: &gtk4::Native, off: (f64, f64)) -> Option<graphene::Rect> {
    if !w.is_visible() || !w.is_mapped() {
        return None;
    }
    let b = w.compute_bounds(root)?;
    Some(graphene::Rect::new(b.x() + off.0 as f32, b.y() + off.1 as f32, b.width(), b.height()))
}

fn subtract(rects: Vec<(i32, i32, i32, i32)>, c: (i32, i32, i32, i32)) -> Vec<(i32, i32, i32, i32)> {
    let mut out = Vec::new();
    let (cx0, cy0, cx1, cy1) = (c.0, c.1, c.0 + c.2, c.1 + c.3);
    for (x, y, w, h) in rects {
        let (x1, y1) = (x + w, y + h);
        if cx1 <= x || cx0 >= x1 || cy1 <= y || cy0 >= y1 {
            out.push((x, y, w, h));
            continue;
        }
        if cy0 > y {
            out.push((x, y, w, cy0 - y));
        }
        if cy1 < y1 {
            out.push((x, cy1, w, y1 - cy1));
        }
        let (my0, my1) = (y.max(cy0), y1.min(cy1));
        if cx0 > x {
            out.push((x, my0, cx0 - x, my1 - my0));
        }
        if cx1 < x1 {
            out.push((cx1, my0, x1 - cx1, my1 - my0));
        }
    }
    out
}

fn rect_i(r: &graphene::Rect, pad: f32) -> (i32, i32, i32, i32) {
    let (x0, y0) = ((r.x() - pad).floor() as i32, (r.y() - pad).floor() as i32);
    let (x1, y1) = ((r.x() + r.width() + pad).ceil() as i32, (r.y() + r.height() + pad).ceil() as i32);
    (x0, y0, x1 - x0, y1 - y0)
}

/// the canvas's visible part minus the toolbars floating over it
fn areas_for(appwindow: &RnAppWindow, canvas: &RnCanvas) -> Vec<(i32, i32, i32, i32)> {
    let native = appwindow.upcast_ref::<gtk4::Native>();
    let off = native.surface_transform();
    let overlays = appwindow.overlays();
    let Some(cb) = surface_box(canvas, native, off) else {
        return Vec::new();
    };
    let cb = match surface_box(&overlays.tabview(), native, off).and_then(|t| t.intersection(&cb)) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut rects = vec![rect_i(&cb, 0.0)];
    let floating: [Option<graphene::Rect>; 4] = [
        surface_box(&overlays.penpicker(), native, off),
        surface_box(&overlays.colorpicker(), native, off),
        surface_box(&overlays.sidebar_box(), native, off),
        surface_box(&overlays.progressbar(), native, off),
    ];
    for f in floating.iter().flatten() {
        rects = subtract(rects, rect_i(f, 4.0));
    }
    rects.retain(|r| r.2 > 0 && r.3 > 0);
    rects
}

/// After the engine handled an event (hover included): send what changed.
pub(crate) fn update(appwindow: &RnAppWindow, canvas: &RnCanvas) {
    let Some(surface) = appwindow.surface() else {
        return;
    };
    let Some(wl_surface) = surface
        .downcast_ref::<gdk4_wayland::WaylandSurface>()
        .and_then(|s| s.wl_surface())
    else {
        return;
    };
    CONN.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(connect(&WidgetExt::display(appwindow)));
        }
        let Some(Some(conn)) = c.as_mut() else {
            return;
        };
        WINS.with(|wins| {
            let mut wins = wins.borrow_mut();
            // forget windows that are gone or got a new surface
            wins.retain_mut(|w| match w.win.upgrade() {
                Some(win) if &win == appwindow => {
                    if w.surface == wl_surface {
                        true
                    } else {
                        w.forget(true);
                        false
                    }
                }
                Some(_) => true,
                None => {
                    w.forget(w.surface.is_alive());
                    false
                }
            });
            let qh = conn.queue.handle();
            let i = match wins.iter().position(|w| w.win.upgrade().as_ref() == Some(appwindow)) {
                Some(i) => i,
                None => {
                    // a resize (the window tiled, rotated, made fullscreen) moves the
                    // canvas without an engine event: update once GTK has laid it out
                    surface.connect_layout(glib::clone!(
                        #[weak]
                        appwindow,
                        move |_, _, _| {
                            glib::idle_add_local_once(glib::clone!(
                                #[weak]
                                appwindow,
                                move || {
                                    if let Some(w) = appwindow.active_tab_wrapper() {
                                        update(&appwindow, &w.canvas());
                                    }
                                }
                            ));
                        }
                    ));
                    let ink = conn.mgr.get_ink_surface(&wl_surface, &qh, ());
                    wins.push(Win {
                        win: appwindow.downgrade(),
                        surface: wl_surface.clone(),
                        ink,
                        areas: vec![(-1, -1, -1, -1)],
                        brushes: [None, None],
                        slots: Default::default(),
                    });
                    wins.len() - 1
                }
            };
            let w = &mut wins[i];
            let mut sent = false;
            let areas = areas_for(appwindow, canvas);
            if areas != w.areas {
                let region = conn.comp.create_region(&qh, ());
                for r in &areas {
                    region.add(r.0, r.1, r.2, r.3);
                }
                w.ink.set_areas(Some(&region));
                region.destroy();
                w.areas = areas;
                sent = true;
            }
            for (k, mode) in [PenMode::Pen, PenMode::Eraser].into_iter().enumerate() {
                // (at most 8 brush objects in use per client: 4 windows x 2 ends;
                // more windows get the version 2 brushes)
                if conn.version >= 3 && i < 4 {
                    sent |= send_preview(conn, &qh, w, k, preview_for(appwindow, canvas, mode));
                    continue;
                }
                let b = brush_for(appwindow, canvas, mode);
                if w.brushes[k] != Some(b) {
                    let tool = if k == 0 {
                        glassine_ink_surface_v1::Tool::Tip
                    } else {
                        glassine_ink_surface_v1::Tool::Eraser
                    };
                    let kind = match b.0 {
                        2 => glassine_ink_surface_v1::Kind::Pen,
                        3 => glassine_ink_surface_v1::Kind::Erase,
                        _ => glassine_ink_surface_v1::Kind::None,
                    };
                    w.ink.set_brush(tool, kind, b.1, b.2, b.3);
                    w.brushes[k] = Some(b);
                    sent = true;
                }
            }
            if sent {
                let _ = conn.conn.flush();
            }
        });
    });
}

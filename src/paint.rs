//! Hand-drawn pieces: the flowing colour bands, the record, sleeves, the
//! waveform and the transport glyphs. Everything is plain meshes, no assets.

use std::f32::consts::{PI, TAU};

use eframe::egui::{
    self, pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke,
    StrokeKind, TextureId,
};

use crate::art::Palette;

pub const INK: Color32 = Color32::from_rgb(12, 13, 17);
pub const PANEL: Color32 = Color32::from_rgb(16, 17, 22);
pub const TEXT: Color32 = Color32::from_rgb(240, 240, 244);
pub const DIM: Color32 = Color32::from_rgb(138, 141, 152);
pub const FAINT: Color32 = Color32::from_rgb(70, 73, 84);

pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

pub fn shade(c: Color32, k: f32) -> Color32 {
    let f = |x: u8| (x as f32 * k).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

// ------------------------------------------------------------------ bands

/// Layered, slowly drifting ribbons of the album's two colours.
/// Horizontal inset of a rounded rect's edge at height `y`.
fn corner_inset(rect: Rect, radius: f32, y: f32) -> f32 {
    let dy = if y < rect.top() + radius {
        rect.top() + radius - y
    } else if y > rect.bottom() - radius {
        y - (rect.bottom() - radius)
    } else {
        return 0.0;
    };
    radius - (radius * radius - dy.min(radius).powi(2)).max(0.0).sqrt()
}

/// Row heights for meshes that must follow rounded corners: dense near the
/// top and bottom edges, sparse in between.
fn rows(rect: Rect, radius: f32, middle: usize) -> Vec<f32> {
    const CORNER: usize = 10;
    let mut ys = Vec::with_capacity(CORNER * 2 + middle + 2);
    for i in 0..CORNER {
        let a = i as f32 / CORNER as f32 * std::f32::consts::FRAC_PI_2;
        ys.push(rect.top() + radius * (1.0 - a.cos()));
    }
    let (a, b) = (rect.top() + radius, rect.bottom() - radius);
    for i in 0..=middle {
        ys.push(a + (b - a) * i as f32 / middle as f32);
    }
    for i in (0..CORNER).rev() {
        let a = i as f32 / CORNER as f32 * std::f32::consts::FRAC_PI_2;
        ys.push(rect.bottom() - radius * (1.0 - a.cos()));
    }
    ys
}

/// Layered, slowly drifting ribbons of the album's two colours, filling a
/// rounded card.
pub fn bands(p: &Painter, rect: Rect, radius: f32, t: f32, pal: Palette, energy: f32) {
    const N: usize = 7;
    p.rect_filled(rect, radius, shade(pal.a, 0.78));
    let ys = rows(rect, radius, 36);

    let amp = rect.width() * (0.035 + 0.012 * energy);
    for i in 0..N {
        let f = i as f32 / (N - 1) as f32;
        let fill = shade(lerp_color(pal.a, pal.b, (f * 1.15).min(1.0)), 0.88 - 0.24 * f);
        let base = rect.left() + rect.width() * (0.2 + 0.125 * i as f32);
        let phase = i as f32 * 0.85;

        // Soft shadow the ribbon casts on whatever lies to its left.
        let mut shadow = Mesh::default();
        let mut body = Mesh::default();
        for (s, &y) in ys.iter().enumerate() {
            let v = (y - rect.top()) / rect.height();
            let lean = (0.5 - v) * rect.width() * 0.3;
            let wobble = (v * 4.2 + t * 0.22 + phase).sin() * amp + (v * 9.0 - t * 0.31 + phase * 1.7).sin() * amp * 0.35;
            let inset = corner_inset(rect, radius, y);
            let (lo, hi) = (rect.left() + inset, rect.right() - inset);
            let x = (base + lean + wobble).clamp(lo, hi);
            shadow.colored_vertex(pos2((x - 22.0).max(lo), y), Color32::TRANSPARENT);
            shadow.colored_vertex(pos2(x, y), Color32::from_black_alpha(70));
            body.colored_vertex(pos2(x, y), fill);
            body.colored_vertex(pos2(hi, y), fill);
            if s > 0 {
                let k = (s as u32) * 2;
                for m in [&mut shadow, &mut body] {
                    m.add_triangle(k - 2, k - 1, k);
                    m.add_triangle(k - 1, k, k + 1);
                }
            }
        }
        p.add(Shape::mesh(shadow));
        p.add(Shape::mesh(body));
    }
}

/// A floating glass panel: soft shadow, translucent fill, a hairline
/// highlight along the top edge.
pub fn glass(p: &Painter, rect: Rect, radius: f32, fill: Color32) {
    for (grow, a, dy) in [(14.0, 10, 6.0), (8.0, 16, 4.0), (3.0, 26, 2.0)] {
        let r = rect.translate(vec2(0.0, dy)).expand(grow);
        p.rect_filled(r, CornerRadius::same((radius + grow).min(255.0) as u8), Color32::from_black_alpha(a));
    }
    let cr = CornerRadius::same(radius.min(255.0) as u8);
    p.rect_filled(rect, cr, fill);
    p.rect_stroke(rect, cr, Stroke::new(1.0, Color32::from_white_alpha(16)), StrokeKind::Inside);
    // Light catching the top edge.
    let top = Rect::from_min_size(rect.min + vec2(radius, 0.5), vec2(rect.width() - radius * 2.0, 1.0));
    p.rect_filled(top, 0.0, Color32::from_white_alpha(18));
}

// ----------------------------------------------------------------- vinyl

fn ring_mesh(c: Pos2, r0: f32, r1: f32, col: impl Fn(f32) -> (Color32, Color32)) -> Mesh {
    const SEG: u32 = 96;
    let mut m = Mesh::default();
    for i in 0..=SEG {
        let a = i as f32 / SEG as f32 * TAU;
        let d = vec2(a.cos(), a.sin());
        let (inner, outer) = col(a);
        m.colored_vertex(c + d * r0, inner);
        m.colored_vertex(c + d * r1, outer);
        if i > 0 {
            let k = i * 2;
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k, k + 1);
        }
    }
    m
}

/// A textured disc, rotated by `angle`.
fn label_mesh(c: Pos2, r: f32, angle: f32, tex: TextureId) -> Mesh {
    const SEG: u32 = 64;
    let mut m = Mesh::with_texture(tex);
    let v = |pos: Pos2, uv: Pos2| egui::epaint::Vertex { pos, uv, color: Color32::WHITE };
    m.vertices.push(v(c, pos2(0.5, 0.5)));
    for i in 0..=SEG {
        let a = i as f32 / SEG as f32 * TAU;
        let d = vec2((a + angle).cos(), (a + angle).sin());
        m.vertices.push(v(c + d * r, pos2(0.5 + a.cos() * 0.5, 0.5 + a.sin() * 0.5)));
        if i > 0 {
            m.add_triangle(0, i, i + 1);
        }
    }
    m
}

pub struct Vinyl<'a> {
    pub center: Pos2,
    pub radius: f32,
    pub angle: f32,
    pub label: Option<TextureId>,
    pub palette: Palette,
    /// Track boundaries as 0..1 fractions of the side; drawn as groove gaps.
    pub gaps: &'a [f32],
    /// Where the "needle" is, 0..1 across the side.
    pub progress: Option<f32>,
}

/// Radii of the grooved area, as fractions of the disc radius.
const GROOVE_OUT: f32 = 0.955;
const GROOVE_IN: f32 = 0.43;
const LABEL: f32 = 0.36;

pub fn vinyl(p: &Painter, v: &Vinyl) {
    let (c, r) = (v.center, v.radius);

    // Drop shadow, offset down-right.
    p.add(Shape::mesh(ring_mesh(c + vec2(r * 0.03, r * 0.05), r * 0.9, r * 1.12, |_| {
        (Color32::from_black_alpha(110), Color32::TRANSPARENT)
    })));
    p.circle_filled(c, r, Color32::from_rgb(9, 9, 11));

    // Grooves: many faint rings whose brightness drifts with radius.
    let span = GROOVE_OUT - GROOVE_IN;
    let rings = ((r * span) / 2.6).clamp(12.0, 64.0) as usize;
    for i in 0..rings {
        let f = i as f32 / rings as f32;
        let a = 9.0 + 9.0 * ((f * 37.0).sin() * 0.5 + 0.5) + 6.0 * ((f * 11.0 + 1.3).sin() * 0.5 + 0.5);
        p.circle_stroke(c, r * (GROOVE_OUT - span * f), Stroke::new(1.0, Color32::from_white_alpha(a as u8)));
    }
    // The silent bands between tracks.
    for &g in v.gaps {
        if g > 0.01 && g < 0.99 {
            p.circle_stroke(c, r * (GROOVE_OUT - span * g), Stroke::new((r * 0.012).max(1.5), Color32::from_rgb(5, 5, 6)));
        }
    }
    if let Some(pr) = v.progress {
        let pr = pr.clamp(0.0, 1.0);
        p.circle_stroke(c, r * (GROOVE_OUT - span * pr), Stroke::new(1.5, alpha(v.palette.a, 0.9)));
    }

    // Light catching the grooves: two fixed wedges, so the label visibly
    // spins underneath while the sheen stays put.
    p.add(Shape::mesh(ring_mesh(c, r * (LABEL + 0.03), r * 0.985, |a| {
        let k = (a - PI * 0.28).cos().powi(2).powf(9.0);
        let k2 = (a - PI * 0.78).cos().powi(2).powf(40.0) * 0.35;
        let w = Color32::from_white_alpha(((k * 0.16 + k2 * 0.1) * 255.0) as u8);
        (alpha(w, 0.55), w)
    })));
    p.circle_stroke(c, r - 0.5, Stroke::new(1.0, Color32::from_white_alpha(26)));

    // Label.
    let lr = r * LABEL;
    match v.label {
        Some(tex) => {
            p.add(Shape::mesh(label_mesh(c, lr, v.angle, tex)));
        }
        None => {
            p.circle_filled(c, lr, v.palette.a);
            // An off-centre pattern so rotation still reads without artwork.
            let d = vec2(v.angle.cos(), v.angle.sin());
            p.circle_filled(c + d * lr * 0.42, lr * 0.34, shade(v.palette.b, 0.9));
            p.circle_stroke(c, lr * 0.78, Stroke::new(1.0, Color32::from_black_alpha(50)));
        }
    }
    p.circle_stroke(c, lr, Stroke::new(1.0, Color32::from_black_alpha(140)));
    p.circle_stroke(c, lr * 0.2, Stroke::new(1.0, Color32::from_black_alpha(60)));
    p.circle_filled(c, (r * 0.022).max(1.5), INK);
}

/// Cheap record for collection tiles: a few rings instead of full grooves.
pub fn vinyl_mini(p: &Painter, c: Pos2, r: f32, pal: Palette) {
    p.circle_filled(c + vec2(1.0, 2.0), r + 1.0, Color32::from_black_alpha(70));
    p.circle_filled(c, r, Color32::from_rgb(24, 25, 30));
    for f in [0.88, 0.72, 0.56] {
        p.circle_stroke(c, r * f, Stroke::new(1.0, Color32::from_white_alpha(26)));
    }
    p.circle_stroke(c, r - 0.5, Stroke::new(1.0, Color32::from_white_alpha(46)));
    p.circle_filled(c, r * LABEL, pal.a);
    p.circle_filled(c, r * 0.05, INK);
}

// ---------------------------------------------------------------- sleeve

/// A record sleeve: cover art, or a generated two-tone cover with an initial.
pub fn sleeve(p: &Painter, rect: Rect, tex: Option<TextureId>, pal: Palette, title: &str, radius: f32) {
    let cr = CornerRadius::same(radius as u8);
    for (grow, a) in [(10.0, 14), (6.0, 22), (2.5, 34)] {
        let r = rect.translate(vec2(0.0, rect.height() * 0.025)).expand(grow);
        p.rect_filled(r, CornerRadius::same((radius + grow) as u8), Color32::from_black_alpha(a));
    }
    match tex {
        Some(tex) => {
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            p.add(egui::epaint::RectShape::filled(rect, cr, Color32::WHITE).with_texture(tex, uv));
        }
        None => {
            p.rect_filled(rect, cr, shade(pal.a, 0.8));
            let clip = p.with_clip_rect(rect.intersect(p.clip_rect()));
            let s = rect.width();
            clip.circle_filled(rect.left_bottom() + vec2(s * 0.15, 0.0), s * 0.62, shade(pal.b, 0.85));
            clip.circle_filled(rect.left_bottom() + vec2(s * 0.15, 0.0), s * 0.36, shade(pal.a, 0.55));
            if let Some(ch) = title.chars().find(|c| c.is_alphanumeric()) {
                clip.text(
                    rect.right_top() + vec2(-s * 0.1, s * 0.06),
                    egui::Align2::RIGHT_TOP,
                    ch.to_uppercase(),
                    display(s * 0.34),
                    Color32::from_white_alpha(225),
                );
            }
        }
    }
    p.rect_stroke(rect, cr, Stroke::new(1.0, Color32::from_white_alpha(22)), StrokeKind::Inside);
}

// -------------------------------------------------------------- waveform

pub struct Wave<'a> {
    pub peaks: Option<&'a [u8]>,
    /// Playback position 0..1.
    pub progress: f32,
    /// Pointer position 0..1 while hovering.
    pub hover: Option<f32>,
    pub accent: Color32,
    /// Live output level 0..1.
    pub level: f32,
    pub time: f32,
    /// 0..1 fade from the placeholder shape into the real peaks.
    pub appear: f32,
}

pub fn waveform(p: &Painter, rect: Rect, w: &Wave) {
    const PITCH: f32 = 3.0;
    let n = (rect.width() / PITCH).floor().max(1.0) as usize;
    let mid = rect.center().y;
    let half = rect.height() * 0.5;
    let play_x = rect.left() + rect.width() * w.progress;
    let mut mesh = Mesh::default();

    for i in 0..n {
        let f = (i as f32 + 0.5) / n as f32;
        let x = rect.left() + f * rect.width();

        // Idle shape: a slow travelling swell, like a signal at rest.
        let rest = 0.1 + 0.07 * (f * 9.0 + w.time * 0.6).sin() * (f * 23.0 - w.time * 0.35).cos();
        let amp = match w.peaks {
            Some(peaks) => {
                let pos = f * (peaks.len() - 1) as f32;
                let (a, b) = (pos.floor() as usize, (pos.ceil() as usize).min(peaks.len() - 1));
                let v = peaks[a] as f32 + (peaks[b] as f32 - peaks[a] as f32) * pos.fract();
                rest + (v / 255.0 - rest) * w.appear
            }
            None => rest,
        };

        // Bars near the needle breathe with the music.
        let dx = (x - play_x) / 46.0;
        let pulse = (-dx * dx).exp() * w.level;
        let h = ((amp * (1.0 + 0.45 * pulse)).clamp(0.02, 1.0) * half).max(1.0);

        let played = x <= play_x;
        let mut col = if played { w.accent } else { Color32::from_rgb(196, 200, 214) };
        let mut a = if played { 0.95 } else { 0.42 + 0.3 * pulse };
        if let Some(hv) = w.hover {
            let hx = rect.left() + rect.width() * hv;
            if (x > play_x && x <= hx) || (x <= play_x && x > hx) {
                col = lerp_color(col, w.accent, 0.5);
                a = 0.7;
            }
        }
        // Each bar is solid at the centre line and fades towards its tips.
        let core = alpha(col, a);
        let tip = alpha(col, a * 0.18);
        let hw = 0.75;
        let k = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(x - hw, mid - h), tip);
        mesh.colored_vertex(pos2(x + hw, mid - h), tip);
        mesh.colored_vertex(pos2(x - hw, mid), core);
        mesh.colored_vertex(pos2(x + hw, mid), core);
        mesh.colored_vertex(pos2(x - hw, mid + h), tip);
        mesh.colored_vertex(pos2(x + hw, mid + h), tip);
        for q in [0, 2] {
            mesh.add_triangle(k + q, k + q + 1, k + q + 2);
            mesh.add_triangle(k + q + 1, k + q + 2, k + q + 3);
        }
    }
    p.add(Shape::mesh(mesh));

    // Needle.
    p.line_segment(
        [pos2(play_x, rect.top() - 4.0), pos2(play_x, rect.bottom() + 4.0)],
        Stroke::new(1.5, w.accent),
    );
    p.circle_filled(pos2(play_x, rect.top() - 4.0), 2.5, w.accent);
    if let Some(hv) = w.hover {
        let hx = rect.left() + rect.width() * hv;
        p.line_segment([pos2(hx, rect.top()), pos2(hx, rect.bottom())], Stroke::new(1.0, Color32::from_white_alpha(90)));
    }
}

// ----------------------------------------------------------------- icons

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Prev,
    Shuffle,
    Repeat,
    RepeatOne,
    Back,
    Plus,
    Refresh,
    Volume,
    /// Keep a streamed song: arrow into a tray.
    Save,
    Check,
    /// Add to playlist: lines with a plus.
    AddList,
    /// Shrink to the mini player.
    Mini,
    /// Back to the full window.
    Expand,
    /// Playlist glyph: lines with a note.
    List,
}

pub fn icon(p: &Painter, c: Pos2, s: f32, icon: Icon, col: Color32) {
    let tri = |pts: [Pos2; 3]| {
        p.add(Shape::convex_polygon(pts.to_vec(), col, Stroke::NONE));
    };
    let line = |a: Pos2, b: Pos2| {
        p.line_segment([a, b], Stroke::new((s * 0.14).max(1.4), col));
    };
    let at = |x: f32, y: f32| c + vec2(x, y) * s;
    match icon {
        Icon::Play => tri([at(-0.32, -0.5), at(-0.32, 0.5), at(0.55, 0.0)]),
        Icon::Pause => {
            for x in [-0.3, 0.3] {
                p.rect_filled(Rect::from_center_size(at(x, 0.0), vec2(0.24, 1.0) * s), 1.0, col);
            }
        }
        Icon::Next | Icon::Prev => {
            let d = if icon == Icon::Next { 1.0 } else { -1.0 };
            tri([at(-0.5 * d, -0.45), at(-0.5 * d, 0.45), at(0.28 * d, 0.0)]);
            p.rect_filled(Rect::from_center_size(at(0.42 * d, 0.0), vec2(0.16, 0.9) * s), 1.0, col);
        }
        Icon::Shuffle => {
            for d in [1.0, -1.0] {
                let pts = [at(-0.6, -0.32 * d), at(-0.25, -0.32 * d), at(0.25, 0.32 * d), at(0.6, 0.32 * d)];
                p.add(Shape::line(pts.to_vec(), Stroke::new((s * 0.13).max(1.3), col)));
                tri([at(0.5, 0.32 * d - 0.2), at(0.5, 0.32 * d + 0.2), at(0.78, 0.32 * d)]);
            }
        }
        Icon::Repeat | Icon::RepeatOne => {
            let r = Rect::from_center_size(c, vec2(1.25, 0.72) * s);
            p.rect_stroke(r, CornerRadius::same((s * 0.3) as u8), Stroke::new((s * 0.13).max(1.3), col), StrokeKind::Middle);
            tri([at(0.05, -0.6), at(0.05, -0.12), at(0.36, -0.36)]);
            if icon == Icon::RepeatOne {
                p.circle_filled(at(0.0, 0.0), s * 0.13, col);
            }
        }
        Icon::Back => {
            line(at(0.2, -0.5), at(-0.3, 0.0));
            line(at(-0.3, 0.0), at(0.2, 0.5));
        }
        Icon::Plus => {
            line(at(-0.5, 0.0), at(0.5, 0.0));
            line(at(0.0, -0.5), at(0.0, 0.5));
        }
        Icon::Refresh => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -PI * 0.3 + i as f32 / 20.0 * TAU * 0.8;
                    c + vec2(a.cos(), a.sin()) * s * 0.5
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new((s * 0.13).max(1.3), col)));
            tri([at(0.18, -0.62), at(0.62, -0.5), at(0.3, -0.16)]);
        }
        Icon::Save => {
            line(at(0.0, -0.55), at(0.0, 0.2));
            line(at(-0.3, -0.08), at(0.0, 0.22));
            line(at(0.3, -0.08), at(0.0, 0.22));
            let w = (s * 0.13).max(1.3);
            p.add(Shape::line(vec![at(-0.55, 0.15), at(-0.55, 0.55), at(0.55, 0.55), at(0.55, 0.15)], Stroke::new(w, col)));
        }
        Icon::Check => {
            let w = (s * 0.15).max(1.4);
            p.add(Shape::line(vec![at(-0.5, 0.0), at(-0.15, 0.38), at(0.55, -0.4)], Stroke::new(w, col)));
        }
        Icon::AddList => {
            line(at(-0.6, -0.4), at(0.2, -0.4));
            line(at(-0.6, 0.0), at(0.2, 0.0));
            line(at(-0.6, 0.4), at(-0.05, 0.4));
            line(at(0.42, 0.05), at(0.42, 0.65));
            line(at(0.12, 0.35), at(0.72, 0.35));
        }
        Icon::List => {
            line(at(-0.6, -0.4), at(0.1, -0.4));
            line(at(-0.6, 0.0), at(0.1, 0.0));
            line(at(-0.6, 0.4), at(-0.1, 0.4));
            p.circle_filled(at(0.32, 0.45), s * 0.17, col);
            line(at(0.45, 0.45), at(0.45, -0.5));
            line(at(0.45, -0.5), at(0.7, -0.35));
        }
        Icon::Mini | Icon::Expand => {
            let w = (s * 0.12).max(1.2);
            let outer = Rect::from_center_size(c, vec2(1.3, 1.0) * s);
            p.rect_stroke(outer, CornerRadius::same((s * 0.18) as u8), Stroke::new(w, col), StrokeKind::Middle);
            let small = if icon == Icon::Mini {
                Rect::from_min_max(at(0.0, 0.05), at(0.5, 0.38))
            } else {
                Rect::from_min_max(at(-0.5, -0.38), at(0.5, 0.38))
            };
            p.rect_filled(small, 1.5, col);
        }
        Icon::Volume => {
            p.add(Shape::convex_polygon(
                vec![at(-0.6, -0.2), at(-0.3, -0.2), at(0.05, -0.5), at(0.05, 0.5), at(-0.3, 0.2), at(-0.6, 0.2)],
                col,
                Stroke::NONE,
            ));
            let pts: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let a = -0.9 + i as f32 / 8.0 * 1.8;
                    c + vec2(0.1 + a.cos() * 0.45, a.sin() * 0.45) * s
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new((s * 0.12).max(1.2), col)));
        }
    }
}

/// Three little bars that dance on the playing track's row.
pub fn eq_bars(p: &Painter, c: Pos2, col: Color32, t: f32, active: bool) {
    for i in 0..3 {
        let phase = t * (5.0 + i as f32 * 1.7) + i as f32 * 2.1;
        let h = if active { 4.0 + 8.0 * (phase.sin() * 0.5 + 0.5) } else { 4.0 };
        let x = c.x + (i as f32 - 1.0) * 4.5;
        p.rect_filled(Rect::from_min_max(pos2(x - 1.25, c.y + 6.0 - h), pos2(x + 1.25, c.y + 6.0)), 1.0, col);
    }
}

/// Lay out `text` on one line, shortening with an ellipsis to fit `width`.
pub fn elided(p: &Painter, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, ..Default::default() };
    p.layout_job(job)
}

/// Letter-spaced caption text. Returns the rect it occupies.
pub fn spaced(p: &Painter, pos: Pos2, anchor: egui::Align2, text: &str, font: FontId, color: Color32, spacing: f32) -> Rect {
    let mut job = egui::text::LayoutJob::default();
    let format = egui::TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() };
    job.append(text, 0.0, format);
    let galley = p.layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
    rect
}

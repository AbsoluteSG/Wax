//! Floaty window motion and a subtle 3D tilt.
//!
//! * Dragging moves the window through a spring, so it trails the cursor a
//!   little and glides to rest (with a small fling) when let go.
//! * The UI leans as if seen from the middle of the screen, and swings with
//!   the drag velocity. The lean is a true perspective warp applied to the
//!   finished UI meshes; pointer input is mapped back through its inverse so
//!   every click still lands where it looks like it does.

use eframe::egui::{self, epaint, layers::ShapeIdx, vec2, LayerId, Pos2, Rect, Shape, Vec2};

use crate::platform::{self, Monitor};

/// Lean at the screen edge when resting (radians, ~1.1°).
const STATIC_TILT: f32 = 0.019;
/// Extra lean per px/s of window speed.
const SWING: f32 = 2.4e-5;
const MAX_TILT: f32 = 0.07;
/// Perspective distance, in points.
const DEPTH: f32 = 2600.0;

#[derive(Default)]
pub struct Motion {
    hwnd: Option<isize>,
    pub monitor: Option<Monitor>,
    monitor_at: f64,
    dragging: bool,
    grab: Vec2,
    /// The window is being moved by the spring (physical px).
    pub moving: bool,
    pos: Vec2,
    vel: Vec2,
    target: Vec2,
    /// (around X, around Y) in radians, and its spring velocity.
    tilt: Vec2,
    tilt_vel: Vec2,
    /// The warp applied to this frame's UI, if any.
    pub warp: Option<Warp>,
}

impl Motion {
    pub fn refresh_hz(&self) -> f32 {
        self.monitor.map_or(60.0, |m| m.refresh_hz)
    }

    /// Start following the cursor (call on drag start).
    pub fn begin_drag(&mut self) {
        let (Some(h), Some(c)) = (self.hwnd, platform::cursor()) else { return };
        let Some(wr) = platform::window_rect(h) else { return };
        if !self.moving {
            self.pos = wr.min.to_vec2();
            self.vel = Vec2::ZERO;
        }
        self.grab = c.to_vec2() - self.pos;
        self.target = self.pos;
        self.dragging = true;
        self.moving = true;
    }

    /// Advance the springs. Returns true while anything is still in motion
    /// (the caller should then repaint at full rate).
    pub fn update(&mut self, frame: &eframe::Frame, time: f64, dt: f32, primary_down: bool, tilt_on: bool, content: Rect) -> bool {
        self.hwnd = platform::hwnd_of(frame);
        let Some(hwnd) = self.hwnd else { return false };
        if self.monitor.is_none() || (!self.moving && time - self.monitor_at > 2.0) {
            self.monitor = platform::monitor(hwnd);
            self.monitor_at = time;
        }

        // --- window position spring
        if self.moving {
            if self.dragging {
                if let Some(c) = platform::cursor() {
                    self.target = c.to_vec2() - self.grab;
                }
                if !primary_down {
                    self.dragging = false;
                    self.fling(hwnd);
                }
            }
            // Slightly under-damped: trails the cursor, settles with a soft overshoot.
            let (w, zeta) = (20.0, 0.66);
            let steps = ((dt / 0.004).ceil() as usize).clamp(1, 12);
            let h = dt / steps as f32;
            for _ in 0..steps {
                let acc = (self.target - self.pos) * (w * w) - self.vel * (2.0 * zeta * w);
                self.vel += acc * h;
                self.pos += self.vel * h;
            }
            if !self.dragging && (self.target - self.pos).length() < 0.4 && self.vel.length() < 6.0 {
                self.pos = self.target;
                self.vel = Vec2::ZERO;
                self.moving = false;
                self.monitor = platform::monitor(hwnd);
            }
            platform::move_window(hwnd, self.pos.x.round() as i32, self.pos.y.round() as i32);
        }

        // --- tilt spring
        let mut goal = Vec2::ZERO;
        if tilt_on {
            if let (Some(m), Some(wr)) = (self.monitor, platform::window_rect(hwnd)) {
                let half = m.bounds.size() * 0.5;
                let off = (wr.center() - m.bounds.center()) / half;
                let off = off.clamp(Vec2::splat(-1.0), Vec2::splat(1.0));
                goal = vec2(-off.y, off.x) * STATIC_TILT;
            }
            let v = if self.moving { self.vel } else { Vec2::ZERO };
            goal += vec2(v.y, -v.x) * SWING;
            goal = goal.clamp(Vec2::splat(-MAX_TILT), Vec2::splat(MAX_TILT));
        }
        let (w, zeta) = (11.0, 0.5);
        let steps = ((dt / 0.008).ceil() as usize).clamp(1, 8);
        let h = dt / steps as f32;
        for _ in 0..steps {
            let acc = (goal - self.tilt) * (w * w) - self.tilt_vel * (2.0 * zeta * w);
            self.tilt_vel += acc * h;
            self.tilt += self.tilt_vel * h;
        }
        let tilting = (goal - self.tilt).length() > 1e-4 || self.tilt_vel.length() > 1e-3;
        if !tilting {
            self.tilt = goal;
            self.tilt_vel = Vec2::ZERO;
        }
        self.warp = (self.tilt.length() > 4e-4).then(|| Warp::new(content, self.tilt.x, self.tilt.y));
        self.moving || tilting
    }

    /// On release, carry some of the throw, but stay on screen.
    fn fling(&mut self, hwnd: isize) {
        let throw = self.vel * 0.09;
        let mut t = self.target + throw;
        if let (Some(m), Some(wr)) = (self.monitor, platform::window_rect(hwnd)) {
            let lo = m.work.min.to_vec2();
            let hi = m.work.max.to_vec2() - wr.size();
            // Only the throw is limited: a window placed off screen stays put.
            t.x = t.x.clamp(lo.x.min(self.target.x), hi.x.max(self.target.x));
            t.y = t.y.clamp(lo.y.min(self.target.y), hi.y.max(self.target.y));
        }
        self.target = t;
    }

    /// Apply this frame's warp to the main UI layer, leaving the shape index
    /// ranges in `keep` (window backdrop, traffic lights) flat.
    pub fn warp_layer(&self, ctx: &egui::Context, layer: LayerId, keep: &[(usize, usize)]) {
        let Some(warp) = self.warp else { return };
        let ppp = ctx.pixels_per_point();
        let options = ctx.tessellation_options(|o| *o);
        let font_size = ctx.fonts(|f| f.font_image_size());
        let mut tess = epaint::Tessellator::new(ppp, options, font_size, Vec::new());
        ctx.graphics_mut(|g| {
            let list = g.entry(layer);
            let n = list.next_idx().0;
            for i in 0..n {
                if keep.iter().any(|&(a, b)| i >= a && i < b) {
                    continue;
                }
                list.mutate_shape(ShapeIdx(i), |cs| {
                    if !cs.clip_rect.is_positive() {
                        return;
                    }
                    let shape = std::mem::replace(&mut cs.shape, Shape::Noop);
                    // Skip what's scrolled out of view, as egui itself would.
                    let bounds = shape.visual_bounding_rect();
                    if bounds.is_finite() && !bounds.intersects(cs.clip_rect) {
                        return;
                    }
                    tess.set_clip_rect(cs.clip_rect);
                    let mut out = Vec::new();
                    to_meshes(&mut tess, shape, &mut out);
                    for s in &mut out {
                        if let Shape::Mesh(m) = s {
                            let m = std::sync::Arc::make_mut(m);
                            for v in &mut m.vertices {
                                v.pos = warp.apply(v.pos);
                            }
                        }
                    }
                    if cs.clip_rect.is_finite() {
                        cs.clip_rect = warp.apply_rect(cs.clip_rect).expand(1.0);
                    }
                    cs.shape = if out.len() == 1 { out.pop().unwrap() } else { Shape::Vec(out) };
                });
            }
        });
    }

    /// Map pointer input from where things look to where they are.
    pub fn unwarp_input(&self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(warp) = self.warp else { return };
        let flat = |p: Pos2| ctx.layer_id_at(p).is_none_or(|l| l == LayerId::background());
        for e in &mut raw.events {
            match e {
                egui::Event::PointerMoved(p) if flat(*p) => *p = warp.invert(*p),
                egui::Event::PointerButton { pos, .. } if flat(*pos) => *pos = warp.invert(*pos),
                _ => {}
            }
        }
    }
}

fn to_meshes(tess: &mut epaint::Tessellator, shape: Shape, out: &mut Vec<Shape>) {
    match shape {
        Shape::Noop => {}
        Shape::Vec(v) => {
            for s in v {
                to_meshes(tess, s, out);
            }
        }
        // Custom GPU callbacks can't be warped; keep them as they are.
        Shape::Callback(_) => out.push(shape),
        s => {
            let mut mesh = epaint::Mesh::default();
            tess.tessellate_shape(s, &mut mesh);
            if !mesh.is_empty() {
                out.push(Shape::mesh(mesh));
            }
        }
    }
}

/// A perspective rotation of the UI plane about its centre, as a 3×3
/// homography (with its inverse for mapping input back).
#[derive(Clone, Copy, Debug)]
pub struct Warp {
    m: [[f32; 3]; 3],
    inv: [[f32; 3]; 3],
}

impl Warp {
    /// `ax`/`ay`: rotation about the X / Y axis, in radians.
    pub fn new(rect: Rect, ax: f32, ay: f32) -> Self {
        let c = rect.center();
        let d = DEPTH;
        let (sx, cx) = ax.sin_cos();
        let (sy, cy) = ay.sin_cos();
        // Shrink a touch so the leaning edge stays inside the window.
        let hw = rect.width() * 0.5;
        let hh = rect.height() * 0.5;
        let near = d / (d - hw * sy.abs() - hh * sx.abs()).max(1.0);
        let fit = 1.0 / near;
        // p' = c + fit * (X/W, Y/W), with (relative) x, y:
        //   X = d·cy·x,  Y = d·sy·sx·x + d·cx·y,  W = -sy·cx·x + sx·y + d
        let h = [[d * cy * fit, 0.0, 0.0], [d * sy * sx * fit, d * cx * fit, 0.0], [-sy * cx, sx, d]];
        // Conjugate with translation by the centre.
        let t = [[1.0, 0.0, c.x], [0.0, 1.0, c.y], [0.0, 0.0, 1.0]];
        let ti = [[1.0, 0.0, -c.x], [0.0, 1.0, -c.y], [0.0, 0.0, 1.0]];
        let m = mul(mul(t, h), ti);
        Self { m, inv: inverse(m) }
    }

    #[inline]
    pub fn apply(&self, p: Pos2) -> Pos2 {
        project(&self.m, p)
    }

    pub fn invert(&self, p: Pos2) -> Pos2 {
        project(&self.inv, p)
    }

    pub fn apply_rect(&self, r: Rect) -> Rect {
        let pts = [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()].map(|p| self.apply(p));
        Rect::from_points(&pts)
    }
}

#[inline]
fn project(m: &[[f32; 3]; 3], p: Pos2) -> Pos2 {
    let x = m[0][0] * p.x + m[0][1] * p.y + m[0][2];
    let y = m[1][0] * p.x + m[1][1] * p.y + m[1][2];
    let w = m[2][0] * p.x + m[2][1] * p.y + m[2][2];
    Pos2::new(x / w, y / w)
}

fn mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn inverse(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = m.map(|r| r.map(|v| v as f64));
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let k = if det.abs() < 1e-12 { 0.0 } else { 1.0 / det };
    let r = [
        [(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k],
        [(f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k],
        [(d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k],
    ];
    r.map(|row| row.map(|v| v as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    #[test]
    fn warp_round_trips() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1320.0, 820.0));
        let w = Warp::new(rect, 0.03, -0.05);
        for p in [pos2(10.0, 10.0), pos2(660.0, 410.0), pos2(1300.0, 800.0), pos2(200.0, 700.0)] {
            let back = w.invert(w.apply(p));
            assert!((back - p).length() < 0.01, "{p:?} -> {back:?}");
        }
        // The centre stays put and corners stay inside the rect.
        assert!((w.apply(rect.center()) - rect.center()).length() < 0.01);
        for c in [rect.left_top(), rect.right_bottom(), rect.right_top(), rect.left_bottom()] {
            assert!(rect.expand(0.5).contains(w.apply(c)), "{:?}", w.apply(c));
        }
    }
}

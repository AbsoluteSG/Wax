//! Renders Wax's app icon: a record on flowing colour bands inside a
//! rounded square. Writes `assets/wax.ico` (all Windows sizes) and
//! `assets/icon.png` (window icon).
//!
//!     cargo run --example gen_icon

use std::f32::consts::PI;

const CORAL: [f32; 3] = [0.93, 0.32, 0.27];
/// Bands from the top left: two teals, then corals deepening to red.
const PALETTE: [[f32; 3]; 6] = [
    [0.24, 0.72, 0.74],
    [0.17, 0.56, 0.62],
    [0.13, 0.42, 0.50],
    [0.96, 0.45, 0.35],
    [0.91, 0.30, 0.26],
    [0.76, 0.20, 0.22],
];

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Colour and coverage at a point of the unit icon. `detail` drops fine
/// texture at small sizes where it would only turn into noise.
fn sample(u: f32, v: f32, detail: bool) -> [f32; 4] {
    // Rounded square (superellipse) with the usual margin.
    let (x, y) = ((u - 0.5) / 0.44, (v - 0.5) / 0.44);
    let n = 5.0;
    if x.abs().powf(n) + y.abs().powf(n) > 1.0 {
        return [0.0; 4];
    }

    // Flowing bands, teal at the top left turning coral at the bottom right.
    let w = u * 0.85 + v * 0.35 + 0.06 * (v * 7.0 + 0.6).sin() + 0.03 * (v * 15.0).sin();
    let band = ((w * 4.2).floor().max(0.0) as usize).min(PALETTE.len() - 1);
    let mut c = PALETTE[band];
    // Soft shadow at each band's leading edge.
    let edge = (w * 4.2).fract();
    c = scale(c, 0.78 + 0.22 * smooth(0.0, 0.18, edge));

    // The record.
    let (cx, cy, r) = (0.5, 0.5, 0.33);
    let (dx, dy) = (u - cx, v - cy);
    let d = (dx * dx + dy * dy).sqrt();
    // Drop shadow under it.
    let sd = ((u - cx - 0.015).powi(2) + (v - cy - 0.03).powi(2)).sqrt();
    c = scale(c, 1.0 - 0.45 * (1.0 - smooth(r - 0.02, r + 0.07, sd)));

    if d <= r {
        let a = dy.atan2(dx);
        let mut g = 0.045;
        if detail {
            // Grooves and a dark ring between "tracks".
            g += 0.025 * (d * 420.0).sin().abs();
            if (d - r * 0.72).abs() < 0.006 {
                g *= 0.4;
            }
        }
        // Light catching the grooves in two opposing wedges.
        let sheen = (a - PI * 0.25).cos().powi(2).powf(10.0) * 0.22;
        g += sheen * smooth(0.13, 0.2, d);
        let mut rec = [g, g, g * 1.05];
        // Label: the two colours split diagonally.
        let label = 0.135;
        if d <= label {
            let t = smooth(-0.02, 0.02, dx * 0.7 + dy * 0.7);
            rec = scale(mix(CORAL, PALETTE[0], t), 1.0 - 0.15 * (d / label));
            if d < label * 0.62 && d > label * 0.58 && detail {
                rec = scale(rec, 0.8);
            }
        }
        if d <= 0.018 {
            rec = [0.04, 0.04, 0.05];
        }
        // Thin bright rim.
        let rim = smooth(r - 0.008, r, d) * 0.25;
        c = [rec[0] + rim, rec[1] + rim, rec[2] + rim];
    }

    // Gentle top highlight across the whole tile.
    let hl = (1.0 - v).powi(3) * 0.08;
    [c[0] + hl, c[1] + hl, c[2] + hl, 1.0]
}

fn render(size: u32) -> image::RgbaImage {
    let ss = 6; // supersampling per axis
    let detail = size >= 48;
    image::RgbaImage::from_fn(size, size, |px, py| {
        let mut acc = [0.0f32; 4];
        for sy in 0..ss {
            for sx in 0..ss {
                let u = (px as f32 + (sx as f32 + 0.5) / ss as f32) / size as f32;
                let v = (py as f32 + (sy as f32 + 0.5) / ss as f32) / size as f32;
                let s = sample(u, v, detail);
                // Accumulate premultiplied so edges blend correctly.
                for i in 0..3 {
                    acc[i] += s[i].clamp(0.0, 1.0) * s[3];
                }
                acc[3] += s[3];
            }
        }
        let n = (ss * ss) as f32;
        let a = acc[3] / n;
        let un = |c: f32| if a > 0.0 { (c / n / a).clamp(0.0, 1.0) } else { 0.0 };
        image::Rgba([(un(acc[0]) * 255.0) as u8, (un(acc[1]) * 255.0) as u8, (un(acc[2]) * 255.0) as u8, (a * 255.0) as u8])
    })
}

fn main() {
    std::fs::create_dir_all("assets").unwrap();
    let sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256];
    let frames: Vec<_> = sizes
        .iter()
        .map(|&s| {
            let img = render(s);
            image::codecs::ico::IcoFrame::as_png(img.as_raw(), s, s, image::ExtendedColorType::Rgba8).unwrap()
        })
        .collect();
    let file = std::fs::File::create("assets/wax.ico").unwrap();
    image::codecs::ico::IcoEncoder::new(file).encode_images(&frames).unwrap();
    render(256).save("assets/icon.png").unwrap();
    // Preview sheet: large plus the small sizes side by side.
    let mut sheet = image::RgbaImage::from_pixel(1024 + 16 + 256, 1024, image::Rgba([40, 42, 48, 255]));
    image::imageops::overlay(&mut sheet, &render(1024), 0, 0);
    let mut y = 16;
    for s in [256, 64, 48, 32, 24, 16] {
        image::imageops::overlay(&mut sheet, &render(s), 1040, y);
        y += s as i64 + 16;
    }
    sheet.save(std::env::temp_dir().join("wax_icon_sheet.png")).unwrap();
    println!("wrote assets/wax.ico and assets/icon.png");
}

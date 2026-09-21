//! Generative pieces, composed at the exact width of the print head.
//!
//! Everything here draws into a Canvas of ink values; dithering happens later,
//! so a generator can work in continuous tone and still print crisply.

use crate::raster::{Canvas, DOTS};
use tiny_skia::{Paint, PathBuilder, Pixmap, Stroke, Transform};

/// xorshift, seeded from the clock — enough randomness for pictures,
/// and it keeps the dependency list short.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: Option<u64>) -> Self {
        let s = seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x2545F4914F6CDD1D)
        });
        Rng(s | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.f32() * (hi - lo)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
}

pub const GENERATORS: &[(&str, &str)] = &[
    ("flow", "flow field of drifting strands"),
    ("rule30", "one-dimensional cellular automaton, grown downward"),
    ("maze", "perfect maze, recursive backtracker"),
    ("truchet", "random Truchet arcs woven into a tiling"),
    ("moire", "interfering ring patterns"),
    ("hilbert", "space-filling Hilbert curve"),
    ("stipple", "Voronoi-ish stipple field"),
    ("waves", "stacked ridgeline horizon"),
];

pub fn generate(name: &str, height: usize, rng: &mut Rng) -> Option<Canvas> {
    Some(match name {
        "flow" => flow(height, rng),
        "rule30" => automaton(height, rng),
        "maze" => maze(height, rng),
        "truchet" => truchet(height, rng),
        "moire" => moire(height, rng),
        "hilbert" => hilbert(height),
        "stipple" => stipple(height, rng),
        "waves" => waves(height, rng),
        _ => return None,
    })
}

fn pixmap(height: usize) -> Pixmap {
    let mut pm = Pixmap::new(DOTS as u32, height as u32).expect("pixmap");
    pm.fill(tiny_skia::Color::WHITE);
    pm
}

fn black() -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(tiny_skia::Color::BLACK);
    paint.anti_alias = true;
    paint
}

/// Strands released into a smoothly varying vector field.
fn flow(height: usize, rng: &mut Rng) -> Canvas {
    let mut pm = pixmap(height);
    let paint = black();
    let stroke = Stroke { width: 0.8, ..Default::default() };
    let (sx, sy) = (rng.range(0.004, 0.012), rng.range(0.004, 0.012));
    let twist = rng.range(0.5, 2.5);

    for _ in 0..1400 {
        let mut x = rng.f32() * DOTS as f32;
        let mut y = rng.f32() * height as f32;
        let mut pb = PathBuilder::new();
        pb.move_to(x, y);
        for _ in 0..70 {
            let a = ((x * sx).sin() + (y * sy).cos()) * twist;
            x += a.cos() * 3.0;
            y += a.sin() * 3.0;
            if x < 0.0 || y < 0.0 || x >= DOTS as f32 || y >= height as f32 {
                break;
            }
            pb.line_to(x, y);
        }
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    Canvas::from_pixmap(&pm)
}

/// Rule 30 and friends: a single live cell, grown one row at a time.
fn automaton(height: usize, rng: &mut Rng) -> Canvas {
    let rules = [30u8, 90, 110, 150, 105, 22];
    let rule = rules[rng.below(rules.len())];
    let scale = 3usize;
    let cells = DOTS / scale;
    let mut row = vec![false; cells];
    row[cells / 2] = true;
    let mut canvas = Canvas::new(DOTS, height);

    for y in 0..height / scale {
        for (x, &on) in row.iter().enumerate() {
            if on {
                for dy in 0..scale {
                    for dx in 0..scale {
                        canvas.set(x * scale + dx, y * scale + dy, 1.0);
                    }
                }
            }
        }
        let mut next = vec![false; cells];
        for x in 0..cells {
            let l = row[(x + cells - 1) % cells] as u8;
            let c = row[x] as u8;
            let r = row[(x + 1) % cells] as u8;
            let idx = (l << 2) | (c << 1) | r;
            next[x] = rule >> idx & 1 == 1;
        }
        row = next;
    }
    canvas
}

/// Perfect maze by recursive backtracking, drawn as walls.
fn maze(height: usize, rng: &mut Rng) -> Canvas {
    let cell = 16usize;
    let (cols, rows) = (DOTS / cell, height / cell);
    // walls[y][x] = [top, right, bottom, left]
    let mut walls = vec![[true; 4]; cols * rows];
    let mut seen = vec![false; cols * rows];
    let mut stack = vec![0usize];
    seen[0] = true;

    while let Some(&current) = stack.last() {
        let (cx, cy) = (current % cols, current / cols);
        let mut options = Vec::new();
        if cy > 0 && !seen[current - cols] {
            options.push((current - cols, 0, 2));
        }
        if cx + 1 < cols && !seen[current + 1] {
            options.push((current + 1, 1, 3));
        }
        if cy + 1 < rows && !seen[current + cols] {
            options.push((current + cols, 2, 0));
        }
        if cx > 0 && !seen[current - 1] {
            options.push((current - 1, 3, 1));
        }
        if options.is_empty() {
            stack.pop();
            continue;
        }
        let (next, here, there) = options[rng.below(options.len())];
        walls[current][here] = false;
        walls[next][there] = false;
        seen[next] = true;
        stack.push(next);
    }

    let mut pm = pixmap(height);
    let paint = black();
    let stroke = Stroke { width: 2.0, ..Default::default() };
    let mut pb = PathBuilder::new();
    for y in 0..rows {
        for x in 0..cols {
            let (px, py) = ((x * cell) as f32, (y * cell) as f32);
            let c = y * cols + x;
            if walls[c][0] {
                pb.move_to(px, py);
                pb.line_to(px + cell as f32, py);
            }
            if walls[c][3] {
                pb.move_to(px, py);
                pb.line_to(px, py + cell as f32);
            }
            if y == rows - 1 && walls[c][2] {
                pb.move_to(px, py + cell as f32);
                pb.line_to(px + cell as f32, py + cell as f32);
            }
            if x == cols - 1 && walls[c][1] {
                pb.move_to(px + cell as f32, py);
                pb.line_to(px + cell as f32, py + cell as f32);
            }
        }
    }
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    Canvas::from_pixmap(&pm)
}

/// Quarter-circle arcs, randomly oriented per tile: the classic weave.
fn truchet(height: usize, rng: &mut Rng) -> Canvas {
    let tile = [24usize, 32, 48][rng.below(3)];
    let mut pm = pixmap(height);
    let paint = black();
    let stroke = Stroke { width: rng.range(1.5, 3.5), ..Default::default() };
    let mut pb = PathBuilder::new();
    let t = tile as f32;
    let k = 0.5522848 * (t / 2.0); // circle approximation with cubics

    for y in (0..height).step_by(tile) {
        for x in (0..DOTS).step_by(tile) {
            let (x, y) = (x as f32, y as f32);
            let flip = rng.chance(0.5);
            let (a, b) = if flip { (0.0, t) } else { (t, 0.0) };
            // arc from mid-left to mid-top
            pb.move_to(x, y + t / 2.0);
            pb.cubic_to(x, y + t / 2.0 - k, x + t / 2.0 - k, y + a, x + t / 2.0, y + a);
            // arc from mid-right to mid-bottom
            pb.move_to(x + t, y + t / 2.0);
            pb.cubic_to(x + t, y + t / 2.0 + k, x + t / 2.0 + k, y + b, x + t / 2.0, y + b);
        }
    }
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    Canvas::from_pixmap(&pm)
}

/// Overlapping ring fields, left in continuous tone so the dither does the work.
fn moire(height: usize, rng: &mut Rng) -> Canvas {
    let mut canvas = Canvas::new(DOTS, height);
    let sources: Vec<(f32, f32, f32)> = (0..rng.below(3) + 2)
        .map(|_| {
            (
                rng.range(0.0, DOTS as f32),
                rng.range(0.0, height as f32),
                rng.range(0.05, 0.22),
            )
        })
        .collect();
    for y in 0..height {
        for x in 0..DOTS {
            let mut v = 0.0;
            for (sx, sy, freq) in &sources {
                let d = ((x as f32 - sx).powi(2) + (y as f32 - sy).powi(2)).sqrt();
                v += (d * freq).sin();
            }
            canvas.set(x, y, (v / sources.len() as f32) * 0.5 + 0.5);
        }
    }
    canvas
}

/// Hilbert curve, one continuous line folded to fill the paper.
fn hilbert(height: usize) -> Canvas {
    let order = 6u32;
    let n = 1usize << order;
    let side = DOTS.min(height);
    let step = side as f32 / n as f32;
    let ox = (DOTS - side) as f32 / 2.0;
    let oy = (height.saturating_sub(side)) as f32 / 2.0;

    let mut pm = pixmap(height);
    let paint = black();
    let stroke = Stroke { width: 2.0, ..Default::default() };
    let mut pb = PathBuilder::new();

    for i in 0..n * n {
        let (hx, hy) = hilbert_point(i, order);
        let (px, py) = (ox + (hx as f32 + 0.5) * step, oy + (hy as f32 + 0.5) * step);
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    Canvas::from_pixmap(&pm)
}

fn hilbert_point(index: usize, order: u32) -> (usize, usize) {
    let mut rx;
    let mut ry;
    let mut t = index;
    let (mut x, mut y) = (0usize, 0usize);
    let mut s = 1usize;
    while s < (1 << order) {
        rx = 1 & (t / 2);
        ry = 1 & (t ^ rx);
        // rotate the quadrant into place
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - x;
                y = s - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        x += s * rx;
        y += s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

/// Dots pushed apart by their neighbours, then joined to the nearest few.
fn stipple(height: usize, rng: &mut Rng) -> Canvas {
    let count = 420;
    let mut pts: Vec<(f32, f32)> = (0..count)
        .map(|_| (rng.range(0.0, DOTS as f32), rng.range(0.0, height as f32)))
        .collect();
    // a few rounds of mutual repulsion spreads them out evenly
    for _ in 0..12 {
        let snapshot = pts.clone();
        for (i, p) in pts.iter_mut().enumerate() {
            let (mut dx, mut dy) = (0.0f32, 0.0f32);
            for (j, q) in snapshot.iter().enumerate() {
                if i == j {
                    continue;
                }
                let (vx, vy) = (p.0 - q.0, p.1 - q.1);
                let d2 = vx * vx + vy * vy;
                if d2 > 0.1 && d2 < 6000.0 {
                    dx += vx / d2 * 120.0;
                    dy += vy / d2 * 120.0;
                }
            }
            p.0 = (p.0 + dx).clamp(0.0, DOTS as f32 - 1.0);
            p.1 = (p.1 + dy).clamp(0.0, height as f32 - 1.0);
        }
    }

    let mut pm = pixmap(height);
    let paint = black();
    let stroke = Stroke { width: 1.0, ..Default::default() };
    let mut pb = PathBuilder::new();
    for (i, p) in pts.iter().enumerate() {
        let mut near: Vec<(f32, usize)> = pts
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(j, q)| (((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)), j))
            .collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (_, j) in near.iter().take(3) {
            if *j > i {
                pb.move_to(p.0, p.1);
                pb.line_to(pts[*j].0, pts[*j].1);
            }
        }
    }
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    Canvas::from_pixmap(&pm)
}

/// Stacked ridgelines, each hiding what is behind it — the Joy Division plot.
fn waves(height: usize, rng: &mut Rng) -> Canvas {
    let lines = 44usize;
    let gap = height as f32 / (lines + 2) as f32;
    let mut pm = pixmap(height);
    let mut fill = black();
    fill.set_color(tiny_skia::Color::WHITE);
    let paint = black();
    let stroke = Stroke { width: 1.6, ..Default::default() };

    // a handful of bumps scattered across the sheet; each line samples them,
    // so peaks build and fade as you travel down the paper
    let bumps: Vec<(f32, f32, f32, f32)> = (0..14)
        .map(|_| {
            (
                rng.range(0.1, 0.9),           // x, as a fraction of the width
                rng.range(0.0, 1.0),           // y, as a fraction of the stack
                rng.range(0.03, 0.14),         // x spread
                rng.range(0.06, 0.3),          // y spread
            )
        })
        .collect();

    for i in 0..lines {
        let base = gap * (i as f32 + 1.5);
        let ly = i as f32 / lines as f32;
        let mut pb = PathBuilder::new();
        pb.move_to(0.0, base);
        for x in 0..=DOTS {
            let fx = x as f32 / DOTS as f32;
            let mut v = 0.0;
            for (bx, by, sx, sy) in &bumps {
                let dx = (fx - bx) / sx;
                let dy = (ly - by) / sy;
                v += (-(dx * dx + dy * dy)).exp();
            }
            // fine wobble so the flat stretches are not dead straight
            v += ((fx * 40.0 + i as f32 * 0.7).sin() * 0.5 + (fx * 97.0).sin() * 0.25) * 0.06;
            pb.line_to(x as f32, base - v * gap * 3.4);
        }
        pb.line_to(DOTS as f32, base + gap * 5.0);
        pb.line_to(0.0, base + gap * 5.0);
        pb.close();
        if let Some(path) = pb.finish() {
            // fill white first so each ridge occludes the ones behind it
            pm.fill_path(&path, &fill, tiny_skia::FillRule::Winding, Transform::identity(), None);
            pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    Canvas::from_pixmap(&pm)
}

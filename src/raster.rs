//! Grayscale composition, dithering, and packing into ESC/POS raster bits.
//!
//! The head is 576 dots across 80mm paper at 203 dpi. Compose at exactly that
//! width so nothing is ever scaled and softened, dither deliberately, then ship
//! the bitmap — ESC/POS has no concept of layout, only dots.

pub const DOTS: usize = 576;

#[derive(Clone, Copy, PartialEq)]
pub enum Dither {
    /// Hard cut at 50%: right for line work that is already 1-bit.
    Threshold,
    /// Keeps line work crisp; the usual choice for thermal paper.
    Atkinson,
    /// Truer mid-tones, slightly mushier edges.
    FloydSteinberg,
    /// Ordered 8x8, which prints as a visible weave.
    Bayer,
}

impl Dither {
    pub fn parse(s: &str) -> Option<Dither> {
        match s {
            "threshold" | "none" => Some(Dither::Threshold),
            "atkinson" => Some(Dither::Atkinson),
            "floyd" | "floyd-steinberg" => Some(Dither::FloydSteinberg),
            "bayer" | "ordered" => Some(Dither::Bayer),
            _ => None,
        }
    }
}

const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// A grayscale image: 0.0 is white paper, 1.0 is a fully burnt dot.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub ink: Vec<f32>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Canvas { width, height, ink: vec![0.0; width * height] }
    }

    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.ink[y * self.width + x]
    }

    pub fn set(&mut self, x: usize, y: usize, v: f32) {
        if x < self.width && y < self.height {
            self.ink[y * self.width + x] = v.clamp(0.0, 1.0);
        }
    }

    pub fn add(&mut self, x: usize, y: usize, v: f32) {
        if x < self.width && y < self.height {
            let i = y * self.width + x;
            self.ink[i] = (self.ink[i] + v).clamp(0.0, 1.0);
        }
    }

    /// Take the ink channel from a tiny-skia pixmap (black drawing on white).
    pub fn from_pixmap(pixmap: &tiny_skia::Pixmap) -> Self {
        let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
        let mut canvas = Canvas::new(w, h);
        for (i, px) in pixmap.pixels().iter().enumerate() {
            // demultiply and take luminance, then invert: dark pixels are ink
            let d = px.demultiply();
            let lum = 0.299 * d.red() as f32 + 0.587 * d.green() as f32 + 0.114 * d.blue() as f32;
            let alpha = d.alpha() as f32 / 255.0;
            canvas.ink[i] = (1.0 - lum / 255.0) * alpha;
        }
        canvas
    }

    /// Load an image file, converting to grayscale ink and fitting the paper width.
    pub fn from_file(path: &str, width: usize) -> Result<Self, String> {
        let img = image::open(path).map_err(|e| format!("{path}: {e}"))?;
        let img = img.resize(
            width as u32,
            u32::MAX,
            image::imageops::FilterType::Lanczos3,
        );
        let gray = img.to_luma8();
        let (w, h) = (gray.width() as usize, gray.height() as usize);
        let mut canvas = Canvas::new(w, h);
        for (i, p) in gray.pixels().enumerate() {
            canvas.ink[i] = 1.0 - p.0[0] as f32 / 255.0;
        }
        Ok(canvas)
    }

    /// Stretch contrast so the darkest ink hits full and the lightest hits zero.
    pub fn normalise(&mut self) {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &v in &self.ink {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        if hi - lo < 1e-6 {
            return;
        }
        for v in &mut self.ink {
            *v = (*v - lo) / (hi - lo);
        }
    }

    /// Apply gamma: >1 lightens, <1 darkens. Thermal paper tends to over-burn,
    /// so photographs usually want a touch of lightening.
    pub fn gamma(&mut self, g: f32) {
        for v in &mut self.ink {
            *v = v.powf(g);
        }
    }

    /// Reduce to one bit per dot, packed MSB-first into rows of bytes.
    pub fn dither(&self, method: Dither) -> Bits {
        let (w, h) = (self.width, self.height);
        let row_bytes = w.div_ceil(8);
        let mut bits = vec![0u8; row_bytes * h];
        let mut buf = self.ink.clone();

        for y in 0..h {
            for x in 0..w {
                let old = buf[y * w + x];
                let on = match method {
                    Dither::Bayer => {
                        old * 64.0 > BAYER8[y % 8][x % 8] as f32 + 0.5
                    }
                    _ => old >= 0.5,
                };
                if on {
                    bits[y * row_bytes + x / 8] |= 0x80 >> (x % 8);
                }
                let err = old - if on { 1.0 } else { 0.0 };
                let mut spread = |dx: isize, dy: isize, factor: f32| {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        buf[ny as usize * w + nx as usize] += err * factor;
                    }
                };
                match method {
                    // Atkinson spreads only 3/4 of the error, which is why it keeps
                    // edges crisp instead of smearing them into the surrounding tone.
                    Dither::Atkinson => {
                        for (dx, dy) in [(1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)] {
                            spread(dx, dy, 1.0 / 8.0);
                        }
                    }
                    Dither::FloydSteinberg => {
                        for (dx, dy, f) in [
                            (1, 0, 7.0 / 16.0),
                            (-1, 1, 3.0 / 16.0),
                            (0, 1, 5.0 / 16.0),
                            (1, 1, 1.0 / 16.0),
                        ] {
                            spread(dx, dy, f);
                        }
                    }
                    _ => {}
                }
            }
        }
        Bits { width: w, height: h, row_bytes, data: bits }
    }
}

/// A packed 1-bit image, ready for GS v 0.
pub struct Bits {
    pub width: usize,
    pub height: usize,
    pub row_bytes: usize,
    pub data: Vec<u8>,
}

impl Bits {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.data[y * self.row_bytes + x / 8] & (0x80 >> (x % 8)) != 0
    }

    /// Save what would be printed, for checking a piece without spending paper.
    pub fn to_png(&self, path: &str) -> Result<(), String> {
        let mut img = image::GrayImage::new(self.width as u32, self.height as u32);
        for y in 0..self.height {
            for x in 0..self.width {
                img.put_pixel(x as u32, y as u32,
                              image::Luma([if self.get(x, y) { 0 } else { 255 }]));
            }
        }
        img.save(path).map_err(|e| format!("{path}: {e}"))
    }
}

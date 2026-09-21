//! Minimal ESC/POS driver for the Epson TM-T20II over USB (80mm paper, 42 columns).

use crate::raster::Bits;
use rusb::{Direction, GlobalContext, TransferType};
use std::fmt;
use std::time::Duration;

pub const VID: u16 = 0x04b8;
pub const PID: u16 = 0x0e15;
/// Characters per line in font A on 80mm paper (576 dots / 12-dot glyphs).
pub const WIDTH: usize = 48;
/// Characters per line once double-width is on.
pub const WIDTH_BIG: usize = WIDTH / 2;

const ESC: u8 = 0x1b;
const GS: u8 = 0x1d;
const DLE: u8 = 0x10;
const EOT: u8 = 0x04;

#[derive(Debug)]
pub enum Error {
    NotFound,
    Usb(rusb::Error),
    Stalled(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => write!(f, "TM-T20II not found on USB — is it powered on and plugged in?"),
            Error::Usb(e) => write!(f, "USB error: {e}"),
            Error::Stalled(problems) if problems.is_empty() => write!(
                f,
                "printer would not accept data: it stopped responding \
                 (check the cover is latched and paper is loaded)"
            ),
            Error::Stalled(problems) => {
                write!(f, "printer would not accept data: {}", problems.join(", "))
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<rusb::Error> for Error {
    fn from(e: rusb::Error) -> Self {
        Error::Usb(e)
    }
}

/// Font A is 12x24 dots (48 columns), font B is 9x17 (64 columns).
#[derive(Clone, Copy, PartialEq)]
pub enum Font {
    A,
    B,
}

impl Font {
    /// Height in dots, which is also the line spacing that makes rows butt
    /// together — what dense ASCII art needs so it does not come apart.
    pub fn height(self) -> u8 {
        match self {
            Font::A => 24,
            Font::B => 17,
        }
    }

    pub fn columns(self) -> usize {
        match self {
            Font::A => 48,
            Font::B => 64,
        }
    }
}

pub enum Align {
    Left,
    Centre,
    Right,
}

#[derive(Default, Clone, Copy)]
pub struct Style {
    pub bold: bool,
    pub tall: bool,
    pub wide: bool,
    pub underline: bool,
}

impl Style {
    pub fn big() -> Self {
        Style { bold: true, tall: true, wide: true, underline: false }
    }
    fn bits(&self) -> u8 {
        let mut n = 0;
        if self.bold {
            n |= 0x08;
        }
        if self.tall {
            n |= 0x10;
        }
        if self.wide {
            n |= 0x20;
        }
        if self.underline {
            n |= 0x80;
        }
        n
    }
}

pub struct Printer {
    handle: rusb::DeviceHandle<GlobalContext>,
    ep_out: u8,
    ep_in: Option<u8>,
    in_max_packet: usize,
    buf: Vec<u8>,
}

impl Printer {
    pub fn open() -> Result<Self, Error> {
        let handle = rusb::open_device_with_vid_pid(VID, PID).ok_or(Error::NotFound)?;
        let config = handle.device().active_config_descriptor()?;

        let mut found = None;
        for interface in config.interfaces() {
            for desc in interface.descriptors() {
                let mut out = None;
                let mut inp = None;
                for ep in desc.endpoint_descriptors() {
                    if ep.transfer_type() != TransferType::Bulk {
                        continue;
                    }
                    match ep.direction() {
                        Direction::Out if out.is_none() => out = Some(ep.address()),
                        Direction::In if inp.is_none() => {
                            inp = Some((ep.address(), ep.max_packet_size() as usize))
                        }
                        _ => {}
                    }
                }
                if let Some(out) = out {
                    found = Some((interface.number(), out, inp));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        let (iface, ep_out, inp) = found.ok_or(Error::NotFound)?;
        // macOS does not support detaching its printer-class driver; ignore the failure
        // and let the claim tell us whether we really have the device.
        let _ = handle.set_auto_detach_kernel_driver(true);
        handle.claim_interface(iface)?;

        Ok(Printer {
            handle,
            ep_out,
            ep_in: inp.map(|(a, _)| a),
            in_max_packet: inp.map(|(_, m)| m).unwrap_or(64),
            buf: Vec::new(),
        })
    }

    // --- status ------------------------------------------------------------

    /// Send a real-time status request and return the reply byte, if the printer answers.
    fn query(&self, n: u8) -> Option<u8> {
        let ep_in = self.ep_in?;
        let timeout = Duration::from_millis(500);
        self.handle.write_bulk(self.ep_out, &[DLE, EOT, n], timeout).ok()?;
        let mut reply = vec![0u8; self.in_max_packet];
        for _ in 0..3 {
            if let Ok(len) = self.handle.read_bulk(ep_in, &mut reply, timeout) {
                if len > 0 {
                    return Some(reply[len - 1]);
                }
            }
        }
        None
    }

    /// Ask the printer how it is. An empty list means it is ready.
    pub fn status(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(s) = self.query(1) {
            if s & 0x08 != 0 {
                problems.push("printer is offline".into());
            }
        }
        if let Some(p) = self.query(4) {
            if p & 0x60 != 0 {
                problems.push("out of paper".into());
            } else if p & 0x0c != 0 {
                problems.push("paper nearly out".into());
            }
        }
        if let Some(o) = self.query(2) {
            if o & 0x04 != 0 {
                problems.push("cover is open".into());
            }
            if o & 0x40 != 0 {
                problems.push("printer is in an error state".into());
            }
        }
        problems
    }

    // --- composition -------------------------------------------------------

    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self
    }

    pub fn init(&mut self) -> &mut Self {
        self.raw(&[ESC, b'@'])
    }

    pub fn align(&mut self, how: Align) -> &mut Self {
        let n = match how {
            Align::Left => 0,
            Align::Centre => 1,
            Align::Right => 2,
        };
        self.raw(&[ESC, b'a', n])
    }

    pub fn font(&mut self, font: Font) -> &mut Self {
        self.raw(&[ESC, b'M', if font == Font::B { 1 } else { 0 }])
    }

    /// Set line spacing in dots (1/180"). Passing the font height closes the gap
    /// between rows, so multi-line art joins up instead of being striped.
    pub fn line_spacing(&mut self, dots: u8) -> &mut Self {
        self.raw(&[ESC, b'3', dots])
    }

    /// Back to the printer's default 1/6" spacing.
    pub fn default_spacing(&mut self) -> &mut Self {
        self.raw(&[ESC, b'2'])
    }

    /// White on black.
    pub fn invert(&mut self, on: bool) -> &mut Self {
        self.raw(&[GS, b'B', on as u8])
    }

    pub fn style(&mut self, style: Style) -> &mut Self {
        self.raw(&[ESC, b'!', style.bits()])
    }

    /// Print text, word-wrapping each line to the paper width.
    pub fn text(&mut self, s: &str) -> &mut Self {
        for line in s.split('\n') {
            for wrapped in wrap(line, WIDTH) {
                self.buf.extend(encode(&wrapped));
                self.buf.push(b'\n');
            }
        }
        self
    }

    /// Print text exactly as given, with no wrapping (for tables and ASCII art).
    pub fn text_raw(&mut self, s: &str) -> &mut Self {
        for line in s.split('\n') {
            self.buf.extend(encode(line));
            self.buf.push(b'\n');
        }
        self
    }

    pub fn rule(&mut self, ch: char) -> &mut Self {
        self.text_raw(&ch.to_string().repeat(WIDTH))
    }

    /// Word-wrap to an explicit width, for text printed at a larger size.
    pub fn text_wrapped(&mut self, s: &str, width: usize) -> &mut Self {
        for line in s.split('\n') {
            for wrapped in wrap(line, width) {
                self.buf.extend(encode(&wrapped));
                self.buf.push(b'\n');
            }
        }
        self
    }

    /// Left text and right text on one line, padded apart.
    #[allow(dead_code)]
    pub fn columns(&mut self, left: &str, right: &str) -> &mut Self {
        let space = WIDTH.saturating_sub(right.chars().count());
        let left: String = left.chars().take(space.saturating_sub(1)).collect();
        let pad = space.saturating_sub(left.chars().count());
        self.text_raw(&format!("{left}{}{right}", " ".repeat(pad)))
    }

    pub fn qr(&mut self, data: &str, size: u8) -> &mut Self {
        let d = data.as_bytes();
        let n = d.len() + 3;
        self.raw(&[GS, b'(', b'k', 4, 0, 49, 65, 50, 0]); // model 2
        self.raw(&[GS, b'(', b'k', 3, 0, 49, 67, size]); // module size
        self.raw(&[GS, b'(', b'k', 3, 0, 49, 69, 48]); // error correction L
        self.raw(&[GS, b'(', b'k', (n & 0xff) as u8, (n >> 8) as u8, 49, 80, 48]);
        self.raw(d); // store
        self.raw(&[GS, b'(', b'k', 3, 0, 49, 81, 48]) // print
    }

    /// Send a 1-bit image with GS v 0, in horizontal bands. Banding keeps each
    /// command inside the printer's image buffer instead of overrunning it.
    pub fn image(&mut self, bits: &Bits) -> &mut Self {
        const BAND: usize = 128;
        let xl = (bits.row_bytes & 0xff) as u8;
        let xh = (bits.row_bytes >> 8) as u8;
        for start in (0..bits.height).step_by(BAND) {
            let rows = BAND.min(bits.height - start);
            self.raw(&[GS, b'v', b'0', 0, xl, xh, (rows & 0xff) as u8, (rows >> 8) as u8]);
            let from = start * bits.row_bytes;
            let to = from + rows * bits.row_bytes;
            let slice = bits.data[from..to].to_vec();
            self.raw(&slice);
        }
        self
    }

    pub fn feed(&mut self, lines: u8) -> &mut Self {
        self.raw(&[ESC, b'd', lines])
    }

    pub fn cut(&mut self) -> &mut Self {
        self.raw(&[GS, b'V', 0x41, 4])
    }

    // --- output ------------------------------------------------------------

    pub fn send(&mut self) -> Result<usize, Error> {
        if self.buf.is_empty() {
            return Ok(0);
        }
        match self.handle.write_bulk(self.ep_out, &self.buf, Duration::from_secs(5)) {
            Ok(written) => {
                self.buf.clear();
                Ok(written)
            }
            Err(rusb::Error::Timeout) => Err(Error::Stalled(self.status())),
            Err(e) => Err(Error::Usb(e)),
        }
    }
}

/// Map to code page 437, which is what the printer starts up in. Box-drawing and
/// shade characters are worth carrying across; anything else becomes '?'.
fn encode(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            c if c.is_ascii() => c as u8,
            '\u{2500}' => 0xc4, // ─
            '\u{2550}' => 0xcd, // ═
            '\u{2588}' => 0xdb, // █
            '\u{2584}' => 0xdc, // ▄
            '\u{2580}' => 0xdf, // ▀
            '\u{2591}' => 0xb0, // ░
            '\u{2592}' => 0xb1, // ▒
            '\u{2593}' => 0xb2, // ▓
            _ => b'?',
        })
        .collect()
}

/// Word-wrap a single line, breaking over-long words rather than overflowing.
fn wrap(line: &str, width: usize) -> Vec<String> {
    if line.chars().count() <= width {
        return vec![line.to_string()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for word in line.split(' ') {
        let mut word = word;
        while word.chars().count() > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            // Cut on a character boundary. Slicing at `width` bytes panics the
            // moment an over-long word holds anything outside ASCII, which is
            // an easy thing for a stranger to send the HTTP endpoint.
            let cut = word.char_indices().nth(width).map_or(word.len(), |(i, _)| i);
            out.push(word[..cut].to_string());
            word = &word[cut..];
        }
        let extra = if current.is_empty() { 0 } else { 1 };
        if current.chars().count() + extra + word.chars().count() > width {
            out.push(std::mem::take(&mut current));
        } else if extra == 1 {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

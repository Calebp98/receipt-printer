use receipt::{art, generative, printer, raster};

use clap::Parser;
use generative::Rng;
use printer::{Align, Font, Printer, Style, WIDTH_BIG};
use raster::{Canvas, Dither, DOTS};
use std::io::{IsTerminal, Read};
use std::process::exit;

/// The horizontal rule character on a task slip: a solid cp437 line.
const RULE: char = '\u{2550}';

/// Height of the banner on a task slip, in dots (about 3cm at 203 dpi).
const BANNER_DOTS: usize = 240;

/// Print text on the Epson TM-T20II receipt printer.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Text to print (default: read stdin)
    text: Vec<String>,

    /// Big centred heading above the text
    #[arg(short, long)]
    title: Option<String>,

    /// Read the text from a file
    #[arg(short, long)]
    file: Option<String>,

    /// Append a QR code with this content
    #[arg(short, long)]
    qr: Option<String>,

    /// Print lines exactly as given, without word-wrapping
    #[arg(long)]
    raw: bool,

    /// Print at double size
    #[arg(short, long)]
    big: bool,

    /// Font: a (12x24, 48 cols) or b (9x17, 64 cols)
    #[arg(long, default_value = "a", value_parser = ["a", "b"])]
    font: String,

    /// Close up the line spacing so rows of art join together
    #[arg(long)]
    tight: bool,

    /// White on black
    #[arg(long)]
    invert: bool,

    /// Leave the paper uncut
    #[arg(long)]
    no_cut: bool,

    /// Format as a task slip: rules, a random picture, then the text large
    #[arg(long)]
    task: bool,

    /// Pick the task slip picture by name instead of at random
    #[arg(long, value_name = "NAME")]
    art: Option<String>,

    /// Print a sheet of sample pictures with their names, then exit
    #[arg(long)]
    art_sheet: bool,

    /// List every picture name on stdout, then exit
    #[arg(long)]
    art_list: bool,

    /// Print a generative piece by name (see --gen-list)
    #[arg(long = "gen", value_name = "NAME")]
    generator: Option<String>,

    /// List the generative pieces, then exit
    #[arg(long)]
    gen_list: bool,

    /// Print an image file, dithered to fit the paper
    #[arg(long, value_name = "FILE")]
    image: Option<String>,

    /// Dithering: atkinson, floyd, bayer, threshold
    #[arg(long, default_value = "atkinson")]
    dither: String,

    /// Height in dots for a generative piece (203 dots per inch)
    #[arg(long, default_value_t = 800)]
    height: usize,

    /// Seed a generative piece so it can be reproduced
    #[arg(long)]
    seed: Option<u64>,

    /// Gamma before dithering: above 1 lightens, below 1 darkens
    #[arg(long, default_value_t = 1.0)]
    gamma: f32,

    /// Write a PNG of what would be printed instead of printing it
    #[arg(long, value_name = "FILE")]
    preview: Option<String>,

    /// Report printer status and exit
    #[arg(long)]
    status: bool,
}

fn main() {
    let args = Args::parse();

    if args.gen_list {
        for (name, description) in generative::GENERATORS {
            println!("{name:<10} {description}");
        }
        return;
    }

    if args.generator.is_some() || args.image.is_some() {
        graphics(&args);
        return;
    }

    let mut p = match Printer::open() {
        Ok(p) => p,
        Err(e) => fail(e),
    };

    if args.status {
        let problems = p.status();
        println!("{}", if problems.is_empty() { "ready".into() } else { problems.join("; ") });
        return;
    }

    if args.art_list {
        for piece in art::ART.iter() {
            println!("{:<28} {:>2}x{:<2}  {} (by {})",
                     piece.name, piece.width, piece.height, piece.title, piece.artist);
        }
        return;
    }

    if args.art_sheet {
        p.init().line_spacing(Font::A.height());
        for piece in art::ART.iter().filter(|a| a.width <= WIDTH_BIG).take(12) {
            p.style(Style { bold: true, ..Style::default() })
                .text(&format!("{}  [{}]", piece.title, piece.name));
            p.style(Style::default()).text_raw(piece.art).feed(1);
        }
        p.default_spacing().feed(2).cut();
        if let Err(e) = p.send() {
            fail(e);
        }
        return;
    }

    let body = match resolve_text(&args) {
        Ok(body) => body,
        Err(msg) => fail(msg),
    };

    if args.task {
        print_task(&mut p, body.trim(), &args);
        if let Err(e) = p.send() {
            fail(e);
        }
        return;
    }

    p.init();
    if let Some(title) = &args.title {
        p.align(Align::Centre).style(Style::big()).text(title);
        p.style(Style::default()).text("").align(Align::Left);
    }
    let font = if args.font == "b" { Font::B } else { Font::A };
    p.font(font);
    if args.tight {
        let height = font.height() * if args.big { 2 } else { 1 };
        p.line_spacing(height);
    }
    if args.invert {
        p.invert(true);
    }
    if args.big {
        p.style(Style { tall: true, wide: true, ..Style::default() });
    }
    let body = body.trim_end_matches('\n');
    if args.raw {
        p.text_raw(body);
    } else {
        let width = font.columns() / if args.big { 2 } else { 1 };
        p.text_wrapped(body, width);
    }
    p.style(Style::default());
    p.invert(false).default_spacing();
    if let Some(qr) = &args.qr {
        p.feed(1).align(Align::Centre).qr(qr, 6).align(Align::Left);
    }
    p.feed(2);
    if !args.no_cut {
        p.cut();
    }

    if let Err(e) = p.send() {
        fail(e);
    }
}

/// A task slip: two rules, a banner, the task in large type, two more rules.
/// The banner is a generated piece by default — 576 dots of real detail rather
/// than a few characters — with the ASCII collection still there via --art.
fn print_task(p: &mut Printer, text: &str, args: &Args) {
    p.init();
    p.align(Align::Left);
    p.rule(RULE).rule(RULE);
    p.feed(1);

    if let Some(name) = &args.art {
        let picture = match art::by_name(name) {
            Some(a) => a,
            None => fail(format!("no picture called '{name}' — try --art-list")),
        };
        // Double-size art needs double-height line spacing or the rows pull apart.
        p.style(Style { tall: true, wide: true, ..Style::default() });
        p.line_spacing(Font::A.height() * 2);
        p.text_raw(&art::centred(picture.art, WIDTH_BIG));
        p.default_spacing().style(Style::default());
    } else {
        let name = args.generator.clone().unwrap_or_else(|| {
            let mut pick = Rng::new(args.seed);
            // hilbert wants a square, which a banner is not
            let usable: Vec<&str> = generative::GENERATORS
                .iter()
                .map(|(n, _)| *n)
                .filter(|n| *n != "hilbert")
                .collect();
            usable[pick.below(usable.len())].to_string()
        });
        let mut rng = Rng::new(args.seed);
        match generative::generate(&name, BANNER_DOTS, &mut rng) {
            Some(canvas) => {
                p.image(&canvas.dither(Dither::Atkinson));
            }
            None => fail(format!("no generator called '{name}' — try --gen-list")),
        }
    }
    p.feed(1);

    p.align(Align::Centre).style(Style::big());
    p.text_wrapped(text, WIDTH_BIG - 1);
    p.style(Style::default()).align(Align::Left);

    p.feed(1);
    p.rule(RULE).rule(RULE);
    p.feed(3).cut();
}

/// Compose a picture — generated or loaded — dither it, then print or preview it.
fn graphics(args: &Args) {
    let dither = match Dither::parse(&args.dither) {
        Some(d) => d,
        None => fail("dither must be atkinson, floyd, bayer or threshold"),
    };

    let mut canvas: Canvas = if let Some(name) = &args.generator {
        let mut rng = Rng::new(args.seed);
        match generative::generate(name, args.height, &mut rng) {
            Some(c) => c,
            None => fail(format!("no generator called '{name}' — try --gen-list")),
        }
    } else {
        let path = args.image.as_ref().unwrap();
        match Canvas::from_file(path, DOTS) {
            Ok(c) => c,
            Err(e) => fail(e),
        }
    };

    if args.image.is_some() {
        canvas.normalise();
    }
    if (args.gamma - 1.0).abs() > f32::EPSILON {
        canvas.gamma(args.gamma);
    }
    let bits = canvas.dither(dither);

    if let Some(path) = &args.preview {
        if let Err(e) = bits.to_png(path) {
            fail(e);
        }
        println!("{path}: {}x{} dots", bits.width, bits.height);
        return;
    }

    let mut p = match Printer::open() {
        Ok(p) => p,
        Err(e) => fail(e),
    };
    p.init().image(&bits).feed(2);
    if !args.no_cut {
        p.cut();
    }
    if let Err(e) = p.send() {
        fail(e);
    }
}

fn resolve_text(args: &Args) -> Result<String, String> {
    if let Some(path) = &args.file {
        return std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    }
    if !args.text.is_empty() {
        return Ok(args.text.join(" "));
    }
    if !std::io::stdin().is_terminal() {
        let mut body = String::new();
        std::io::stdin().read_to_string(&mut body).map_err(|e| e.to_string())?;
        return Ok(body);
    }
    Err("nothing to print — pass text, use --file, or pipe stdin".into())
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("receipt: {msg}");
    exit(1);
}

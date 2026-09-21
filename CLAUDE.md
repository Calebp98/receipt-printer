# Receipt printer

Drives an **Epson TM-T20II** thermal receipt printer over USB and prints text,
ASCII art, generated pictures and images on it.

## The hardware

- USB `04b8:0e15`, ESC/POS command set, 80mm paper.
- Print head is **576 dots wide at 203 dpi** (72mm of the 80mm roll).
- **48 columns** in font A (12x24 dots), 64 in font B (9x17). This was measured
  on paper, not assumed — do not "correct" it to the 42 that many TM printers use.
- Double-size text halves the column count to 24.

## Talking to it

macOS no longer supports raw CUPS queues (`lpadmin -m raw` fails outright), so
the tool speaks to the device directly through libusb. There is no print queue
and no driver involved.

Build and run:

```sh
cargo build --release
./target/release/receipt --status        # ready / cover is open / out of paper
echo hi | receipt                        # /opt/homebrew/bin/receipt is a symlink
                                         # to target/release/receipt
```

The symlink means `cargo clean` breaks the `receipt` command until the next build.

## Layout

`src/lib.rs` is the crate; both binaries are shells around it.

- `src/printer.rs` — the driver: USB discovery, interface claim, real-time status
  queries, text wrapping, styles, fonts, line spacing, QR codes, raster images, cut.
- `src/raster.rs` — grayscale canvas, dithering (Atkinson, Floyd-Steinberg,
  Bayer, threshold), packing into `GS v 0` bits, PNG preview.
- `src/generative.rs` — generated pieces composed at exactly 576 dots wide.
- `src/art.rs` + `src/art_data.rs` — the ASCII collection. `art_data.rs` is
  **generated**: `python3 tools/gen_art.py` rebuilds it from `art/art.json`,
  which `tools/fetch_art.py` scrapes from asciiart.eu. Artist credit is kept with
  each piece; don't strip it.
- `src/main.rs` — the CLI.
- `src/bin/server.rs` + `web/index.html` — the HTTP endpoint behind
  `receipt.calebparikh.xyz`, and the page it serves. Behind the `server`
  feature, so a plain `cargo build` does not drag in tokio.
- `deploy/` — the systemd unit and the cloudflared config, as installed.
- `docs/pi-setup.md`, `docs/network.md` — running it off a Pi, and the public
  endpoint.
- `escpos.py`, `receipt.py`, `nyan.py` — the original Python version, kept only
  as a reference. Everything it did is in the Rust tool now.

## Things learnt the hard way

- **Check paper orientation first.** Thermal paper only marks on the coated side.
  A roll in backwards feeds and cuts perfectly and prints nothing at all — it
  looks exactly like a data problem and isn't.
- **A write timeout usually means the printer is offline**, not that the code is
  wrong: cover open, out of paper, or mid-reload. `send()` turns that into a
  message naming the cause via a status query.
- **Line spacing must be set for art.** The default is 1/6" against a 24-dot
  font, so every row of multi-line art gets a white stripe under it. Set the
  spacing to the font height (doubled if the text is double-height).
- **Centre art as a block**, by padding every line equally. `ESC a 1` centres
  each line independently, which shears a drawing apart.
- Non-ASCII goes through a small cp437 map in `encode()` — box drawing and
  shades survive, everything else becomes `?`.

## The public endpoint

`receipt-server` runs on the Pi as a systemd service and is published through a
Cloudflare tunnel at `https://receipt.calebparikh.xyz`. A shared password, a
cooldown, per-client and per-day caps and a pause file are what stand between a
stranger and the whole roll — `docs/network.md` says why each one is there.

- **The page is compiled in** with `include_str!`. Editing `web/index.html`
  means a rebuild and a deploy, not a file copy.
- **`./deploy.sh` restarts the service**, because the running process holds the
  USB claim and the new binary cannot open the printer until it lets go.
- **Anything reachable from `/api/print` is reachable by a stranger.** The
  wrapper that hard-split long words used to slice by bytes and would panic on
  a multi-byte character; that class of bug is now a live one.

## Working on this

- **Use `--preview out.png` instead of paper** when checking a generated piece or
  an image. Paper is the slow loop and Claude cannot see it.
- **Claude cannot see the physical output.** Ask Caleb specific questions about
  what came out — is it too dark, did fine line work survive, did the cut land
  clear of the text — rather than assuming a print worked because the write
  succeeded.
- Compose images at exactly 576 px wide so nothing is scaled and softened, and
  dither deliberately; Atkinson keeps line work crisp on thermal paper.

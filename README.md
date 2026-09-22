# receipt

Drives an **Epson TM-T20II** thermal receipt printer over USB and prints text,
ASCII art, generated pictures and images on it — from a command line, and from
the internet.

There is one running at **[receipt.calebparikh.xyz](https://receipt.calebparikh.xyz)**:
a Pi Zero 2 W with the printer hanging off it, published through a Cloudflare
tunnel. What you type there comes out on paper in Cambridge.

## Why it talks to USB directly

macOS no longer supports raw CUPS queues — `lpadmin -m raw` fails outright — so
the tool speaks to the device through libusb. There is no print queue and no
driver involved, on either macOS or the Pi.

The head is 576 dots wide at 203 dpi, which is 48 columns in font A. That was
measured on paper, not assumed: plenty of TM printers are 42 and this one is not.

## The command line

```sh
cargo build --release
./target/release/receipt --status       # ready / cover is open / out of paper

echo "hello" | receipt
receipt --title "TODO" --file list.txt
receipt --task "take the bins out"      # a slip with a generated banner
receipt --gen waves --height 600
receipt --image photo.jpg --dither atkinson
receipt --gen flow --preview out.png    # to a PNG instead of paper
```

`--preview` is how you iterate without spending paper.

## The HTTP endpoint

`receipt-server` is a small axum service that holds the USB claim and serves its
own page, so there is no CORS and no second host to keep in sync.

```sh
cargo build --release --features server
RECEIPT_PASSWORD=... receipt-server --listen 127.0.0.1:8080
```

```
GET  /api/status   {ready, detail, paused, remaining_today, max_chars}
POST /api/print    {password, name, text}
GET  /healthz
```

It binds to localhost and is published by a Cloudflare tunnel, so nothing
listens on the public internet and no port is forwarded. Because the thing on
the other end is a finite roll of paper in someone's house, it is guarded by a
shared password, a length limit, an optional cooldown and per-client allowance,
a daily cap, and a pause file. `docs/network.md` says why each one is there.

## Layout

- `src/printer.rs` — the driver: USB discovery, interface claim, real-time
  status queries, text wrapping, styles, fonts, line spacing, QR codes, raster
  images, cut
- `src/raster.rs` — grayscale canvas, dithering (Atkinson, Floyd-Steinberg,
  Bayer, threshold), packing into `GS v 0` bits, PNG preview
- `src/generative.rs` — generated pieces composed at exactly 576 dots wide
- `src/art.rs`, `src/art_data.rs` — the ASCII collection, with artist credit
  kept against each piece
- `src/main.rs` — the CLI
- `src/bin/server.rs`, `web/index.html` — the endpoint and its page
- `docs/pi-setup.md` — running it headless off a Pi Zero 2 W
- `docs/network.md` — the tunnel, the service, and the guards

## Things that cost an evening to learn

- **Check the paper orientation first.** Thermal paper only marks on the coated
  side. A roll in backwards feeds and cuts perfectly and prints nothing at all.
  It looks exactly like a data problem and isn't.
- **A write timeout usually means the printer is offline**, not that the code is
  wrong: cover open, out of paper, or mid-reload.
- **Line spacing has to be set for art.** The default is 1/6" against a 24-dot
  font, so every row of multi-line art gets a white stripe under it.
- **Centre art as a block**, by padding every line equally. `ESC a 1` centres
  each line independently, which shears a drawing apart.
- **The near-end paper sensor trips with metres still on the roll.** Treat it as
  a warning; refusing to print there stops you days early.

## Licence

MIT. The ASCII art in `art/` is other people's work, scraped from asciiart.eu
with the artist credit kept against each piece — that part is theirs, not mine.

# Printing over the network

Status as of 2026-09-22: **built and running**. `receipt-server` runs on the Pi,
holds the USB claim, and serves both its own page and its API on
`127.0.0.1:8080`. A Cloudflare tunnel publishes it at
`https://receipt.calebparikh.xyz`. Nothing listens on the public internet and no
port is forwarded.

## The shape

```
browser ──https──▶ Cloudflare ──tunnel──▶ cloudflared (Pi) ──▶ 127.0.0.1:8080
                                                                    │
                                                          receipt-server
                                                                    │ libusb
                                                              TM-T20II
```

The server binds to localhost only, so the tunnel is the firewall: there is no
route to the port that does not come down it.

## Endpoints

- `GET /` — the page, compiled into the binary from `web/index.html`
- `GET /api/status` — `{ready, detail, paused, remaining_today, max_chars}`
- `POST /api/print` — `{password, name, text}`
- `POST /linear/webhook` — Linear Comment events; see below
- `GET /healthz` — for uptime checks

Refusals carry the status code that fits: 401 wrong password, 400 empty, 413
over-length, 429 cooldown or daily limit, 503 paused or the printer is unhappy.
The body is always `{ok, message}`, and the message is written to be shown to
whoever sent it.

## What stops a stranger burning the roll

Paper is finite and the printer is in someone's house, so:

- **A shared password**, compared in constant time. Handed out deliberately —
  this is not "anyone with the URL".
- **A cap for the whole day** (2000), which is a stop on a runaway loop rather
  than a rationing of the paper.
- **A cooldown and a per-client daily allowance**, both off. `--cooldown` and
  `--per-client-daily` turn them back on, keyed on `CF-Connecting-IP`, which
  only the tunnel can set because nothing else can reach the port. Every limit
  is off when it is zero, `--daily-cap` included.
- **A length limit** (600 characters) and control characters stripped.
- **One print at a time**, behind a mutex — and the status query is behind the
  same one. There is a single USB claim to go round, and a page polling status
  while a print starts makes the print fail with "Resource busy".
- **A pause file**. `sudo -u caleb touch /var/lib/receipt-server/paused` stops
  printing within a second and the page says so; delete it to resume.

Allowances are spent before printing and handed back if the printer refuses, so
a jam cannot be used to mint extra prints. Counts live in memory and reset at
local midnight — and on restart, which is a deliberate trade for not keeping a
database on an SD card.

A **warning is not a refusal**: the near-end paper sensor trips with metres
still on the roll, so `blockers()` leaves it out and only an open cover, a truly
empty roll or an error state stops a print. `/api/status` still reports it, and
the page shows it while staying ready.

## Linear

Comment `[print]` on a Linear issue and it comes out on paper: identifier,
priority, title at double size, state, assignee, estimate, due date, project,
labels, the first 400 characters of the description, and whatever else the
comment said.

Linear posts Comment events to `POST /linear/webhook`. Two secrets live in
`/etc/receipt-server.env` beside the password:

```
LINEAR_WEBHOOK_SECRET=lin_wh_...     # from the webhook in Linear settings
LINEAR_API_KEY=lin_api_...           # a personal API key, read scope
```

Both or neither — the service refuses to start with only one, because an
integration that verifies requests but cannot fetch anything is worse than one
that is plainly off. With neither, the route answers 503 and says so.

The API key needs only read access. It is also how the service knows who "me"
is: it resolves `viewer` at startup, and only that person's comments print.

### Why it is shaped this way

- **Linear escapes Markdown punctuation.** A comment typed as `[print]` arrives
  as `\[print\]`, so the body is unescaped before the marker is looked for —
  and before the note goes on paper, where the backslashes would print.
- **The signature is checked over the raw bytes, before parsing.** Re-serialising
  the JSON would not reproduce what was signed.
- **A bad signature gets 401; everything else gets 200.** Linear retries a
  non-200 at one minute, one hour and six hours, then disables the webhook. A
  printer with its cover open must not be able to switch the integration off, so
  no printing problem ever reaches the response.
- **The reply goes out before the printing starts.** Linear allows five seconds,
  and fetching the issue alone can outlast that.
- **Only the marker *arriving* counts.** Linear fires an update for any edit, so
  a comment that already said `[print]` would reprint every time a typo was
  fixed. `updatedFrom.body` is what tells the difference. An update with no
  previous body is treated as already-there: a fresh request is one new comment
  away, and a spurious reprint is paper.
- **Deliveries and comment ids are remembered** (the last 200), so a retry does
  not mean a second receipt.
- **A print that cannot happen is retried** every 30s for a few minutes, which
  covers a paper change. It does not survive a restart; comment again.
- **The pause file still wins.** Linear prints are exempt from the public daily
  cap, though — your own work should not queue behind strangers.

### Testing it without Linear

Sign a payload with the same secret and post it. Note that Cloudflare's bot
check rejects some HTTP clients on sight with a 403 and `error code: 1010` —
that is Cloudflare, not this service, and `curl` gets through where Python's
`urllib` does not.

## Running it

The unit is `deploy/receipt-server.service`, installed to
`/etc/systemd/system/`. It runs as `caleb` with the `plugdev` group, which is
what the udev rule grants the printer to. The password comes from
`/etc/receipt-server.env` (`RECEIPT_PASSWORD=...`, mode 0640 root:caleb) rather
than the command line, where `ps` would show it.

```sh
sudo systemctl status receipt-server
sudo journalctl -u receipt-server -f
sudo systemctl restart receipt-server     # after a deploy, to retake the USB claim
```

`./deploy.sh` restarts it for you.

To change the password: edit `/etc/receipt-server.env`, then restart.

## The tunnel

`cloudflared` runs as its own service from `/etc/cloudflared/config.yml` (see
`deploy/cloudflared-config.yml`). Setting it up again from scratch:

```sh
cloudflared tunnel login                  # opens a browser; authorize the zone
cloudflared tunnel create receipt
cloudflared tunnel route dns receipt receipt.calebparikh.xyz
sudo cloudflared service install
```

`route dns` writes the CNAME itself — there is no DNS record to add by hand.

## Worth doing next

- **A Cloudflare WAF rate-limit rule** in front, as a second layer that never
  reaches the Pi at all.
- **Print density** (`GS ( E` function 5) is still unwired. Worth trying if
  output is consistently too light or too dark.

## Not worth pursuing

- **Multi-tone printing** (`GS 8 L`, 16 grey levels). Needs a newer head than
  the TM-T20II. Dithering is the ceiling here.
- **ESP32 instead of the Pi.** Only the S2/S3 have USB OTG host, printer-class
  support is immature, and it means writing raw USB host code instead of using
  libusb.
- **Port forwarding.** The tunnel is strictly better: no public listener, no
  dynamic-IP problem, and Cloudflare in front.

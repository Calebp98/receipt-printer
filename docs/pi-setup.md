# Running the receipt printer from a Pi Zero 2 W

The Zero 2 W has no Ethernet and two micro-USB sockets that look identical.
Everything below assumes a headless setup: the Pi is never plugged into a monitor.

## The Pi that exists

Set up on 2026-09-21 and printing.

- `raspi-zero2w-receipt.local`, `192.168.0.47`, user `caleb`
- Pi OS trixie (Debian 13), kernel 6.18.50+rpt-rpi-v8, aarch64, 512MB
- SSH key auth from the Mac; `sudo` still asks for a password
- `dtoverlay=dwc2,dr_mode=host` is already in `/boot/firmware/config.txt`, so
  the middle micro-USB socket runs as a host port
- `/usr/local/bin/receipt` is the deployed binary

The rest of this document is how it got there, and how to do it again.

## What you need besides the Pi

- **microSD card**, 8GB or larger
- **micro-USB power supply**, 5V 2.5A, into the socket marked `PWR`
- **micro-USB OTG adapter** (micro-USB male to USB-A female). The printer's
  existing cable plugs into this. It goes in the socket marked `USB`, the middle
  one — the outer socket is power only and will look dead if you use it by mistake.

## 1. Flash the card

Use Raspberry Pi Imager and pick **Raspberry Pi OS Lite (64-bit)**. Lite has no
desktop, which matters on 512MB of RAM.

Before writing, open the settings (the gear icon) and fill in:

- hostname: `receipt` — the Pi is then reachable as `receipt.local`
- enable SSH, with your public key rather than a password
- username and WiFi network, with country `GB`

Doing this in Imager is what makes the Pi come up on the network by itself. Skip
it and you need a monitor and keyboard to finish setup.

## 2. First boot

```sh
ssh <you>@receipt.local
sudo apt update && sudo apt full-upgrade -y
sudo apt install -y libusb-1.0-0
```

The Zero 2 W takes a couple of minutes on its first boot. If `receipt.local`
doesn't resolve, find it by IP from your router instead.

## 3. Let a normal user talk to the printer

Linux binds its own `usblp` driver to anything claiming to be a printer, and a
plain user can't claim a USB device at all. Both are fixed with a udev rule:

```sh
sudo tee /etc/udev/rules.d/99-epson-receipt.rules <<'EOF'
SUBSYSTEM=="usb", ATTRS{idVendor}=="04b8", ATTRS{idProduct}=="0e15", MODE="0660", GROUP="plugdev", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG plugdev $USER
```

Log out and back in for the group change to take. The tool detaches `usblp`
itself when it claims the interface, so no blacklisting is needed — but if you
ever see "Resource busy", `echo 'blacklist usblp' | sudo tee /etc/modprobe.d/blacklist-usblp.conf`
and reboot is the bigger hammer.

Check the printer is seen:

```sh
lsusb | grep 04b8     # Bus 001 Device 00X: ID 04b8:0e15 Seiko Epson Corp.
```

## 4. Get the binary onto the Pi

512MB of RAM will not happily compile `image` and `tiny-skia`, so build on the
Mac and copy the result. The cross toolchain is **zig**: it supplies both the
aarch64 linker and the C compiler that builds the vendored libusb, so there is
no aarch64 library to hunt down and no Docker daemon in the loop.

```sh
brew install zig                                    # once
cargo install cargo-zigbuild                        # once
rustup target add aarch64-unknown-linux-gnu         # once
./deploy.sh raspi-zero2w-receipt.local caleb        # build and copy
```

`cargo install cargo-zigbuild` needs rustc 1.88 or newer. On an older toolchain
either `rustup update`, or pin the last compatible release:
`cargo install cargo-zigbuild --version 0.21.8 --locked`.

`deploy.sh` targets `aarch64-unknown-linux-gnu.2.36` — the `.2.36` pins the
glibc version the binary asks for well below the Pi's own (trixie ships 2.41),
so the build does not drift with whatever glibc zig defaults to.

`cross` is the other documented option and works in principle, but it needs a
healthy Docker daemon and several GB of free disk for its image; a full disk
corrupts containerd's metadata DB and the failure is opaque. zig has neither
requirement.

If you would rather build on the Pi itself, it is possible with 1GB of swap
(`sudo apt install dphys-swapfile`, set `CONF_SWAPSIZE=1024` in
`/etc/dphys-swapfile`, `sudo dphys-swapfile setup && sudo dphys-swapfile swapon`),
but expect 20+ minutes.

## 5. Test

```sh
ssh caleb@raspi-zero2w-receipt.local
receipt --status
echo "hello from the pi" | receipt
receipt --gen waves --height 600
```

## 6. Reaching it from anywhere

Install Tailscale on the Pi and on your phone or laptop:

```sh
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up
```

The Pi gets a stable private address reachable from any of your devices, with
nothing exposed to the public internet and no port forwarding. This is the part
to get right before running an HTTP endpoint that prints on demand.

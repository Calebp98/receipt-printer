#!/bin/sh
# Cross-compile for the Pi and copy the binary over.
#   ./deploy.sh [host] [user]
set -e

HOST="${1:-raspi-zero2w-receipt.local}"
USER="${2:-caleb}"
TARGET=aarch64-unknown-linux-gnu

# zig is the cross toolchain: it supplies the aarch64 linker and the C compiler
# that builds the vendored libusb. Docker is not involved.
#   brew install zig
#   cargo install cargo-zigbuild        # 0.21.8 if your rustc is older than 1.88
#   rustup target add aarch64-unknown-linux-gnu
command -v zig >/dev/null || { echo "zig is not installed: brew install zig" >&2; exit 1; }
command -v cargo-zigbuild >/dev/null || {
    echo "cargo-zigbuild is not installed: cargo install cargo-zigbuild" >&2
    exit 1
}

# .2.36 pins the glibc the binary asks for, well below the Pi's own, so the
# build does not depend on whatever glibc zig defaults to this release.
cargo zigbuild --release --target "$TARGET.2.36" --features "vendored-usb server"

scp "target/$TARGET/release/receipt" "target/$TARGET/release/receipt-server" "$USER@$HOST:/tmp/"
# -t so the remote sudo can prompt for a password on this terminal. The
# service is restarted rather than reloaded: it holds the USB claim, so the
# old process has to let go before the new one can open the printer.
ssh -t "$USER@$HOST" 'sudo install -m 755 /tmp/receipt /usr/local/bin/receipt \
    && sudo install -m 755 /tmp/receipt-server /usr/local/bin/receipt-server \
    && rm /tmp/receipt /tmp/receipt-server \
    && sudo systemctl try-restart receipt-server \
    && receipt --status'
echo "deployed to $HOST"

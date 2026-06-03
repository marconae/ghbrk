#!/bin/bash
set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"

echo "==> Building release binary..."
cargo build --release --manifest-path "$REPO/Cargo.toml"

echo "==> Installing (requires sudo)..."
sudo bash "$REPO/deploy/linux/install.sh"

echo "==> Waiting for daemon socket..."
SOCK=/var/run/ghbrk/broker.sock
for i in $(seq 1 10); do
    [ -S "$SOCK" ] && break
    sleep 0.5
done
if [ ! -S "$SOCK" ]; then
    echo "ERROR: daemon socket not found after 5s" >&2
    sudo systemctl status ghbrk >&2
    exit 1
fi

echo "==> Verifying..."
ghbrk doctor

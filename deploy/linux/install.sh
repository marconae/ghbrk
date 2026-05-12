#!/bin/bash
set -euo pipefail

BINARY_SRC="./target/release/ghbrk"
BINARY_DST="/usr/local/bin/ghbrk"
SERVICE_SRC="$(dirname "$0")/ghbrk.service"
SERVICE_DST="/etc/systemd/system/ghbrk.service"
POLICY_SRC="$(dirname "$0")/../../config/policy.example.yaml"
POLICY_DST="/etc/ghbrk/policy.yaml"

# ---------------------------------------------------------------------------
# 1. Create system user ghbrk (idempotent)
# ---------------------------------------------------------------------------
if ! id ghbrk &>/dev/null; then
    useradd --system --shell /usr/sbin/nologin --no-create-home ghbrk
    echo "Created system user: ghbrk"
else
    echo "User ghbrk already exists, skipping."
fi

# ---------------------------------------------------------------------------
# 2. Create group ghbrk-clients (idempotent)
# ---------------------------------------------------------------------------
if ! getent group ghbrk-clients &>/dev/null; then
    groupadd --system ghbrk-clients
    echo "Created group: ghbrk-clients"
else
    echo "Group ghbrk-clients already exists, skipping."
fi

# ---------------------------------------------------------------------------
# 3. Install binary (only if built artefact is present)
# ---------------------------------------------------------------------------
if [ -f "$BINARY_SRC" ]; then
    install -m 0755 -o root -g root "$BINARY_SRC" "$BINARY_DST"
    echo "Installed binary to $BINARY_DST"
else
    echo "WARNING: $BINARY_SRC not found — run 'cargo build --release' first."
fi

# ---------------------------------------------------------------------------
# 4. Create directories with correct ownership and modes
# ---------------------------------------------------------------------------
install -d -m 0755 /etc/ghbrk
install -d -m 0700 -o ghbrk -g ghbrk /etc/ghbrk/credentials
install -d -m 0750 -o ghbrk -g ghbrk-clients /var/run/ghbrk
install -d -m 0750 -o ghbrk -g ghbrk-clients /var/log/ghbrk

# ---------------------------------------------------------------------------
# 5. Install example policy (no overwrite if already present)
# ---------------------------------------------------------------------------
if [ ! -f "$POLICY_DST" ]; then
    install -m 0644 -o root -g root "$POLICY_SRC" "$POLICY_DST"
    echo "Installed example policy to $POLICY_DST"
else
    echo "Policy file $POLICY_DST already exists, skipping."
fi

# ---------------------------------------------------------------------------
# 6. Install systemd unit
# ---------------------------------------------------------------------------
install -m 0644 -o root -g root "$SERVICE_SRC" "$SERVICE_DST"
echo "Installed systemd unit to $SERVICE_DST"

# ---------------------------------------------------------------------------
# 7. Reload systemd if available
# ---------------------------------------------------------------------------
if command -v systemctl &>/dev/null; then
    systemctl daemon-reload
    echo "Reloaded systemd daemon."
fi

# ---------------------------------------------------------------------------
# Done — print enable instructions
# ---------------------------------------------------------------------------
echo ""
echo "Installation complete."
echo "To enable and start ghbrk:"
echo "  systemctl enable ghbrk"
echo "  systemctl start ghbrk"
echo ""
echo "To check status:"
echo "  systemctl status ghbrk"
echo "  journalctl -u ghbrk -f"

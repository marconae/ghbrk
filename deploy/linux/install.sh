#!/bin/bash
set -euo pipefail

BINARY_SRC="./target/release/ghbrk"
BINARY_DST="/usr/local/bin/ghbrk"
SERVICE_SRC="$(dirname "$0")/ghbrk.service"
SERVICE_DST="/etc/systemd/system/ghbrk.service"
TMPFILES_SRC="$(dirname "$0")/ghbrk.tmpfiles"
TMPFILES_DST="/etc/tmpfiles.d/ghbrk.conf"
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
# 2b. Add ghbrk user to ghbrk-clients group (idempotent)
# ---------------------------------------------------------------------------
# -aG appends (vs. -G which replaces supplementary groups). Idempotent on re-run.
usermod -aG ghbrk-clients ghbrk
echo "Added ghbrk to group ghbrk-clients."

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
install -d -m 0750 -o ghbrk -g ghbrk-clients /var/log/ghbrk

# ---------------------------------------------------------------------------
# 5. Install example policy (no overwrite if already present)
# ---------------------------------------------------------------------------
if [ ! -f "$POLICY_DST" ]; then
    if [ -f "$POLICY_SRC" ]; then
        install -m 0600 -o ghbrk -g ghbrk "$POLICY_SRC" "$POLICY_DST"
        echo "Installed example policy to $POLICY_DST"
    else
        install -m 0600 -o ghbrk -g ghbrk /dev/null "$POLICY_DST"
        echo "Created empty policy file at $POLICY_DST"
    fi
else
    echo "Policy file $POLICY_DST already exists, skipping."
fi
# Unconditionally correct owner and mode (idempotent on re-run).
chown ghbrk:ghbrk "$POLICY_DST"
chmod 0600 "$POLICY_DST"

# ---------------------------------------------------------------------------
# 7. Install systemd unit and tmpfiles snippet
# ---------------------------------------------------------------------------
install -m 0644 -o root -g root "$SERVICE_SRC" "$SERVICE_DST"
echo "Installed systemd unit to $SERVICE_DST"
install -m 0644 -o root -g root "$TMPFILES_SRC" "$TMPFILES_DST"
echo "Installed tmpfiles snippet to $TMPFILES_DST"

# ---------------------------------------------------------------------------
# 8. Create /run/ghbrk and start the service
# ---------------------------------------------------------------------------
if command -v systemctl &>/dev/null; then
    systemctl daemon-reload
    systemd-tmpfiles --create "$TMPFILES_DST"
    systemctl enable ghbrk
    systemctl restart ghbrk
    echo "ghbrk service enabled and started."
fi

# ---------------------------------------------------------------------------
# 10. Add the invoking user to ghbrk-clients (if running under sudo)
# ---------------------------------------------------------------------------
# When invoked via sudo, $SUDO_USER holds the original (unprivileged) user. We
# add that user to ghbrk-clients so their next login can talk to the broker
# socket. When the script is run as actual root (no sudo), $SUDO_USER is unset
# and we print a manual-add instruction instead.
INVOKER="${SUDO_USER:-}"
if [ -n "$INVOKER" ]; then
    usermod -aG ghbrk-clients "$INVOKER"
    echo "Added $INVOKER to group ghbrk-clients."
    echo "NOTE: log out and back in for the group change to take effect."
else
    echo "NOTE: not invoked via sudo; no user was added to ghbrk-clients."
    echo "      Run 'usermod -aG ghbrk-clients <username>' manually for each"
    echo "      user that should be allowed to talk to the broker."
fi

# ---------------------------------------------------------------------------
# Done — summarise what the script accomplished
# ---------------------------------------------------------------------------
echo ""
echo "Installation complete:"
echo "  - ghbrk service enabled and running"
if [ -n "$INVOKER" ]; then
    echo "  - $INVOKER added to group ghbrk-clients (effective at next login)"
else
    echo "  - no invoking user detected; add operators to ghbrk-clients manually"
fi
echo ""
echo "Remaining manual steps:"
echo "  1. Copy credentials into /etc/ghbrk/credentials/<username>/"
echo "  2. Edit /etc/ghbrk/policy.yaml to allow the operations you need"

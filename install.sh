#!/bin/bash
set -euo pipefail

# ghbrk one-line installer — Linux x86_64 + systemd
# Usage: curl -fsSL https://raw.githubusercontent.com/marconae/ghbrk/main/install.sh | sudo bash

REPO="marconae/ghbrk"
BINARY_DST="/usr/local/bin/ghbrk"
SERVICE_DST="/etc/systemd/system/ghbrk.service"
TMPFILES_DST="/etc/tmpfiles.d/ghbrk.conf"
POLICY_DST="/etc/ghbrk/policy.yaml"

if [ "$(id -u)" -ne 0 ]; then
    echo "ERROR: this installer must run as root (use sudo)." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 1. Download latest release binary
# ---------------------------------------------------------------------------
VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/')

if [ -z "$VERSION" ]; then
    echo "ERROR: could not determine latest release version." >&2
    exit 1
fi

ARCHIVE="ghbrk-${VERSION}-x86_64-linux"
TMPDIR=$(mktemp -d)
trap "rm -rf ${TMPDIR}" EXIT

echo "Downloading ghbrk ${VERSION}..."
curl -fsSL "https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}.tar.gz" \
    | tar xz -C "${TMPDIR}"

install -m 0755 -o root -g root "${TMPDIR}/${ARCHIVE}/ghbrk" "${BINARY_DST}"
echo "Installed binary to ${BINARY_DST}"

# ---------------------------------------------------------------------------
# 2. Create system user and group (idempotent)
# ---------------------------------------------------------------------------
if ! id ghbrk &>/dev/null; then
    useradd --system --shell /usr/sbin/nologin --no-create-home ghbrk
    echo "Created system user: ghbrk"
else
    echo "User ghbrk already exists, skipping."
fi

if ! getent group ghbrk-clients &>/dev/null; then
    groupadd --system ghbrk-clients
    echo "Created group: ghbrk-clients"
else
    echo "Group ghbrk-clients already exists, skipping."
fi

usermod -aG ghbrk-clients ghbrk

# ---------------------------------------------------------------------------
# 3. Create directories
# ---------------------------------------------------------------------------
install -d -m 0755 /etc/ghbrk
install -d -m 0700 -o ghbrk -g ghbrk /etc/ghbrk/credentials
install -d -m 0750 -o ghbrk -g ghbrk-clients /var/log/ghbrk

# ---------------------------------------------------------------------------
# 4. Write systemd unit
# ---------------------------------------------------------------------------
cat > "${SERVICE_DST}" << 'EOF'
[Unit]
Description=ghbrk — privilege-separated git/gh broker
After=network.target

[Service]
Type=simple
User=ghbrk
Group=ghbrk-clients
ExecStart=/usr/local/bin/ghbrk daemon
Environment=GHBRK_POLICY=/etc/ghbrk/policy.yaml
Environment=GHBRK_AUDIT_LOG=/var/log/ghbrk/audit.log
Environment=GHBRK_SOCKET=/run/ghbrk/broker.sock
Environment=HOME=/run/ghbrk
Restart=on-failure
RestartSec=5s
NoNewPrivileges=true
AmbientCapabilities=CAP_SETUID CAP_SETGID
CapabilityBoundingSet=CAP_SETUID CAP_SETGID
ProtectSystem=strict
PrivateTmp=true
ReadWritePaths=/run/ghbrk /var/log/ghbrk
ProtectHome=no
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true

[Install]
WantedBy=multi-user.target
EOF
echo "Installed systemd unit to ${SERVICE_DST}"

# ---------------------------------------------------------------------------
# 5. Write tmpfiles snippet
# ---------------------------------------------------------------------------
cat > "${TMPFILES_DST}" << 'EOF'
# Create /run/ghbrk on every boot with the correct ownership and mode.
# Mode 2750: setgid so socket inherits ghbrk-clients group; no world access.
d /run/ghbrk 2750 ghbrk ghbrk-clients -
EOF
echo "Installed tmpfiles snippet to ${TMPFILES_DST}"

# ---------------------------------------------------------------------------
# 6. Write starter policy (no overwrite if already present)
# ---------------------------------------------------------------------------
if [ ! -f "${POLICY_DST}" ]; then
    cat > "${POLICY_DST}" << 'EOF'
# ghbrk policy — edit this file, then run: sudo systemctl restart ghbrk
#
# Rules are evaluated top-to-bottom; first match wins. Default: deny.
#
# Example: allow alice to push feature branches and open PRs in acme/platform
#
# rules:
#   - user: alice
#     org: acme
#     repo: platform
#     operations: [push]
#     branches: ["feature/*"]
#     effect: allow
#
#   - user: alice
#     org: acme
#     repo: platform
#     operations: [pr_open, pr_comment]
#     effect: allow

rules: []
EOF
    chown ghbrk:ghbrk "${POLICY_DST}"
    chmod 0600 "${POLICY_DST}"
    echo "Installed starter policy to ${POLICY_DST}"
else
    echo "Policy file ${POLICY_DST} already exists, skipping."
fi

# ---------------------------------------------------------------------------
# 7. Enable and start the service
# ---------------------------------------------------------------------------
if command -v systemctl &>/dev/null; then
    systemctl daemon-reload
    systemd-tmpfiles --create "${TMPFILES_DST}"
    systemctl enable ghbrk
    systemctl restart ghbrk
    echo "ghbrk service enabled and started."
else
    echo "WARNING: systemctl not found; start the service manually."
fi

# ---------------------------------------------------------------------------
# 8. Add the invoking user to ghbrk-clients
# ---------------------------------------------------------------------------
INVOKER="${SUDO_USER:-}"
if [ -n "$INVOKER" ]; then
    usermod -aG ghbrk-clients "$INVOKER"
    echo "Added ${INVOKER} to group ghbrk-clients."
    echo "NOTE: log out and back in for the group change to take effect."
else
    echo "NOTE: not invoked via sudo; add operators manually:"
    echo "      sudo usermod -aG ghbrk-clients <username>"
fi

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
echo ""
echo "Installation complete. Next steps:"
echo "  1. Add credentials:  sudo mkdir -p /etc/ghbrk/credentials/<username>"
echo "                       sudo install -m 0600 -o ghbrk ~/.ssh/id_rsa /etc/ghbrk/credentials/<username>/id_rsa"
echo "  2. Edit policy:      sudo \$EDITOR /etc/ghbrk/policy.yaml"
echo "  3. Reload:           sudo systemctl restart ghbrk"
echo "  4. Verify:           ghbrk doctor"
echo ""
echo "Full documentation: https://github.com/marconae/ghbrk/blob/main/docs/install.md"

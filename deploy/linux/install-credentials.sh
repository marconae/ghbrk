#!/bin/bash
set -euo pipefail

# ---------------------------------------------------------------------------
# Must run as root
# ---------------------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
    echo "ERROR: run this script with sudo." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Resolve the target Unix username
# ---------------------------------------------------------------------------
# Prefer $SUDO_USER (the unprivileged caller); fall back to logname, then $USER.
USERNAME="${SUDO_USER:-}"
if [ -z "$USERNAME" ]; then
    USERNAME="$(logname 2>/dev/null || true)"
fi
if [ -z "$USERNAME" ] || [ "$USERNAME" = "root" ]; then
    USERNAME="${USER:-root}"
fi

CRED_DIR="/etc/ghbrk/credentials/$USERNAME"

# ---------------------------------------------------------------------------
# 1. Prompt for SSH private key path
# ---------------------------------------------------------------------------
DEFAULT_KEY="$HOME/.ssh/id_ed25519"
# When run via sudo, HOME is root's home; try the real user's home instead.
if [ -n "${SUDO_USER:-}" ]; then
    DEFAULT_KEY="$(getent passwd "$SUDO_USER" | cut -d: -f6)/.ssh/id_ed25519"
fi

read -rp "SSH private key path [${DEFAULT_KEY}]: " KEY_PATH
KEY_PATH="${KEY_PATH:-$DEFAULT_KEY}"

if [ ! -f "$KEY_PATH" ]; then
    echo "ERROR: file not found: $KEY_PATH" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 2. Prompt for GitHub token (silent — never echoed, never in history)
# ---------------------------------------------------------------------------
read -rsp "GitHub token: " TOKEN
echo  # newline after silent input

if [ -z "$TOKEN" ]; then
    echo "ERROR: token must not be empty." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 3. Create per-user credential directory
# ---------------------------------------------------------------------------
install -d -m 0700 -o ghbrk -g ghbrk "$CRED_DIR"
echo "Credential directory: $CRED_DIR"

# ---------------------------------------------------------------------------
# 4. Install SSH key
# ---------------------------------------------------------------------------
install -m 0600 -o ghbrk -g ghbrk "$KEY_PATH" "$CRED_DIR/id_rsa"
echo "Installed SSH key:    $CRED_DIR/id_rsa"

# ---------------------------------------------------------------------------
# 5. Write token
# ---------------------------------------------------------------------------
# printf is a bash builtin — TOKEN is never passed as an argument to an
# external process and will not appear in ps or /proc/<pid>/cmdline.
printf '%s' "$TOKEN" | install -m 0600 -o ghbrk -g ghbrk /dev/stdin \
    "$CRED_DIR/token"
unset TOKEN
echo "Installed token:      $CRED_DIR/token"

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
echo ""
echo "Credentials installed for user: $USERNAME"
echo "  $CRED_DIR/id_rsa  (SSH private key)"
echo "  $CRED_DIR/token   (GitHub token)"

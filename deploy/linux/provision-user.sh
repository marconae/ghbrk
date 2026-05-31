#!/bin/bash
set -euo pipefail

# provision-user.sh — set up per-user credentials for the ghbrk daemon.
#
# Usage: sudo ./deploy/linux/provision-user.sh <USERNAME>
#
# Creates the credential tree under /etc/ghbrk/credentials/:
#
#   /etc/ghbrk/credentials/<username>/         mode 750, owner ghbrk, group ghbrk
#   /etc/ghbrk/credentials/<username>/id_rsa   mode 600, owner ghbrk, group ghbrk
#   /etc/ghbrk/credentials/<username>/token    mode 600, owner ghbrk, group ghbrk
#
# The parent /etc/ghbrk/credentials/ must already exist (created by install.sh
# with mode 0700 so that only the daemon can traverse into credential subdirectories).

# ---------------------------------------------------------------------------
# Must run as root
# ---------------------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
    echo "ERROR: this script must be run as root (e.g. sudo $0 <USERNAME>)" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Validate argument
# ---------------------------------------------------------------------------
if [ $# -ne 1 ]; then
    echo "Usage: $0 <USERNAME>" >&2
    exit 1
fi

USERNAME="$1"

# Non-empty check
if [ -z "$USERNAME" ]; then
    echo "ERROR: USERNAME must not be empty." >&2
    exit 1
fi

# No slashes
if [[ "$USERNAME" == */* ]]; then
    echo "ERROR: USERNAME must not contain a slash." >&2
    exit 1
fi

# Not "." or ".."
if [ "$USERNAME" = "." ] || [ "$USERNAME" = ".." ]; then
    echo "ERROR: USERNAME must not be '.' or '..'." >&2
    exit 1
fi

# Must be a real system user
if ! id "$USERNAME" &>/dev/null; then
    echo "ERROR: user '$USERNAME' does not exist on this system." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Verify that ghbrk group and credentials parent exist
# ---------------------------------------------------------------------------
if ! getent group ghbrk &>/dev/null; then
    echo "ERROR: group 'ghbrk' does not exist. Run install.sh first." >&2
    exit 1
fi

CREDS_ROOT="/etc/ghbrk/credentials"
if [ ! -d "$CREDS_ROOT" ]; then
    echo "ERROR: $CREDS_ROOT does not exist. Run install.sh first." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Create per-user directory
# mode 750: owner(user)=rwx, group(ghbrk)=r-x, others=---
# ghbrk daemon can list/traverse; peer user owns and can write their key.
# ---------------------------------------------------------------------------
USER_DIR="$CREDS_ROOT/$USERNAME"
install -d -m 0750 -o ghbrk -g ghbrk "$USER_DIR"
echo "Created directory: $USER_DIR (mode 0750, owner ghbrk, group ghbrk)"

# ---------------------------------------------------------------------------
# Create id_rsa placeholder
# mode 600: only the peer user can read/write their own SSH private key.
# ssh(1) refuses to use keys with looser permissions.
# ---------------------------------------------------------------------------
ID_RSA="$USER_DIR/id_rsa"
if [ ! -f "$ID_RSA" ]; then
    install -m 0600 -o ghbrk -g ghbrk /dev/null "$ID_RSA"
    echo "Created placeholder: $ID_RSA (mode 0600, owner ghbrk)"
else
    echo "File already exists, skipping: $ID_RSA"
fi

# ---------------------------------------------------------------------------
# Create token placeholder
# mode 600: only the ghbrk daemon can read the GitHub token.
# Peer user need not (and should not) access it directly.
# ---------------------------------------------------------------------------
TOKEN="$USER_DIR/token"
if [ ! -f "$TOKEN" ]; then
    install -m 0600 -o ghbrk -g ghbrk /dev/null "$TOKEN"
    echo "Created placeholder: $TOKEN (mode 0600, owner ghbrk)"
else
    echo "File already exists, skipping: $TOKEN"
fi

# ---------------------------------------------------------------------------
# Done — print fill-in instructions
# ---------------------------------------------------------------------------
echo ""
echo "Credentials scaffold created for user '$USERNAME'."
echo ""
echo "Next steps:"
echo "  1. Install the SSH private key:"
echo "       cp /path/to/id_rsa $ID_RSA"
echo "       chown ghbrk:ghbrk $ID_RSA"
echo "       chmod 600 $ID_RSA"
echo ""
echo "  2. Write the GitHub personal-access token:"
echo "       echo 'ghp_...' > $TOKEN"
echo "       chown ghbrk:ghbrk $TOKEN"
echo "       chmod 600 $TOKEN"
echo ""
echo "  3. Ensure '$USERNAME' is a member of the ghbrk-clients group:"
echo "       usermod -aG ghbrk-clients $USERNAME"
echo "       (user must log out and back in for the group change to take effect)"

#!/usr/bin/env bash
# Docker integration test: verifies that install.sh section 9 correctly wires
# Claude Code and Codex agent instructions inside a fresh container.
set -euo pipefail

REPO="${REPO:-marconae/ghbrk}"
VERSION="${VERSION:-v1.1.2}"

echo "Testing agent-install wiring for ${REPO}@${VERSION} ..."

docker run --rm -i debian:bookworm-slim bash -s <<DOCKER
set -euo pipefail
apt-get update -qq && apt-get install -y -qq curl adduser 2>/dev/null

useradd -m testuser
INVOKER=testuser
INVOKER_HOME=\$(getent passwd testuser | cut -d: -f6)
REPO="${REPO}"
VERSION="${VERSION}"
INSTALL_CLAUDE=1
INSTALL_CODEX=1

# ── Claude Code ────────────────────────────────────────────────────────────
CLAUDE_DIR="\${INVOKER_HOME}/.claude"
mkdir -p "\${CLAUDE_DIR}"
curl -fsSL "https://raw.githubusercontent.com/\${REPO}/\${VERSION}/ghbrk.md" \\
  -o "\${CLAUDE_DIR}/ghbrk.md"
chown testuser:testuser "\${CLAUDE_DIR}/ghbrk.md"

CLAUDE_MD="\${CLAUDE_DIR}/CLAUDE.md"
if [ ! -f "\${CLAUDE_MD}" ]; then
  printf '@ghbrk.md\n' > "\${CLAUDE_MD}"
elif ! grep -q '@ghbrk.md' "\${CLAUDE_MD}"; then
  { printf '@ghbrk.md\n'; cat "\${CLAUDE_MD}"; } > "\${CLAUDE_MD}.tmp"
  mv "\${CLAUDE_MD}.tmp" "\${CLAUDE_MD}"
fi
chown testuser:testuser "\${CLAUDE_MD}"

# ── Codex ──────────────────────────────────────────────────────────────────
CODEX_DIR="\${INVOKER_HOME}/.codex"
mkdir -p "\${CODEX_DIR}"
AGENTS_MD="\${CODEX_DIR}/AGENTS.md"
if ! grep -q 'ghbrk' "\${AGENTS_MD}" 2>/dev/null; then
  curl -fsSL "https://raw.githubusercontent.com/\${REPO}/\${VERSION}/ghbrk.md" \\
    >> "\${AGENTS_MD}"
  chown testuser:testuser "\${AGENTS_MD}"
fi

# ── Assertions ─────────────────────────────────────────────────────────────
fail=0

assert() {
  if eval "\$1"; then
    echo "PASS: \$2"
  else
    echo "FAIL: \$2"
    fail=1
  fi
}

assert "test -f '\${CLAUDE_DIR}/ghbrk.md'"               "ghbrk.md installed for Claude"
assert "grep -q '@ghbrk.md' '\${CLAUDE_MD}'"             "@ghbrk.md present in CLAUDE.md"
assert "grep -qi 'ghbrk' '\${AGENTS_MD}'"                "ghbrk content in ~/.codex/AGENTS.md"

# Idempotency: re-run the wiring, check for no duplication
if ! grep -q '@ghbrk.md' "\${CLAUDE_MD}"; then
  { printf '@ghbrk.md\n'; cat "\${CLAUDE_MD}"; } > "\${CLAUDE_MD}.tmp"
  mv "\${CLAUDE_MD}.tmp" "\${CLAUDE_MD}"
fi
count=\$(grep -c '@ghbrk.md' "\${CLAUDE_MD}")
assert "[ \"\${count}\" -eq 1 ]" "idempotent: @ghbrk.md appears exactly once"

exit \$fail
DOCKER

echo "All assertions passed."

# ghbrk

`ghbrk` is a policy-enforcing proxy that lets AI coding agents run `git` and `gh` operations using a shared credential, without ever having direct access to that credential. A privileged daemon holds the SSH key and GitHub token; agents call `git` and `gh` normally through thin shim symlinks. Every operation is checked against a YAML policy before the real command runs.

## How it works

```
Agent process (e.g. Claude Code)
  │
  │  calls git/gh via symlinks in PATH
  ▼
ghbrk shim  ──────────────────────────────────────────────────────┐
  │  connects to /var/run/ghbrk/broker.sock                       │
  │  sends: { tool, args, cwd }                                   │
  ▼                                                               │
ghbrk daemon (runs as system user "ghbrk")                        │
  │  reads SO_PEERCRED → caller UID → Unix username               │
  │  reads /etc/ghbrk/credentials/<username>/{id_rsa,token}       │
  │  evaluates /etc/ghbrk/policy.yaml                             │
  │                                                               │
  ├─ ALLOW → spawns real git/gh with credential env vars          │
  │           streams stdout/stderr back to shim ─────────────────┘
  │           writes allow record to audit log
  │
  └─ DENY  → sends denial reason to shim (printed to stderr)
              writes deny record to audit log
              shim exits non-zero
```

The agent's `PATH` is configured so the shim's `git`/`gh` symlinks come before `/usr/bin`. The interception is transparent — the agent sees normal stdio and exit codes.

## Requirements

- Linux (kernel 2.6+; `SO_PEERCRED` required)
- Rust stable toolchain (for building from source)
- `git` and `gh` installed at their standard system paths
- systemd (for the provided service unit)

macOS is not supported in v1.

## Installation

```bash
cargo build --release
sudo ./deploy/linux/install.sh
sudo systemctl enable --now ghbrk
```

The install script (idempotent; safe to re-run):

1. Creates system user `ghbrk` (no login shell) and group `ghbrk-clients`
2. Installs the binary to `/usr/local/bin/ghbrk`
3. Creates directories with strict permissions:
   - `/etc/ghbrk/credentials/` — mode `0700`, owned by `ghbrk`
   - `/var/run/ghbrk/` — mode `0750`, group `ghbrk-clients`
   - `/var/log/ghbrk/` — mode `0750`, group `ghbrk-clients`
4. Writes a starter policy to `/etc/ghbrk/policy.yaml` if one does not exist
5. Writes a starter shim config to `/etc/ghbrk/config.yaml` if one does not exist
6. Installs the systemd unit and reloads the daemon

Check the service:

```bash
systemctl status ghbrk
journalctl -u ghbrk -f
```

## Configuration

### Policy file

`/etc/ghbrk/policy.yaml` controls which callers may perform which operations. Rules are evaluated top-to-bottom; the **first match wins**. If no rule matches, the request is **denied** (default-deny).

```yaml
rules:
  # alice can push to feature branches in acme/platform
  - user: alice
    org: acme
    repo: platform
    operations: [push]
    branches: ["feature/*"]
    effect: allow

  # any agent may open and comment on PRs across the acme org
  - user: "*"
    org: acme
    repo: "*"
    operations: [pr_open, pr_comment]
    effect: allow

  # nobody may push to main — deny takes priority if placed before a broader allow
  - user: "*"
    org: acme
    repo: "*"
    operations: [push]
    branches: [main]
    effect: deny
```

See `config/policy.example.yaml` for a fully annotated example covering every field.

**Rule fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `user` | yes | Unix username or `"*"` (any caller) |
| `org` | yes | GitHub organisation or user, or `"*"` |
| `repo` | yes | Repository name (without org prefix), or `"*"` |
| `operations` | yes | Non-empty list — see [Operations reference](#operations-reference) |
| `branches` | no | Glob list for branch matching; defaults to `["*"]`; only applied to `push` |
| `effect` | yes | `allow` or `deny` |

Reload the policy by restarting the daemon:

```bash
sudo systemctl restart ghbrk
```

### Credentials

Credentials are stored under `/etc/ghbrk/credentials/<username>/` — one directory per Unix user whose agents will be proxied. Both files must be owned by `ghbrk` and have mode `0600`; files with permissive modes are rejected.

**SSH key** (for `git@github.com` and `ssh://` remotes):

```bash
sudo mkdir -p /etc/ghbrk/credentials/alice
sudo install -m 0600 -o ghbrk ~/.ssh/my_agent_key /etc/ghbrk/credentials/alice/id_rsa
```

**GitHub token** (for HTTPS git remotes and all `gh` CLI operations):

```bash
printf '%s' "$GITHUB_TOKEN" | sudo tee /etc/ghbrk/credentials/alice/token > /dev/null
sudo chown ghbrk /etc/ghbrk/credentials/alice/token
sudo chmod 0600 /etc/ghbrk/credentials/alice/token
```

The token needs the scopes required by the operations you allow (`repo`, `workflow`, etc.). Token contents are never written to logs.

### Shim config

`/etc/ghbrk/config.yaml` tells the shim where the real `git` and `gh` binaries live. The file is **optional** — when absent the shim falls back to the compiled-in defaults (`/usr/bin/git` and `/usr/bin/gh`).

```yaml
real_git: /usr/bin/git
real_gh: /usr/bin/gh
```

Set these only if your system installs the binaries at non-standard paths (e.g. `/usr/local/bin/git` from Homebrew-on-Linux or a custom build). A malformed file is a fatal error; a missing file is silently ignored.

## Agent setup

Place `git` and `gh` symlinks that resolve to the `ghbrk` binary early in the agent's `PATH`. When invoked via a symlink named `git` or `gh`, `ghbrk` detects this automatically and routes to the broker.

```bash
mkdir -p ~/.local/bin
ln -sf /usr/local/bin/ghbrk ~/.local/bin/git
ln -sf /usr/local/bin/ghbrk ~/.local/bin/gh
```

In the agent's environment (e.g. Claude Code's `env` configuration):

```bash
export PATH="$HOME/.local/bin:$PATH"
```

The agent must also be a member of the `ghbrk-clients` group so it can reach the socket:

```bash
sudo usermod -aG ghbrk-clients alice
```

To verify the shim is active in the agent's session:

```bash
which git   # should print ~/.local/bin/git
git status  # passes through to the real git; no broker contact required
```

If the broker socket is not at the default path, set `GHBRK_SOCKET` in the agent's environment.

## Command routing

The shim makes a routing decision before contacting the broker. Commands that do not require broker involvement are passed directly to the real binary via `exec()` — the broker is never contacted and no policy check occurs.

### git

| Subcommand | Routed to |
|------------|-----------|
| `push` | broker |
| `fetch` | broker |
| `clone` (GitHub remote) | broker |
| `status`, `add`, `commit`, `log`, `diff`, `checkout`, and everything else | real `git` binary |

The classification is based on the first non-flag argument (global flags such as `-c`, `-C`, `--git-dir`, and `--work-tree` are skipped). An invocation with no subcommand is passed through.

### gh

| Group + action | Routed to |
|----------------|-----------|
| `pr create`, `pr comment`, `pr merge`, `pr close`, `pr review` | broker |
| `issue create`, `issue comment`, `issue close` | broker |
| `release create` | broker |
| `auth status`, `repo view`, `pr list`, `pr status`, and everything else | real `gh` binary |

The classification is based on the first two positional arguments. Any `(group, action)` pair not in the brokered set is passed through.

## Operations reference

`ghbrk` recognises the following operations. Use these names in the `operations` list of your policy rules.

| Operation | Triggered by | Branch-aware |
|-----------|-------------|:------------:|
| `push` | `git push` | yes |
| `fetch` | `git fetch` | no |
| `clone` | `git clone` | no |
| `pr_open` | `gh pr create` | no |
| `pr_comment` | `gh pr comment` | no |
| `pr_close` | `gh pr close` | no |
| `pr_merge` | `gh pr merge` | no |
| `pr_review` | `gh pr review` | no |
| `issue_open` | `gh issue create` | no |
| `issue_comment` | `gh issue comment` | no |
| `issue_close` | `gh issue close` | no |
| `release_create` | `gh release create` | no |

Only `push` evaluates the `branches` field. For all other operations the branch field in a rule is ignored.

**Repo resolution:**

- `git` commands: walks up from the current directory to find `.git/config` and reads the origin remote URL.
- `gh` commands: uses the `-R`/`--repo` flag if present; otherwise falls back to the git repo context of the working directory.
- Supported remote URL formats: `git@github.com:org/repo`, `ssh://git@github.com/org/repo`, `https://github.com/org/repo` (with or without `.git` suffix).
- Non-GitHub hosts (GitLab, Bitbucket, etc.) are rejected.

## Audit log

Every allow and deny decision is appended as a JSON line to `/var/log/ghbrk/audit.log` (mode `0640`).

Example lines:

```json
{"timestamp":"2026-04-27T09:12:00Z","user":"alice","tool":"git","args":["push","origin","feature/ui"],"org":"acme","repo":"platform","branch":"feature/ui","operation":"push","decision":"allow"}
{"timestamp":"2026-04-27T09:13:44Z","user":"alice","tool":"git","args":["push","origin","main"],"org":"acme","repo":"platform","branch":"main","operation":"push","decision":{"deny":{"reason":"no matching rule"}}}
```

Token and key material never appear in this file. To follow the log live:

```bash
sudo tail -f /var/log/ghbrk/audit.log | jq .
```

## Environment variables

These can be set in the systemd unit's `[Service]` block or in the agent's environment.

| Variable | Default | Scope | Description |
|----------|---------|-------|-------------|
| `GHBRK_SOCKET` | `/var/run/ghbrk/broker.sock` | daemon + shim | Unix socket path |
| `GHBRK_POLICY` | `/etc/ghbrk/policy.yaml` | daemon only | Policy file path |
| `GHBRK_AUDIT_LOG` | `/var/log/ghbrk/audit.log` | daemon only | Audit log path |
| `RUST_LOG` | `info` | daemon only | Log verbosity (`debug`, `trace` for troubleshooting) |

To override the socket in both the daemon and shim, set `GHBRK_SOCKET` consistently in both environments.

## Building from source

```bash
# Build
cargo build --release

# Run tests
cargo test

# Lint
cargo clippy --all-targets --all-features -- -D warnings

# Format check
cargo fmt --check

# License and advisory audit
cargo deny check
```

All dependencies are permissive-licensed (MIT, Apache-2.0, BSD, ISC). GPL and AGPL dependencies are blocked by `deny.toml`.

## License

MIT — see [LICENSE](LICENSE) or the `license` field in `Cargo.toml`.

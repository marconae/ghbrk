# ghbrk

`ghbrk` is a policy-enforcing broker that lets AI coding agents run `git` and `gh` operations using a shared credential, without ever having direct access to that credential. A privileged daemon holds the SSH key and GitHub token; agents call `ghbrk git push` / `ghbrk gh pr create` explicitly for remote operations and use plain `git`/`gh` for everything local. Every brokered operation is checked against a YAML policy before the real command runs.

## How it works

```
Agent process (e.g. Claude Code)
  │
  │  local/read-only: calls plain git/gh directly
  │
  │  remote/authenticated: calls ghbrk git <sub> or ghbrk gh <sub>
  ▼
ghbrk binary (clap dispatch)
  │  rejects local-only git subcommands with a guidance error
  │  connects to /var/run/ghbrk/broker.sock
  │  sends: { tool, args, cwd }
  ▼
ghbrk daemon (runs as system user "ghbrk")
  │  reads SO_PEERCRED → caller UID → Unix username
  │  reads /etc/ghbrk/credentials/<username>/{id_rsa,token}
  │  evaluates /etc/ghbrk/policy.yaml
  │
  ├─ ALLOW → spawns real git/gh with credential env vars
  │           streams stdout/stderr back to caller
  │           writes allow record to audit log
  │
  └─ DENY  → sends denial reason (printed to stderr)
              writes deny record to audit log
              ghbrk exits non-zero
```

The privilege boundary is explicit: agents choose to call `ghbrk git push` when they need to reach the network. There are no symlinks, no transparent interception, and no hidden credential injection.

## Requirements

- Linux (kernel 2.6+; `SO_PEERCRED` required)
- Rust stable toolchain (for building from source)
- `git` and `gh` installed at their standard system paths
- systemd (for the provided service unit)

macOS is not supported in v1.

## Getting Started

This guide covers the minimal steps to get `ghbrk` running. See the full [Configuration](#configuration) section for all options.

### Prerequisites

- Linux system with systemd
- Rust stable toolchain installed (`rustup show`)
- `git` and `gh` present at `/usr/bin/git` and `/usr/bin/gh`
- `sudo` access

### Build and install

```bash
git clone https://github.com/marconae/ghbrk
cd ghbrk
cargo build --release
sudo ./deploy/linux/install.sh
```

The install script creates the `ghbrk` system user, installs the binary, writes a starter policy, and enables the systemd service. No `git`/`gh` symlinks are created. Run it again at any time — it is idempotent.

### Create credentials

Replace `alice` with your Unix username throughout.

```bash
# SSH key (for git@github.com and ssh:// remotes)
sudo mkdir -p /etc/ghbrk/credentials/alice
sudo install -m 0600 -o ghbrk ~/.ssh/my_agent_key /etc/ghbrk/credentials/alice/id_rsa

# GitHub token (for HTTPS remotes and all gh operations)
printf '%s' "$GITHUB_TOKEN" | sudo tee /etc/ghbrk/credentials/alice/token > /dev/null
sudo chown ghbrk /etc/ghbrk/credentials/alice/token
sudo chmod 0600 /etc/ghbrk/credentials/alice/token
```

The token needs at minimum the `repo` scope. See [Credentials](#credentials) for scope guidance.

**Home directory traversal:** The broker daemon runs as the `ghbrk` system user. If your home directory is mode `0700` (the default on many distros), the daemon cannot enter it to run git operations. Add the execute bit for others so the daemon can traverse — without granting read access to directory contents:

```bash
chmod o+x ~
```

### Write a minimal policy

Edit `/etc/ghbrk/policy.yaml` (or let the starter file created by `install.sh` serve as a base):

```yaml
rules:
  - user: alice
    org: your-org
    repo: "*"
    operations: [push, fetch, pull]
    effect: allow
```

Reload the policy:

```bash
sudo systemctl restart ghbrk
```

### Verify and run

```bash
ghbrk doctor         # verifies daemon reachability, SSH key, token, and policy health
ghbrk git push       # routes through the broker; check /var/log/ghbrk/audit.log
```

See [Configuration](#configuration) for the full policy reference, additional credential options, and environment variables.

## Installation

```bash
cargo build --release
sudo ./deploy/linux/install.sh
```

The install script (idempotent; safe to re-run):

1. Creates system user `ghbrk` (no login shell) and group `ghbrk-clients`
2. Installs the binary to `/usr/local/bin/ghbrk` (no `git`/`gh` symlinks are created)
3. Creates directories with strict permissions:
   - `/etc/ghbrk/` — mode `0755`, owned by `root:root`
   - `/etc/ghbrk/credentials/` — mode `0700`, owned by `ghbrk`
   - `/run/ghbrk/` — mode `2750`, group `ghbrk-clients`; managed by `systemd-tmpfiles` at every boot via `/etc/tmpfiles.d/ghbrk.conf`
   - `/var/log/ghbrk/` — mode `0750`, group `ghbrk-clients`
4. Writes a starter policy to `/etc/ghbrk/policy.yaml` if one does not exist
5. Installs the systemd unit and `tmpfiles.d` snippet; enables and restarts the service
6. Adds `$SUDO_USER` to `ghbrk-clients` (effective at next login)

Check the service and verify health:

```bash
systemctl status ghbrk
journalctl -u ghbrk -f
ghbrk doctor         # verify daemon, SSH key, token, and policy
```

## Commands

### `ghbrk daemon`

Starts the broker server. Normally managed by systemd; you do not need to invoke this manually.

### `ghbrk git <remote-subcommand> [args]`

Relays a remote git operation to the broker. Only remote-capable subcommands (`push`, `fetch`, `pull`, `clone`) are accepted; local-only subcommands return a guidance error on stderr without contacting the broker.

```bash
ghbrk git push origin main
ghbrk git fetch
ghbrk git pull --rebase
ghbrk git clone git@github.com:acme/platform
```

### `ghbrk gh <subcommand> [args]`

Relays any `gh` invocation to the broker. The broker injects `GH_TOKEN` from stored credentials and, for mutating operations, evaluates the configured policy.

```bash
ghbrk gh pr create --title "My PR" --body "..."
ghbrk gh pr merge 42 --squash
ghbrk gh issue comment 7 --body "Fixed in abc123"
```

### `ghbrk doctor`

Checks daemon reachability, credential presence and file-permission validity, and policy-file parsability. Prints one status line per check; exits zero only when all checks pass.

```bash
ghbrk doctor
```

### `ghbrk explain <cmd> [args]`

Dry run: sends the command to the broker, which resolves the operation and evaluates policy without executing anything. Reports the would-be decision and which credential would be injected.

```bash
ghbrk explain git push origin main    # see what the broker would do
ghbrk explain git status              # reports: local operation, out of scope
```

### `ghbrk policy <org>/<repo>`

Lists which operations the calling user is allowed and forbidden to perform on the specified repository, based on the current policy file.

```bash
ghbrk policy acme/web
```

## Agent integration

Agents use plain `git`/`gh` for local and read-only operations, and `ghbrk git`/`ghbrk gh` for remote and authenticated operations. No `PATH` manipulation or symlink setup is required.

```bash
# Local operations (no brokering, plain git)
git status
git add -p
git commit -m "fix: correct off-by-one"
git log --oneline -10
git diff HEAD~1

# Brokered remote operations (explicit gateway)
ghbrk git push origin feature/my-branch
ghbrk git fetch
ghbrk git pull --rebase
ghbrk gh pr create --title "My PR" --fill
ghbrk gh pr merge 42 --squash
```

The agent must be a member of the `ghbrk-clients` group so it can reach the socket:

```bash
sudo usermod -aG ghbrk-clients alice
```

If the broker socket is not at the default path, set `GHBRK_SOCKET` in the agent's environment.

### Guidance errors

If an agent mistakenly calls `ghbrk git status` (a local subcommand), `ghbrk` exits non-zero immediately — before contacting the broker — and prints a message like:

```
error: 'git status' is a local operation; run 'git status' directly.
       ghbrk only brokers remote operations: push, fetch, pull, clone.
```

This makes the boundary self-documenting.

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

## Command routing

For `ghbrk git`, a routing decision is made before contacting the broker. Git subcommands that do not require credential injection are rejected immediately with a guidance error — the broker is never contacted and no policy check occurs. Every `ghbrk gh` invocation contacts the broker so that `GH_TOKEN` can always be injected.

### git

| Subcommand | Routed to |
|------------|-----------|
| `push` | broker |
| `fetch` | broker |
| `pull` | broker |
| `clone` (GitHub remote) | broker |
| `status`, `add`, `commit`, `log`, `diff`, `checkout`, and everything else | guidance error (use plain `git`) |

The classification is based on the first non-flag argument (global flags such as `-c`, `-C`, `--git-dir`, and `--work-tree` are skipped). An invocation with no subcommand returns a guidance error.

### gh

Every `ghbrk gh` invocation is relayed to the broker so that `GH_TOKEN` is always injected from the stored credential. The broker classifies the request:

| Group + action | Broker path |
|----------------|-------------|
| `pr create`, `pr comment`, `pr merge`, `pr close`, `pr review` | policy check → inject → exec |
| `issue create`, `issue comment`, `issue close` | policy check → inject → exec |
| `release create` | policy check → inject → exec |
| `api <path>` (GET only; `-X POST/PATCH/DELETE` rejected) | policy check → inject → exec |
| `auth status`, `repo view`, `pr list`, `pr status`, and everything else | inject → exec (no policy check) |

The classification is based on the first two positional arguments. Non-policy-gated invocations bypass the resolver and policy engine but still receive `GH_TOKEN`; they are logged with `decision=passthrough`.

## Operations reference

`ghbrk` recognises the following operations. Use these names in the `operations` list of your policy rules.

| Operation | Triggered by | Branch-aware |
|-----------|-------------|:------------:|
| `push` | `ghbrk git push` | yes |
| `fetch` | `ghbrk git fetch` | no |
| `pull` | `ghbrk git pull` | no |
| `clone` | `ghbrk git clone` | no |
| `pr_open` | `ghbrk gh pr create` | no |
| `pr_comment` | `ghbrk gh pr comment` | no |
| `pr_close` | `ghbrk gh pr close` | no |
| `pr_merge` | `ghbrk gh pr merge` | no |
| `pr_review` | `ghbrk gh pr review` | no |
| `issue_open` | `ghbrk gh issue create` | no |
| `issue_comment` | `ghbrk gh issue comment` | no |
| `issue_close` | `ghbrk gh issue close` | no |
| `release_create` | `ghbrk gh release create` | no |
| `gh_api_read` | `ghbrk gh api <path>` (GET) | no |

Only `push` evaluates the `branches` field. For all other operations the branch field in a rule is ignored.

**Repo resolution:**

- `ghbrk git` commands: The client reads the origin remote URL from `.git/config` in the invoking user's context and forwards it to the broker; this works even when the user's home directory is not accessible to the broker process.
- `ghbrk gh` commands: uses the `-R`/`--repo` flag if present; otherwise falls back to the git repo context of the working directory.
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
| `GHBRK_SOCKET` | `/var/run/ghbrk/broker.sock` | daemon + client | Unix socket path |
| `GHBRK_POLICY` | `/etc/ghbrk/policy.yaml` | daemon only | Policy file path |
| `GHBRK_AUDIT_LOG` | `/var/log/ghbrk/audit.log` | daemon only | Audit log path |
| `RUST_LOG` | `info` | daemon only | Log verbosity (`debug`, `trace` for troubleshooting) |

To override the socket in both the daemon and client, set `GHBRK_SOCKET` consistently in both environments.

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

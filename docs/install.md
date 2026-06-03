[ghbrk](../README.md) / [Docs](./index.md) / Installation

---

# Installation

## One-line install (Linux x86_64)

```bash
curl -fsSL https://raw.githubusercontent.com/marconae/ghbrk/main/install.sh | sudo bash
```

The installer downloads the latest release binary, creates the `ghbrk` system user and `ghbrk-clients` group, installs the systemd service, and writes a starter policy. It is idempotent — safe to re-run.

**Requirements:** Linux with systemd, x86_64 CPU, `curl`.

After installation, complete the two manual steps below: add credentials and write a policy.

---

## Add credentials

Credentials are stored under `/etc/ghbrk/credentials/<username>/` — one directory per Unix user whose agents will be proxied. Both files must be owned by `ghbrk` with mode `0600`.

Replace `alice` with your Unix username.

**SSH key** (for `git@github.com` and `ssh://` remotes):

```bash
sudo mkdir -p /etc/ghbrk/credentials/alice
sudo install -m 0600 -o ghbrk ~/.ssh/my_agent_key /etc/ghbrk/credentials/alice/id_rsa
```

**GitHub token** (for HTTPS git remotes and all `gh` operations):

```bash
printf '%s' "$GITHUB_TOKEN" | sudo tee /etc/ghbrk/credentials/alice/token > /dev/null
sudo chown ghbrk /etc/ghbrk/credentials/alice/token
sudo chmod 0600 /etc/ghbrk/credentials/alice/token
```

The token needs the scopes required by the operations you allow — at minimum `repo`. Token contents are never written to logs.

---

## Write a policy

Edit `/etc/ghbrk/policy.yaml`. Rules are evaluated top-to-bottom; the first match wins. If no rule matches, the request is denied.

```yaml
rules:
  # alice can push feature branches and open PRs in acme/platform
  - user: alice
    org: acme
    repo: platform
    operations: [push]
    branches: ["feature/*"]
    effect: allow

  - user: alice
    org: acme
    repo: platform
    operations: [pr_open, pr_comment]
    effect: allow
```

Reload after editing:

```bash
sudo systemctl restart ghbrk
```

See [Policy Reference](./policy.md) for the full rule syntax, all available operations, and branch matching rules.

---

## Verify

```bash
ghbrk doctor
```

`doctor` checks daemon reachability, credential presence and file-permission validity, and policy-file parsability. It prints one status line per check and exits zero only when all pass.

---

## Build from source

```bash
git clone https://github.com/marconae/ghbrk
cd ghbrk
cargo build --release
sudo ./deploy/linux/install.sh
```

**Requirements:** Rust stable toolchain, `git` and `gh` at standard system paths, Linux with systemd.

```bash
# Run tests
cargo test

# Lint
cargo clippy --all-targets --all-features -- -D warnings

# Format check
cargo fmt --check

# License and advisory audit
cargo deny check
```

# Mission: ghbrk

> A policy-enforcing proxy that lets AI coding agents perform Git/GitHub operations using a shared credential, without direct access to that credential.

## Problem Statement

AI coding agents (e.g. Claude Code running with bypass permissions) need to push code, open pull requests, and comment on issues — but granting them unrestricted access to a developer's GitHub credentials is dangerous. A rogue or compromised agent could push to any repo, close any PR, or exfiltrate SSH keys. Creating per-agent GitHub bot accounts is an operational burden. Existing solutions like `sudo` rules do not understand Git/GitHub semantics and cannot enforce per-repo or per-branch policies. `ghbrk` solves this by acting as the sole credential holder and gating every remote Git/GitHub operation through a configurable allow/deny policy.

## Target Users

| Persona | Goal | Key Workflow |
|---------|------|--------------|
| Developer running AI agents | Let agents commit and push without risking unrestricted GitHub access | Installs ghbrk, registers credentials under `/etc/ghbrk/`, configures policy; agents transparently use the shims |
| System administrator (root or designated user) | Control which repos and operations each Unix user's agents may access | Edits `/etc/ghbrk/policy.yaml` and manages credentials in `/etc/ghbrk/credentials/` |

## Core Capabilities

1. **Credential isolation** — SSH keys and GitHub tokens are stored under `/etc/ghbrk/credentials/<username>/`, owned by the `ghbrk` system user with mode `0600`. Agent processes have no filesystem read access to these files.
2. **Policy enforcement** — A YAML policy config defines per-org, per-repo, and per-branch allow/deny rules for each Git/GitHub operation type. First-matching rule wins; default is deny.
3. **Transparent interception** — A thin shim binary (`ghbrk`) is symlinked as `git` and `gh` early in the agent's `PATH`. Agents call these normally; the shim forwards requests to the broker daemon over a Unix socket and relays I/O back.
4. **Multi-user daemon** — A single `ghbrk-daemon` process serves all Unix users on the machine. It identifies callers via `SO_PEERCRED` and applies per-user credentials and policy.
5. **Audit logging** — Every allow and deny decision is written to a structured append-only log for accountability.

## Out of Scope

- GUI or web-based configuration interface (YAML files only)
- Remote or networked broker operation (Unix socket only — broker and agents on the same machine)
- Non-GitHub forges (GitLab, Gitea, Bitbucket, etc.)
- CI/CD runner environments (GitHub Actions, Jenkins, etc.)

## Domain Glossary

| Term | Definition |
|------|------------|
| Agent | An automated process (e.g. Claude Code) running as a Unix user, potentially with elevated OS permissions via bypass mode |
| Broker | The `ghbrk-daemon` process — a privileged system daemon that owns credentials and executes Git/GitHub operations on behalf of callers |
| Shim | The `ghbrk` binary placed in an agent's `PATH`, symlinked as `git` and `gh`; intercepts invocations and relays them to the broker via Unix socket |
| Caller | The Unix user whose agent issued a Git/GitHub command; identified by the broker via `SO_PEERCRED` |
| Policy | The YAML configuration (`/etc/ghbrk/policy.yaml`) defining which operations callers may perform on which repos and branches |
| Operation | A categorised Git/GitHub action: `push`, `fetch`, `pull`, `clone`, `pr_open`, `pr_comment`, `pr_close`, `pr_merge`, `pr_review`, `issue_open`, `issue_comment`, `issue_close`, `release_create` |

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| Language | Rust (stable) | Single static binary; memory-safe credential handling |
| Async runtime | Tokio | Concurrent Unix socket server in the daemon subcommand |
| Config parsing | serde + serde_yaml | Policy YAML deserialisation |
| CLI / subcommands | clap | `ghbrk daemon`, `ghbrk git`, `ghbrk gh`, plus argv[0] shim detection |
| Structured logging | tracing + tracing-subscriber | Async-aware logs; audit trail |
| Unix primitives | nix | `SO_PEERCRED` / `getpeereid` (cross-platform), signal handling, file permissions |
| License enforcement | cargo-deny | Blocks GPL/AGPL/non-MIT-compatible dependencies at CI time |
| Testing | cargo test + cargo llvm-cov | Unit and integration tests with coverage reporting |

**License policy:** `ghbrk` is MIT-licensed. All dependencies must be MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, or equivalent permissive. GPL, AGPL, LGPL, and SSPL dependencies are forbidden. `cargo deny check` must pass in CI.

## Commands

```bash
# Build
cargo build --release

# Test
cargo test

# Lint & Format
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check

# Code coverage
cargo llvm-cov

# License & dependency audit
cargo deny check
```

## Project Structure

```
ghbrk/
├── Cargo.toml              # single crate
├── deny.toml               # cargo-deny license + advisory config
├── CLAUDE.md               # dev rules incl. MIT-only dependency policy
├── src/
│   ├── main.rs             # clap subcommand dispatch + argv[0] shim detection
│   ├── cmd/
│   │   ├── daemon.rs       # `ghbrk daemon` — socket server, policy engine, executor
│   │   ├── git.rs          # `ghbrk git [args]` — shim client for git operations
│   │   └── gh.rs           # `ghbrk gh [args]`  — shim client for gh operations
│   ├── policy.rs           # policy config types + rule evaluation engine
│   ├── protocol.rs         # wire protocol: request/response types, framing
│   └── resolver.rs         # cwd + git remote URL → org/repo name
├── config/
│   └── policy.example.yaml # annotated example policy for users
└── deploy/
    ├── linux/
    │   ├── ghbrk.service   # systemd unit file
    │   └── install.sh      # creates ghbrk user, installs binary, sets /etc/ghbrk perms
    └── macos/
        ├── io.ghbrk.daemon.plist  # launchd plist
        └── install.sh
```

## Architecture

**Pattern:** Privilege-separated client–server over Unix domain socket. Single binary with subcommands; shim mode also triggered when `argv[0]` is `git` or `gh` (symlink).

**Binary entry points:**

- `ghbrk daemon` — starts the broker server. Runs as the `ghbrk` system user (or root). Listens on `/var/run/ghbrk/broker.sock` (mode `0660`, group `ghbrk-clients`). On each connection, reads the caller's UID via `SO_PEERCRED` (Linux) / `getpeereid` (macOS), maps it to a Unix username, loads that user's credentials from `/etc/ghbrk/credentials/<username>/`, evaluates the request against `/etc/ghbrk/policy.yaml`, then either executes the git/gh command with the stored credentials and streams back stdout/stderr, or returns a structured denial.

- `ghbrk git [args]` / `ghbrk gh [args]` — shim mode. Also activated when the binary is invoked as `git` or `gh` via symlink. Connects to the broker socket, sends a JSON request containing the tool, arguments, and working directory. Streams back output. Exits with the same code returned by the daemon. The agent's `PATH` is configured to include the directory containing the symlinks before `/usr/bin`.

**Wire protocol:** length-prefixed JSON frames over Unix stream socket.

**Data flow:**
```
Agent process
  → calls git/gh (via symlink or `ghbrk git/gh` in PATH)
  → shim connects to /var/run/ghbrk/broker.sock
  → sends: { tool, args, cwd }
  → daemon reads SO_PEERCRED/getpeereid → UID → username
  → daemon reads /etc/ghbrk/credentials/<username>/ and policy.yaml
  → evaluates: repo × operation × branch → allow | deny
  → if allowed: spawns real git/gh with stored credentials; streams stdout/stderr
  → if denied:  sends structured error; shim exits nonzero
  → audit log entry written in both cases
```

## Constraints

- **Technical:** Linux and macOS. Local machine only — Unix socket, no TCP listener. GitHub + git only in v1.
- **Security:** No GPL/AGPL/non-permissive dependencies. Credentials stored at `/etc/ghbrk/credentials/`, mode `0600`, owned by `ghbrk` user — inaccessible to any agent running as another Unix user.
- **Performance:** Correctness and safety first; no hard latency or memory targets for v1.

## External Dependencies

| Service | Purpose | Failure Impact |
|---------|---------|----------------|
| GitHub (SSH) | git push/fetch/clone authentication | Remote git operations fail with auth error; local ops unaffected |
| GitHub API (via `gh` CLI) | PR, issue, release operations | All `gh`-proxied operations fail; git operations unaffected |

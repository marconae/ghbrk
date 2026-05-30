# Mission: ghbrk

> A policy-enforcing proxy that lets AI coding agents perform Git/GitHub operations using a shared credential, without direct access to that credential.

## Problem Statement

AI coding agents (e.g. Claude Code running with bypass permissions) need to push code, open pull requests, and comment on issues — but granting them unrestricted access to a developer's GitHub credentials is dangerous. A rogue or compromised agent could push to any repo, close any PR, or exfiltrate SSH keys. Creating per-agent GitHub bot accounts is an operational burden. Existing solutions like `sudo` rules do not understand Git/GitHub semantics and cannot enforce per-repo or per-branch policies. `ghbrk` solves this by acting as the sole credential holder and gating every remote Git/GitHub operation through a configurable allow/deny policy.

## Target Users

| Persona | Goal | Key Workflow |
|---------|------|--------------|
| Developer running AI agents | Let agents commit and push without risking unrestricted GitHub access | Installs ghbrk, registers credentials under `/etc/ghbrk/`, configures policy; agents call `ghbrk git`/`ghbrk gh` explicitly for remote operations |
| System administrator (root or designated user) | Control which repos and operations each Unix user's agents may access | Edits `/etc/ghbrk/policy.yaml` and manages credentials in `/etc/ghbrk/credentials/` |

## Core Capabilities

1. **Credential isolation** — SSH keys and GitHub tokens are stored under `/etc/ghbrk/credentials/<username>/`, owned by the `ghbrk` system user with mode `0600`. Agent processes have no filesystem read access to these files.
2. **Policy enforcement** — A YAML policy config defines per-org, per-repo, and per-branch allow/deny rules for each Git/GitHub operation type. First-matching rule wins; default is deny.
3. **Explicit gateway** — Agents use plain `git`/`gh` for local and read-only operations. Remote and authenticated operations are brokered explicitly via `ghbrk git <remote-subcommand>` and `ghbrk gh <subcommand>`. There are no symlinks and no transparent interception; the privilege boundary is part of the interface, not hidden from it.
4. **Inspectable boundary** — `ghbrk doctor` checks daemon reachability, credentials, and policy health. `ghbrk explain <cmd>` performs a dry run showing what the broker would do without executing it. `ghbrk policy <org>/<repo>` lists the allowed and forbidden operations for the calling user.
5. **Multi-user daemon** — A single `ghbrk-daemon` process serves all Unix users on the machine. It identifies callers via `SO_PEERCRED` and applies per-user credentials and policy.
6. **Audit logging** — Every allow and deny decision is written to a structured append-only log for accountability.

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
| Gateway | The explicit invocation interface: `ghbrk git <remote-subcommand>` or `ghbrk gh <subcommand>`. Agents call this instead of plain `git`/`gh` when they need a network or authenticated operation. No symlinks or transparent interception. |
| Caller | The Unix user whose agent issued a `ghbrk git`/`ghbrk gh` command; identified by the broker via `SO_PEERCRED` |
| Policy | The YAML configuration (`/etc/ghbrk/policy.yaml`) defining which operations callers may perform on which repos and branches |
| Operation | A categorised Git/GitHub action: `push`, `fetch`, `pull`, `clone`, `pr_open`, `pr_comment`, `pr_close`, `pr_merge`, `pr_review`, `issue_open`, `issue_comment`, `issue_close`, `release_create` |

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| Language | Rust (stable) | Single static binary; memory-safe credential handling |
| Async runtime | Tokio | Concurrent Unix socket server in the daemon subcommand |
| Config parsing | serde + serde_yaml | Policy YAML deserialisation |
| CLI / subcommands | clap | `ghbrk daemon`, `ghbrk git`, `ghbrk gh`, `ghbrk doctor`, `ghbrk explain`, `ghbrk policy` |
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
│   ├── main.rs             # clap subcommand dispatch (no argv[0] symlink dispatch)
│   ├── cmd/
│   │   ├── daemon.rs       # `ghbrk daemon` — socket server, policy engine, executor
│   │   ├── git.rs          # `ghbrk git [args]` — rejects local subcommands; relays remote ops to broker
│   │   ├── gh.rs           # `ghbrk gh [args]`  — relays all gh invocations to broker
│   │   ├── doctor.rs       # `ghbrk doctor` — daemon, credential, and policy health checks
│   │   ├── explain.rs      # `ghbrk explain <cmd>` — dry-run: broker resolves and evaluates without executing
│   │   └── policy.rs       # `ghbrk policy <org>/<repo>` — lists allowed/forbidden operations
│   ├── policy.rs           # policy config types + rule evaluation engine
│   ├── protocol.rs         # wire protocol: request/response types, framing
│   └── resolver.rs         # cwd + git remote URL → org/repo name (broker-side)
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

**Pattern:** Privilege-separated client–server over Unix domain socket. Single binary with clap-dispatched subcommands. No argv[0] symlink dispatch; the privilege boundary is explicit, not hidden.

**Binary entry points:**

- `ghbrk daemon` — starts the broker server. Runs as the `ghbrk` system user (or root). Listens on `/var/run/ghbrk/broker.sock` (mode `0660`, group `ghbrk-clients`). On each connection, reads the caller's UID via `SO_PEERCRED` (Linux) / `getpeereid` (macOS), maps it to a Unix username, loads that user's credentials from `/etc/ghbrk/credentials/<username>/`, evaluates the request against `/etc/ghbrk/policy.yaml`, then either executes the git/gh command with the stored credentials and streams back stdout/stderr, or returns a structured denial.

- `ghbrk git <remote-subcommand>` — explicit gateway for remote git operations (push, fetch, pull, clone). Local-only subcommands (status, commit, log, etc.) are rejected immediately with a guidance error before any socket connection is attempted. Connects to the broker socket, sends a JSON request containing the tool, arguments, and working directory. Streams back output. Exits with the same code returned by the daemon.

- `ghbrk gh <subcommand>` — explicit gateway for all gh operations. Every invocation is relayed to the broker for credential injection and policy evaluation.

- `ghbrk doctor` — checks daemon reachability, stored credentials, and policy-file validity. Prints one status line per check; exits zero only when all checks pass.

- `ghbrk explain <cmd> [args]` — dry run: sends a `Tool::Explain` request to the broker, which resolves the operation and evaluates policy without executing it. Reports the would-be decision and which credential would be injected.

- `ghbrk policy <org>/<repo>` — lists which operations the calling user is allowed or forbidden to perform on the specified repository, based on the current policy file.

Agents use plain `git`/`gh` directly for all local and read-only operations. No symlinks are created; the distinction between local work and brokered remote work is the command the agent types.

**Wire protocol:** length-prefixed JSON frames over Unix stream socket.

**Data flow:**
```
Agent process
  → local operation: calls plain git/gh directly (no broker contact)
  → remote operation: calls ghbrk git <remote-sub> / ghbrk gh <sub>
      → ghbrk connects to /var/run/ghbrk/broker.sock
      → sends: { tool, args, cwd }
      → daemon reads SO_PEERCRED/getpeereid → UID → username
      → daemon reads /etc/ghbrk/credentials/<username>/ and policy.yaml
      → evaluates: repo × operation × branch → allow | deny
      → if allowed: spawns real git/gh with stored credentials; streams stdout/stderr
      → if denied:  sends structured error; ghbrk exits nonzero
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

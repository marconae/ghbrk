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

1. **Credential isolation** — SSH keys and GitHub tokens are stored under `/etc/ghbrk/credentials/<username>/`, owned by the `ghbrk` system user with mode `0600`. Agent processes have no filesystem read access to these files. For SSH operations, the daemon loads the key into a per-operation `ssh-agent` it owns; the child process receives only a proxy `SSH_AUTH_SOCK` (never the agent socket or the key bytes directly) and the agent's lifetime is capped. Child `git`/`gh` processes run with the requesting user's UID/GID/supplementary groups (privilege dropped per operation), not the daemon's own identity.
2. **Policy enforcement** — A YAML policy config defines per-org, per-repo, and per-branch allow/deny rules for each Git/GitHub operation type. First-matching rule wins; default is deny. Rules can reference a role name instead of an inline operation list: built-in tiers `read-only` ⊂ `write` ⊂ `maintain` ⊂ `admin` are available without declaration, and operators can declare custom roles or narrow a built-in in the policy file. `ghbrk allow <org>/<repo> --ops ...|--role ...` (root-only) appends a rule and hot-reloads the running daemon without a restart.
3. **Explicit gateway** — Agents use plain `git`/`gh` for local and read-only operations. Remote and authenticated operations are brokered explicitly via `ghbrk git <remote-subcommand>` and `ghbrk gh <subcommand>`. There are no symlinks and no transparent interception; the privilege boundary is part of the interface, not hidden from it.
4. **Inspectable boundary** — `ghbrk doctor` checks daemon reachability, credentials, and policy health, including ownership/mode audits of the policy file, config directory, credential directories, and the socket (write-path exposure fails the check, read-path exposure warns). `ghbrk explain <cmd>` performs a dry run showing what the broker would do without executing it. `ghbrk policy <org>/<repo>` lists the allowed and forbidden operations for the calling user.
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
| Operation | A categorised Git/GitHub action. 21 total: `push`, `fetch`, `pull`, `clone`; PR ops `pr_open`, `pr_comment`, `pr_close`, `pr_merge`, `pr_review`; issue ops `issue_open`, `issue_comment`, `issue_close`; release-lifecycle ops `release_create`, `release_delete`, `release_edit`, `release_upload`, `release_delete_asset`, `release_list`, `release_view`, `release_download`; and `gh_api_read` |
| Role | A named, reusable operation set a policy rule can reference instead of an inline list. Built-in tiers `read-only` ⊂ `write` ⊂ `maintain` ⊂ `admin` exist without declaration; a user-defined role with the same name overrides the built-in |

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| Language | Rust (stable) | Single static binary; memory-safe credential handling |
| Async runtime | Tokio | Concurrent Unix socket server in the daemon subcommand |
| Config parsing | serde + serde_yaml | Policy YAML deserialisation |
| CLI / subcommands | clap | `ghbrk daemon`, `ghbrk git`, `ghbrk gh`, `ghbrk doctor`, `ghbrk explain`, `ghbrk policy`, `ghbrk allow` |
| Structured logging | tracing + tracing-subscriber | Async-aware logs; audit trail |
| Unix primitives | nix | `SO_PEERCRED`, privilege-drop syscalls (`setuid`/`setgid`/`setgroups`), signal handling, file permissions |
| License enforcement | cargo-deny | Blocks GPL/AGPL/non-MIT-compatible dependencies at CI time |
| Testing | cargo test + a Docker Compose end-to-end harness | Unit tests (`cargo test --lib`) plus a real SSH git server, a mock GitHub API over TLS, and an unprivileged user fixture for integration/e2e coverage |

**License policy:** `ghbrk` is MIT-licensed. All dependencies must be MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, or equivalent permissive. GPL, AGPL, LGPL, and SSPL dependencies are forbidden. `cargo deny check` must pass in CI.

## Commands

```bash
# Build
cargo build --release

# Unit tests
cargo test --lib

# Integration/e2e tests (Docker required, serial)
cargo test --tests -- --test-threads=1

# Lint & Format
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check

# License & dependency audit
cargo deny check
```

## Project Structure

```
ghbrk/
├── Cargo.toml              # single crate
├── deny.toml               # cargo-deny license + advisory config
├── CLAUDE.md               # dev rules incl. MIT-only dependency policy
├── install.sh              # public one-line curl installer (downloads a tagged release binary)
├── src/
│   ├── main.rs             # clap subcommand dispatch (no argv[0] symlink dispatch)
│   ├── broker.rs           # daemon accept loop, request routing, allow-mutation handling
│   ├── policy.rs           # policy config types, roles, and rule evaluation engine
│   ├── resolver.rs         # cwd + git remote URL → org/repo/branch (broker-side, git/gh/release variants)
│   ├── credentials.rs      # credential loading, SSH agent escrow, GH_TOKEN/HTTPS env injection
│   ├── executor.rs         # privilege-dropped child spawn and I/O streaming
│   ├── audit.rs            # append-only audit log writer
│   ├── health_check.rs     # `doctor`'s permission/reachability checks
│   ├── protocol.rs         # wire protocol: request/response types, framing
│   └── cmd/
│       ├── daemon.rs       # `ghbrk daemon` — starts the broker server
│       ├── git.rs          # `ghbrk git [args]` — rejects local subcommands; relays remote ops to broker
│       ├── gh.rs           # `ghbrk gh [args]`  — relays all gh invocations to broker
│       ├── gateway.rs      # shared gateway plumbing for git.rs/gh.rs
│       ├── doctor.rs       # `ghbrk doctor` — daemon, credential, and policy health checks
│       ├── explain.rs      # `ghbrk explain <cmd>` — dry-run: broker resolves and evaluates without executing
│       ├── policy.rs       # `ghbrk policy <org>/<repo>` — lists allowed/forbidden operations
│       └── allow.rs        # `ghbrk allow <org>/<repo> --ops|--role` — root-only, mutates the policy file
├── config/
│   └── policy.example.yaml # annotated example policy for users
├── tests/integration/      # Docker Compose e2e harness: SSH git server, mock GitHub API over TLS, unprivileged user fixture
└── deploy/linux/
    ├── ghbrk.service        # systemd unit file
    ├── ghbrk.tmpfiles       # tmpfiles.d snippet creating /run/ghbrk at boot
    ├── install.sh           # local dev installer: builds from source, installs unit/tmpfiles/policy
    ├── install-credentials.sh # provisions a single user's SSH key/token under /etc/ghbrk/credentials/
    └── provision-user.sh    # creates a Unix user and its credential directory
```

Two installers exist for different audiences: the root `install.sh` is the public one-line curl installer (downloads a tagged release binary); `deploy/linux/install.sh` is the local/dev installer (builds from source). Both write the same systemd unit and tmpfiles snippet.

## Architecture

**Pattern:** Privilege-separated client–server over Unix domain socket. Single binary with clap-dispatched subcommands. No argv[0] symlink dispatch; the privilege boundary is explicit, not hidden.

**Binary entry points:**

- `ghbrk daemon` — starts the broker server. Runs as the `ghbrk` system user, whose primary group is `ghbrk-clients` (Linux only; the socket group derives from this rather than a runtime `chown`). Listens on `/run/ghbrk/broker.sock` (mode `0660`, group `ghbrk-clients`; the directory is recreated on every boot by a `tmpfiles.d` snippet). On each connection, reads the caller's UID via `SO_PEERCRED`, maps it to a Unix username, loads that user's credentials from `/etc/ghbrk/credentials/<username>/`, evaluates the request against `/etc/ghbrk/policy.yaml`, then either executes the git/gh command with the stored credentials and streams back stdout/stderr, or returns a structured denial. Child processes are spawned with the requesting user's UID/GID/supplementary groups (privilege dropped in the child; the daemon's own identity never changes).

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
      → gateway computes remote-URL and HEAD-branch hints from the caller's
        own working directory (worktree- and submodule-aware) and includes
        them on the request
      → ghbrk connects to /run/ghbrk/broker.sock
      → sends: { tool, args, cwd, remote_url?, head_branch? }
      → daemon reads SO_PEERCRED → UID → username
      → daemon reads /etc/ghbrk/credentials/<username>/ and policy.yaml
      → resolves org/repo/branch: uses the client's hint when present,
        otherwise falls back to its own filesystem discovery from cwd
      → evaluates: repo × operation × branch → allow | deny
      → if allowed: spawns real git/gh with stored credentials (privilege
        dropped to the caller's UID/GID); streams stdout/stderr
      → if denied:  sends structured error; ghbrk exits nonzero
      → audit log entry written in both cases
```

## Constraints

- **Technical:** Linux only (peer identity relies on `SO_PEERCRED`). Local machine only — Unix socket, no TCP listener. GitHub + git only.
- **Security:** No GPL/AGPL/non-permissive dependencies. Credentials stored at `/etc/ghbrk/credentials/`, mode `0600`, owned by `ghbrk` user — inaccessible to any agent running as another Unix user.
- **Performance:** Correctness and safety first; no hard latency or memory targets.

## External Dependencies

| Service | Purpose | Failure Impact |
|---------|---------|----------------|
| GitHub (SSH) | git push/fetch/clone authentication | Remote git operations fail with auth error; local ops unaffected |
| GitHub API (via `gh` CLI) | PR, issue, release operations | All `gh`-proxied operations fail; git operations unaffected |

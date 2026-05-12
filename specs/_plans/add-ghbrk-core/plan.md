# Plan: add-ghbrk-core

## Summary

Bootstraps the entire first working state of `ghbrk` — a privilege-separated Unix daemon that proxies `git` and `gh` CLI operations through a YAML-driven policy engine, with credential isolation, real-time output streaming, audit logging, systemd deployment artefacts, and a Docker-based end-to-end integration harness.

## Design

### Context

ghbrk must let AI agents run unmodified `git` and `gh` invocations while a daemon owned by a separate Unix user holds the only copy of the SSH key and GitHub token. Without this separation, any compromise of an agent equals full credential compromise. The challenge is to do this transparently — the agent should see normal stdio, normal exit codes, and live progress output — while the daemon retains full mediation over which org, repo, branch, and operation are permitted.

- **Goals**
  - Single static Rust binary serving three roles via subcommands and argv[0] detection: daemon, git shim, gh shim
  - Strong credential isolation via filesystem permissions and a non-agent system user
  - First-match-wins YAML policy with org/repo/branch/operation granularity
  - Real-time bidirectional streaming so progress bars and large clones work
  - End-to-end automated test of push using a Docker-hosted SSH git server
- **Non-Goals**
  - macOS support (deferred; Linux SO_PEERCRED only in v1)
  - Networked broker over TCP
  - Non-GitHub forges
  - Automated end-to-end test against the real GitHub API (manual smoke test only in v1)

### Decision

A single Rust crate is dispatched at startup. argv[0] inspection short-circuits to shim mode when basename is `git` or `gh`; otherwise clap parses subcommands. The shim and daemon converse over a Unix stream socket using length-prefixed JSON frames. Each accepted connection inside the daemon is its own Tokio task: it reads SO_PEERCRED, resolves the username, parses the request through the resolver, evaluates against the policy engine, and on allow spawns the real binary with credential env vars set, streaming stdout and stderr back as separate frame types until the child exits. On deny, a structured `Denied` frame is sent and the connection closes. Every decision is appended to an audit log.

#### Architecture

```
+--------+        +------ ghbrk binary (one artefact) ------+
| agent  |        |                                         |
| (cwd)  |--git-->|  argv[0]=git/gh ──> shim-client         |
|        |--gh--->|     │                                   |
+--------+        |     │  Unix socket: length-prefixed JSON|
                  |     ▼                                   |
                  |  /var/run/ghbrk/broker.sock             |
                  |     │                                   |
                  |     ▼                                   |
                  |  broker-server (Tokio task per conn)    |
                  |     │                                   |
                  |     ├── SO_PEERCRED -> username          |
                  |     ├── resolver: tool/args/cwd -> op    |
                  |     ├── policy-engine: rules -> decision |
                  |     ├── audit-log: append JSON line      |
                  |     ├── credential-injection: env vars   |
                  |     └── executor-streaming: spawn child  |
                  |             stdout -> StdoutChunk frames |
                  |             stderr -> StderrChunk frames |
                  |             exit   -> Exit frame         |
                  +-----------------------------------------+
```

#### Patterns

| Pattern | Where | Why |
|---------|-------|-----|
| Single-binary multiplexing | `main.rs` argv[0] check + clap | One artefact installs as binary + symlinks; simpler packaging than three crates |
| Length-prefixed framing | `protocol.rs` | Predictable parse boundaries; trivial to implement; cheaper than a full RPC framework |
| Task-per-connection (Tokio) | `broker-server` | Concurrent shims, no head-of-line blocking, idiomatic async I/O |
| First-match-wins rules | `policy-engine` | Operator intuition matches firewall mental model; explicit ordering |
| Default deny | `policy-engine` | Fail-closed posture for a security tool |
| Env-var credential passing | `credential-injection` | Avoids leaking secrets via argv; uses git's documented askpass / GIT_SSH_COMMAND points |

### Consequences

| Decision | Alternatives Considered | Rationale |
|----------|------------------------|-----------|
| Single binary, argv[0] shim detection | Three separate binaries; one binary with always-explicit subcommand | argv[0] detection lets the agent's PATH point at unmodified `git`/`gh` symlinks — invisible to tooling that hard-codes the names |
| Length-prefixed JSON over Unix socket | gRPC, capnp, raw line-delimited | JSON is debuggable, length-prefix is trivial, avoids pulling a heavy RPC dep into a security-sensitive binary |
| YAML policy with first-match | OPA/Rego, custom DSL | YAML is the lowest-friction format for sysadmins; first-match maps to firewall mental model |
| SO_PEERCRED only (Linux) | getpeereid for macOS in v1 | Mission already deferred macOS; reduces surface for v1 |
| Tokio for async runtime | std::thread per connection | Streaming child stdout/stderr concurrently with frame sends is straightforward in async; std threads would need extra plumbing |
| Token via env vars / GIT_ASKPASS | Pass on stdin to a credential helper | env-var path is the documented git/gh integration; stdin-based helpers add complexity for marginal gain in v1 |

## Features

| Feature | Status | Spec |
|---------|--------|------|
| ghbrk/cli-dispatch | NEW | `ghbrk/cli-dispatch/spec.md` |
| ghbrk/wire-protocol | NEW | `ghbrk/wire-protocol/spec.md` |
| ghbrk/shim-client | NEW | `ghbrk/shim-client/spec.md` |
| ghbrk/broker-server | NEW | `ghbrk/broker-server/spec.md` |
| ghbrk/policy-engine | NEW | `ghbrk/policy-engine/spec.md` |
| ghbrk/resolver | NEW | `ghbrk/resolver/spec.md` |
| ghbrk/credential-injection | NEW | `ghbrk/credential-injection/spec.md` |
| ghbrk/executor-streaming | NEW | `ghbrk/executor-streaming/spec.md` |
| ghbrk/audit-log | NEW | `ghbrk/audit-log/spec.md` |
| ghbrk/deployment | NEW | `ghbrk/deployment/spec.md` |
| ghbrk/integration-harness | NEW | `ghbrk/integration-harness/spec.md` |

## Dependencies

External crates (all permissive: MIT / Apache-2.0 / BSD / ISC):

- `tokio` — async runtime, Unix socket, child process, signal handling
- `serde`, `serde_json`, `serde_yaml` — JSON wire frames + YAML policy
- `clap` — CLI parsing
- `tracing`, `tracing-subscriber` — structured logging + audit
- `nix` — SO_PEERCRED, file mode introspection, signals
- `thiserror` / `anyhow` — error ergonomics
- `glob` — branch glob matching
- `time` or `chrono` — RFC 3339 timestamps for audit log

External tools:

- `cargo-deny` — license + advisory enforcement
- Docker + docker compose — integration harness
- A bare-git SSH container image (e.g. `linuxserver/openssh-server` or a custom Debian + openssh-server + git image)

## Implementation Tasks

Tasks are numbered for cross-reference. Tags:

- `[expert]` — requires deep reasoning (concurrency, security boundaries, novel correctness)

### 1. Project bootstrap

- [ ] 1.1 Initialise `Cargo.toml` for a single binary crate named `ghbrk`, edition 2021, MIT license metadata
- [ ] 1.2 Add `deny.toml` denying GPL/AGPL/LGPL/SSPL and allowing MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-DFS-2016
- [ ] 1.3 Add core dependencies (tokio, serde, serde_json, serde_yaml, clap, tracing, tracing-subscriber, nix, thiserror, glob, time)
- [ ] 1.4 Create the source skeleton `src/{main.rs, cmd/{daemon,git,gh}.rs, policy.rs, protocol.rs, resolver.rs}` with empty stubs that compile

### 2. Wire protocol

- [ ] 2.1 Define request/response enums in `protocol.rs` (`Request`, `StdoutChunk`, `StderrChunk`, `Exit`, `Denied`) with serde derives
- [ ] 2.2 Implement length-prefixed frame encoder (4-byte BE length + JSON body) [expert]
- [ ] 2.3 Implement length-prefixed frame decoder with 16 MiB ceiling and truncation detection [expert]
- [ ] 2.4 Add unit tests for round-tripping each variant and protocol-error edge cases

### 3. CLI dispatch

- [ ] 3.1 Implement argv[0] basename detection in `main.rs` (recognise `git` and `gh`)
- [ ] 3.2 Wire up clap with `daemon`, `git`, `gh` subcommands; route to `cmd::*` modules
- [ ] 3.3 Integration test: invoke binary with `--help`, with `daemon`, with `git status`, and via a temp-dir symlink named `git`

### 4. Shim client

- [ ] 4.1 Implement `cmd/git.rs` and `cmd/gh.rs` shared shim entry that connects to the broker socket
- [ ] 4.2 Send `Request { tool, args, cwd }` frame using the wire protocol
- [ ] 4.3 Loop reading frames; route `StdoutChunk`/`StderrChunk` to the process's own stdio in real time [expert]
- [ ] 4.4 Handle `Denied` and `Exit` frames; propagate exit code; print denial reason to stderr
- [ ] 4.5 Handle missing socket / connect refused with clear error message

### 5. Policy engine

- [ ] 5.1 Define `Policy`, `Rule`, `Effect`, `Operation` types in `policy.rs` with serde
- [ ] 5.2 Implement YAML loader with strict validation: known operations, non-empty operations list, reject unknowns
- [ ] 5.3 Implement rule evaluation: first-match-wins, default deny, branch glob matching [expert]
- [ ] 5.4 Unit tests for: exact match, wildcard user, branch glob, no-match deny, ordering precedence, unknown-op rejection

### 6. Resolver

- [ ] 6.1 Implement remote URL parser recognising `git@github.com:org/repo(.git)?`, `ssh://git@github.com/org/repo`, `https://github.com/org/repo(.git)?`
- [ ] 6.2 Implement git-config remote lookup walking up from cwd to find `.git/config`
- [ ] 6.3 Implement git argv → operation classification (push/fetch/clone) plus branch detection from refspec or HEAD
- [ ] 6.4 Implement gh argv → operation classification across pr/issue/release subcommands; honour `-R org/repo` flag
- [ ] 6.5 Reject non-GitHub hosts with a structured error; reject git push outside a repo
- [ ] 6.6 Unit tests covering each scenario in the resolver spec

### 7. Credential injection

- [ ] 7.1 Implement credential file lookup at `/etc/ghbrk/credentials/<user>/{id_rsa,token}`
- [ ] 7.2 Verify mode `0600` on each file via `nix::sys::stat`; refuse permissive modes [expert]
- [ ] 7.3 Build SSH env: `GIT_SSH_COMMAND="ssh -i <path> -o StrictHostKeyChecking=accept-new"`
- [ ] 7.4 Build HTTPS env for git: token-based credential helper or `GIT_ASKPASS` script wrapping the token
- [ ] 7.5 Build gh env: `GH_TOKEN=<token contents>`
- [ ] 7.6 Ensure tracing never logs token contents (write a test asserting absence in captured tracing output)

### 8. Broker server

- [ ] 8.1 Bind Unix socket at configured path; chmod 0660; chgrp `ghbrk-clients` if present
- [ ] 8.2 Read peer UID via SO_PEERCRED; map UID to username via `nix::unistd::User::from_uid` [expert]
- [ ] 8.3 Per-connection Tokio task: read request frame, run resolver, run policy engine, write audit record, dispatch to executor or send `Denied` [expert]
- [ ] 8.4 Reject malformed frames per-connection without crashing the daemon
- [ ] 8.5 SIGTERM/SIGINT handling: stop accepting new connections, flush audit log, remove socket file, exit zero
- [ ] 8.6 Unit and integration tests for socket permissions, UID resolution, malformed-frame survival, signal shutdown

### 9. Executor streaming

- [ ] 9.1 Spawn child via `tokio::process::Command` with cwd set, stdin closed, stdout and stderr piped
- [ ] 9.2 Concurrent reader tasks for stdout and stderr emitting `StdoutChunk` / `StderrChunk` frames as bytes arrive [expert]
- [ ] 9.3 Wait for child, emit `Exit { code }` as the final frame
- [ ] 9.4 On spawn failure (e.g. missing binary) emit `Denied` with reason; do not crash daemon
- [ ] 9.5 Bound buffer sizes per read so 100 MiB output streams without unbounded memory growth [expert]
- [ ] 9.6 Integration test asserting interleaving order is preserved (stdout, stderr, stdout)

### 10. Audit log

- [ ] 10.1 Define `AuditRecord` struct with serde JSON serialisation
- [ ] 10.2 Append-only writer keyed off configured path (default `/var/log/ghbrk/audit.log`); buffered with periodic flush
- [ ] 10.3 Hook into the broker-server decision path so every allow and deny writes one record
- [ ] 10.4 Flush on SIGTERM before socket removal and exit
- [ ] 10.5 Test asserting token contents never appear in the audit file

### 11. Deployment

- [ ] 11.1 Author `deploy/linux/ghbrk.service` with `User=ghbrk`, `Group=ghbrk`, `ProtectSystem=strict`, `NoNewPrivileges=true`, `PrivateTmp=true`, `ExecStart=/usr/local/bin/ghbrk daemon`
- [ ] 11.2 Author `deploy/linux/install.sh`: create user/group, install binary, create `/etc/ghbrk`, `/etc/ghbrk/credentials`, `/var/run/ghbrk`, `/var/log/ghbrk` with correct modes; idempotent
- [ ] 11.3 Author `config/policy.example.yaml` exercising all rule fields and at least one allow + one deny
- [ ] 11.4 Verify `cargo deny check` passes against the project's actual dependency tree
- [ ] 11.5 Add a CI-style test that loads `policy.example.yaml` through the engine

### 12. Integration harness

- [ ] 12.1 Build `tests/integration/docker-compose.yml` with an SSH-accessible bare git container
- [ ] 12.2 Test helper that generates an SSH keypair per run, injects pubkey into the container's authorized_keys, places privkey under a temp `credentials/<user>/id_rsa`
- [ ] 12.3 Test helper that starts a daemon in a temp directory with a temp socket and temp policy
- [ ] 12.4 End-to-end test: shim invokes `git clone` against the harness URL, succeeds, working tree exists [expert]
- [ ] 12.5 End-to-end test: shim invokes `git push` against the harness URL, allowed by policy, refs/heads/main updates [expert]
- [ ] 12.6 End-to-end test: shim invokes `git push`, denied by policy, exit non-zero, refs unchanged, deny audit record present
- [ ] 12.7 Teardown helper that brings the compose project down between tests

## Parallelization

| Parallel Group | Tasks |
|----------------|-------|
| A — foundations | 1.* (bootstrap) |
| B — protocol + policy + resolver | 2.*, 5.*, 6.* |
| C — CLI dispatch | 3.* |
| D — shim client | 4.* (depends on 2.*) |
| E — credential + audit | 7.*, 10.* |
| F — broker + executor | 8.*, 9.* (depend on 2, 5, 6, 7, 10) |
| G — deployment | 11.* (depends on 5.* for policy example, 8.* for service) |
| H — integration harness | 12.* (depends on 4, 7, 8, 9, 10) |

Sequential dependencies:

- A → all others
- B can run in parallel with C
- D depends on B (specifically 2.*)
- E can run in parallel with B, C, D
- F depends on B, E
- G depends on B, F
- H depends on D, E, F

## Dead Code Removal

| Type | Location | Reason |
|------|----------|--------|
| (none) | — | Greenfield repository; no prior code to remove |

## Verification

### Scenario Coverage

| Scenario | Test Type | Test Location | Test Name |
|----------|-----------|---------------|-----------|
| cli-dispatch / Binary invoked as ghbrk daemon | Integration | `tests/cli_dispatch.rs` | `ghbrk_daemon_subcommand_starts_daemon` |
| cli-dispatch / Binary invoked as ghbrk git | Integration | `tests/cli_dispatch.rs` | `ghbrk_git_subcommand_enters_shim` |
| cli-dispatch / Binary invoked as ghbrk gh | Integration | `tests/cli_dispatch.rs` | `ghbrk_gh_subcommand_enters_shim` |
| cli-dispatch / Symlink named git activates shim mode | Integration | `tests/cli_dispatch.rs` | `argv0_git_symlink_enters_shim` |
| cli-dispatch / Symlink named gh activates shim mode | Integration | `tests/cli_dispatch.rs` | `argv0_gh_symlink_enters_shim` |
| cli-dispatch / Unknown subcommand exits with usage error | Integration | `tests/cli_dispatch.rs` | `unknown_subcommand_exits_nonzero` |
| cli-dispatch / Help flag shows subcommand list | Integration | `tests/cli_dispatch.rs` | `help_flag_lists_subcommands` |
| wire-protocol / Request frame round-trips | Unit | `src/protocol.rs` | `request_round_trip` |
| wire-protocol / Stdout chunk frame carries streaming output | Unit | `src/protocol.rs` | `stdout_chunk_decodes_bytes` |
| wire-protocol / Stderr chunk frame is distinguishable | Unit | `src/protocol.rs` | `stderr_chunk_distinct_from_stdout` |
| wire-protocol / Exit frame terminates the response stream | Unit | `src/protocol.rs` | `exit_frame_terminates_stream` |
| wire-protocol / Denial frame carries structured error | Unit | `src/protocol.rs` | `denied_frame_carries_reason` |
| wire-protocol / Frame exceeding 16 MiB rejected | Unit | `src/protocol.rs` | `oversize_length_rejected` |
| wire-protocol / Truncated frame body returns parse error | Unit | `src/protocol.rs` | `truncated_body_parse_error` |
| shim-client / Shim relays git push and forwards exit code | Integration | `tests/shim_client.rs` | `shim_relays_git_push_exit_code` |
| shim-client / Shim streams stdout in real time | Integration | `tests/shim_client.rs` | `shim_streams_stdout_realtime` |
| shim-client / Shim streams stderr in real time | Integration | `tests/shim_client.rs` | `shim_streams_stderr_realtime` |
| shim-client / Shim reports denial to stderr and exits non-zero | Integration | `tests/shim_client.rs` | `shim_reports_denial` |
| shim-client / Shim reports broker socket missing | Integration | `tests/shim_client.rs` | `shim_reports_missing_broker` |
| shim-client / Shim sends current working directory | Integration | `tests/shim_client.rs` | `shim_sends_cwd` |
| broker-server / Daemon binds socket with correct permissions | Integration | `tests/broker_server.rs` | `daemon_binds_socket_with_mode_0660` |
| broker-server / Daemon refuses to start when socket has active listener | Integration | `tests/broker_server.rs` | `daemon_refuses_when_socket_in_use` |
| broker-server / Daemon resolves caller UID via SO_PEERCRED | Integration | `tests/broker_server.rs` | `daemon_resolves_uid_via_peercred` |
| broker-server / Daemon rejects request when caller UID has no Unix user | Integration | `tests/broker_server.rs` | `daemon_rejects_unknown_uid` |
| broker-server / Daemon handles multiple concurrent connections | Integration | `tests/broker_server.rs` | `daemon_handles_concurrent_connections` |
| broker-server / Daemon survives malformed request frame | Integration | `tests/broker_server.rs` | `daemon_survives_malformed_frame` |
| broker-server / Daemon shuts down cleanly on SIGTERM | Integration | `tests/broker_server.rs` | `daemon_shuts_down_on_sigterm` |
| policy-engine / Allow rule matches exact user repo and operation | Unit | `src/policy.rs` | `allow_exact_match` |
| policy-engine / Default deny when no rule matches | Unit | `src/policy.rs` | `default_deny_when_no_rule_matches` |
| policy-engine / First-matching rule wins over later rules | Unit | `src/policy.rs` | `first_match_wins` |
| policy-engine / Wildcard user matches any caller | Unit | `src/policy.rs` | `wildcard_user_matches_anyone` |
| policy-engine / Branch glob release wildcard matches release/v1 | Unit | `src/policy.rs` | `branch_glob_release_wildcard` |
| policy-engine / Operation not in rule's operations list does not match | Unit | `src/policy.rs` | `operation_mismatch_no_match` |
| policy-engine / Policy with unknown operation name fails to load | Unit | `src/policy.rs` | `unknown_operation_rejected` |
| policy-engine / Policy with empty operations list fails to load | Unit | `src/policy.rs` | `empty_operations_rejected` |
| policy-engine / Operations without a branch concept ignore branch field | Unit | `src/policy.rs` | `branchless_operation_ignores_branch` |
| resolver / Resolve git push to push operation | Integration | `tests/resolver.rs` | `resolve_git_push` |
| resolver / Resolve git clone with explicit URL | Integration | `tests/resolver.rs` | `resolve_git_clone_explicit_url` |
| resolver / Resolve git fetch in existing repo | Integration | `tests/resolver.rs` | `resolve_git_fetch` |
| resolver / Resolve gh pr create using cwd repo | Integration | `tests/resolver.rs` | `resolve_gh_pr_create_cwd` |
| resolver / Resolve gh pr create with explicit -R flag | Integration | `tests/resolver.rs` | `resolve_gh_pr_create_repo_flag` |
| resolver / Resolve gh issue close | Integration | `tests/resolver.rs` | `resolve_gh_issue_close` |
| resolver / Reject non-GitHub remote URL | Integration | `tests/resolver.rs` | `reject_non_github_url` |
| resolver / Reject git command outside any repo when remote is needed | Integration | `tests/resolver.rs` | `reject_git_outside_repo` |
| resolver / Unknown git subcommand maps to a sentinel | Integration | `tests/resolver.rs` | `unknown_git_subcommand_denied` |
| credential-injection / SSH URL selects SSH key injection | Integration | `tests/credentials.rs` | `ssh_url_selects_ssh_command` |
| credential-injection / HTTPS URL selects token injection for git | Integration | `tests/credentials.rs` | `https_url_selects_token_helper` |
| credential-injection / gh CLI receives GH_TOKEN | Integration | `tests/credentials.rs` | `gh_receives_gh_token_env` |
| credential-injection / SSH URL with missing key returns explicit error | Integration | `tests/credentials.rs` | `ssh_missing_key_errors` |
| credential-injection / HTTPS URL with missing token returns explicit error | Integration | `tests/credentials.rs` | `https_missing_token_errors` |
| credential-injection / Credential file with permissive mode is rejected | Integration | `tests/credentials.rs` | `permissive_mode_rejected` |
| credential-injection / Token contents are not logged | Integration | `tests/credentials.rs` | `token_never_logged` |
| executor-streaming / Child stdout streamed as StdoutChunk frames | Integration | `tests/executor.rs` | `stdout_streams_in_chunks` |
| executor-streaming / Child stderr streamed as StderrChunk frames | Integration | `tests/executor.rs` | `stderr_streams_in_chunks` |
| executor-streaming / Child exit code propagated in Exit frame | Integration | `tests/executor.rs` | `exit_code_propagated` |
| executor-streaming / Child cwd matches request cwd | Integration | `tests/executor.rs` | `child_cwd_matches_request` |
| executor-streaming / Stdout and stderr interleaved in arrival order | Integration | `tests/executor.rs` | `stdout_stderr_interleaving_preserved` |
| executor-streaming / Killed child reports non-zero exit | Integration | `tests/executor.rs` | `killed_child_nonzero_exit` |
| executor-streaming / Failure to spawn child reports denial-style error | Integration | `tests/executor.rs` | `spawn_failure_emits_denied` |
| executor-streaming / Large output stream does not exhaust memory | Integration | `tests/executor.rs` | `large_output_bounded_memory` |
| audit-log / Allow decision produces an allow record | Integration | `tests/audit_log.rs` | `allow_decision_record` |
| audit-log / Deny decision includes reason | Integration | `tests/audit_log.rs` | `deny_decision_includes_reason` |
| audit-log / Audit record carries timestamp | Integration | `tests/audit_log.rs` | `audit_record_has_rfc3339_timestamp` |
| audit-log / Token value never appears in audit log | Integration | `tests/audit_log.rs` | `token_absent_from_audit` |
| audit-log / Audit log survives daemon restart | Integration | `tests/audit_log.rs` | `audit_log_survives_restart` |
| audit-log / Audit log flushes on SIGTERM | Integration | `tests/audit_log.rs` | `audit_log_flushes_on_sigterm` |
| deployment / systemd unit starts daemon as ghbrk user | Integration | `tests/deployment.rs` | `systemd_unit_user_group` |
| deployment / systemd unit has hardening directives | Integration | `tests/deployment.rs` | `systemd_unit_hardening_directives` |
| deployment / install.sh creates ghbrk system user | Integration | `tests/deployment.rs` | `install_creates_user_and_group` |
| deployment / install.sh creates required directories with correct modes | Integration | `tests/deployment.rs` | `install_creates_directories_with_modes` |
| deployment / install.sh is idempotent on second run | Integration | `tests/deployment.rs` | `install_idempotent` |
| deployment / Example policy YAML is loadable | Integration | `tests/deployment.rs` | `example_policy_loads` |
| deployment / cargo deny rejects a GPL dependency | Integration | `tests/deployment.rs` | `cargo_deny_rejects_gpl` |
| deployment / cargo deny passes on the real dependency tree | Integration | `tests/deployment.rs` | `cargo_deny_passes_on_real_tree` |
| integration-harness / Harness starts a reachable git SSH server | Integration | `tests/integration/harness.rs` | `harness_ssh_server_reachable` |
| integration-harness / Push through shim succeeds when policy allows | Integration | `tests/integration/harness.rs` | `e2e_push_allowed` |
| integration-harness / Push through shim is rejected when policy denies | Integration | `tests/integration/harness.rs` | `e2e_push_denied` |
| integration-harness / Clone through shim streams progress | Integration | `tests/integration/harness.rs` | `e2e_clone_succeeds` |
| integration-harness / Harness tears down cleanly | Integration | `tests/integration/harness.rs` | `harness_teardown_clean` |

### Manual Testing

| Feature | Command | Expected Output |
|---------|---------|-----------------|
| cli-dispatch | `cargo run -- --help` | Prints help listing `daemon`, `git`, `gh`; exit 0 |
| wire-protocol | `cargo test --lib protocol` | All protocol unit tests pass |
| shim-client | (with daemon running) `cargo run -- git status` in a git repo | git status output streams to terminal; exit code matches real git |
| broker-server | `cargo run -- daemon` then `ls -l /var/run/ghbrk/broker.sock` | Socket exists with mode `srw-rw----` |
| policy-engine | `cargo test --lib policy` | All policy unit tests pass |
| resolver | `cargo test --test resolver` | Resolver tests pass on synthetic git repos |
| credential-injection | Place a key under `/etc/ghbrk/credentials/$USER/id_rsa` mode 0600, run shim against an SSH URL | Push uses the key; `chmod 0644` then retry — daemon refuses with mode error |
| executor-streaming | (with daemon running) `cargo run -- git clone https://github.com/<allowed-public-repo>` | Progress lines stream live; final tree present |
| audit-log | (with daemon running) attempt one allowed and one denied operation, then `tail -n 2 /var/log/ghbrk/audit.log` | One JSON line with `decision:"allow"`, one with `decision:"deny"` and `reason` |
| deployment | `sudo deploy/linux/install.sh` on a fresh VM, then `systemctl start ghbrk && systemctl status ghbrk` | Service active; socket exists; running as `ghbrk` user |
| integration-harness | `cargo test --test harness -- --test-threads=1` | All end-to-end push/clone/deny tests pass |

### Checklist

| Step | Command | Expected |
|------|---------|----------|
| Build | `cargo build --release` | Exit 0 |
| Test | `cargo test` | 0 failures |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | 0 errors/warnings |
| Format | `cargo fmt --check` | No changes |
| Coverage | `cargo llvm-cov` | Report generated |
| License audit | `cargo deny check` | Exit 0 |

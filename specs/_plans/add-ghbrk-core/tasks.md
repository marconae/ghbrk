# Tasks: add-ghbrk-core

## Group A — Foundations
- [x] 1.1 Initialise `Cargo.toml` for a single binary crate named `ghbrk`, edition 2021, MIT license metadata
- [x] 1.2 Add `deny.toml` denying GPL/AGPL/LGPL/SSPL and allowing MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-DFS-2016
- [x] 1.3 Add core dependencies (tokio, serde, serde_json, serde_yaml, clap, tracing, tracing-subscriber, nix, thiserror, glob, time)
- [x] 1.4 Create the source skeleton `src/{main.rs, cmd/{daemon,git,gh}.rs, policy.rs, protocol.rs, resolver.rs}` with empty stubs that compile

## Group B — Protocol + Policy + Resolver
- [x] 2.1 Define request/response enums in `protocol.rs` (`Request`, `StdoutChunk`, `StderrChunk`, `Exit`, `Denied`) with serde derives
- [x] 2.2 Implement length-prefixed frame encoder (4-byte BE length + JSON body) [expert]
- [x] 2.3 Implement length-prefixed frame decoder with 16 MiB ceiling and truncation detection [expert]
- [x] 2.4 Add unit tests for round-tripping each variant and protocol-error edge cases
- [x] 5.1 Define `Policy`, `Rule`, `Effect`, `Operation` types in `policy.rs` with serde
- [x] 5.2 Implement YAML loader with strict validation: known operations, non-empty operations list, reject unknowns
- [x] 5.3 Implement rule evaluation: first-match-wins, default deny, branch glob matching [expert]
- [x] 5.4 Unit tests for: exact match, wildcard user, branch glob, no-match deny, ordering precedence, unknown-op rejection
- [x] 6.1 Implement remote URL parser recognising `git@github.com:org/repo(.git)?`, `ssh://git@github.com/org/repo`, `https://github.com/org/repo(.git)?`
- [x] 6.2 Implement git-config remote lookup walking up from cwd to find `.git/config`
- [x] 6.3 Implement git argv → operation classification (push/fetch/clone) plus branch detection from refspec or HEAD
- [x] 6.4 Implement gh argv → operation classification across pr/issue/release subcommands; honour `-R org/repo` flag
- [x] 6.5 Reject non-GitHub hosts with a structured error; reject git push outside a repo
- [x] 6.6 Unit tests covering each scenario in the resolver spec

## Group C — CLI Dispatch
- [x] 3.1 Implement argv[0] basename detection in `main.rs` (recognise `git` and `gh`)
- [x] 3.2 Wire up clap with `daemon`, `git`, `gh` subcommands; route to `cmd::*` modules
- [x] 3.3 Integration test: invoke binary with `--help`, with `daemon`, with `git status`, and via a temp-dir symlink named `git`

## Group D — Shim Client (depends on Group B 2.*)
- [x] 4.1 Implement `cmd/git.rs` and `cmd/gh.rs` shared shim entry that connects to the broker socket
- [x] 4.2 Send `Request { tool, args, cwd }` frame using the wire protocol
- [x] 4.3 Loop reading frames; route `StdoutChunk`/`StderrChunk` to the process's own stdio in real time [expert]
- [x] 4.4 Handle `Denied` and `Exit` frames; propagate exit code; print denial reason to stderr
- [x] 4.5 Handle missing socket / connect refused with clear error message

## Group E — Credential + Audit
- [x] 7.1 Implement credential file lookup at `/etc/ghbrk/credentials/<user>/{id_rsa,token}`
- [x] 7.2 Verify mode `0600` on each file via `nix::sys::stat`; refuse permissive modes [expert]
- [x] 7.3 Build SSH env: `GIT_SSH_COMMAND="ssh -i <path> -o StrictHostKeyChecking=accept-new"`
- [x] 7.4 Build HTTPS env for git: token-based credential helper or `GIT_ASKPASS` script wrapping the token
- [x] 7.5 Build gh env: `GH_TOKEN=<token contents>`
- [x] 7.6 Ensure tracing never logs token contents (write a test asserting absence in captured tracing output)
- [x] 10.1 Define `AuditRecord` struct with serde JSON serialisation
- [x] 10.2 Append-only writer keyed off configured path (default `/var/log/ghbrk/audit.log`); buffered with periodic flush
- [x] 10.3 Hook into the broker-server decision path so every allow and deny writes one record
- [x] 10.4 Flush on SIGTERM before socket removal and exit
- [x] 10.5 Test asserting token contents never appear in the audit file

## Group F — Broker + Executor (depends on Groups B, E)
- [x] 8.1 Bind Unix socket at configured path; chmod 0660; chgrp `ghbrk-clients` if present
- [x] 8.2 Read peer UID via SO_PEERCRED; map UID to username via `nix::unistd::User::from_uid` [expert]
- [x] 8.3 Per-connection Tokio task: read request frame, run resolver, run policy engine, write audit record, dispatch to executor or send `Denied` [expert]
- [x] 8.4 Reject malformed frames per-connection without crashing the daemon
- [x] 8.5 SIGTERM/SIGINT handling: stop accepting new connections, flush audit log, remove socket file, exit zero
- [x] 8.6 Unit and integration tests for socket permissions, UID resolution, malformed-frame survival, signal shutdown
- [x] 9.1 Spawn child via `tokio::process::Command` with cwd set, stdin closed, stdout and stderr piped
- [x] 9.2 Concurrent reader tasks for stdout and stderr emitting `StdoutChunk` / `StderrChunk` frames as bytes arrive [expert]
- [x] 9.3 Wait for child, emit `Exit { code }` as the final frame
- [x] 9.4 On spawn failure (e.g. missing binary) emit `Denied` with reason; do not crash daemon
- [x] 9.5 Bound buffer sizes per read so 100 MiB output streams without unbounded memory growth [expert]
- [x] 9.6 Integration test asserting interleaving order is preserved (stdout, stderr, stdout)

## Group G — Deployment (depends on Groups B policy, F broker)
- [x] 11.1 Author `deploy/linux/ghbrk.service` systemd unit
- [x] 11.2 Author `deploy/linux/install.sh`: create user/group, install binary, create dirs with correct modes; idempotent
- [x] 11.3 Author `config/policy.example.yaml` exercising all rule fields
- [x] 11.4 Verify `cargo deny check` passes against the project's actual dependency tree
- [x] 11.5 Add a CI-style test that loads `policy.example.yaml` through the engine

## Group H — Integration Harness (depends on Groups D, E, F)
- [x] 12.1 Build `tests/integration/docker-compose.yml` with an SSH-accessible bare git container
- [x] 12.2 Test helper: generate SSH keypair per run, inject pubkey, place privkey under temp credentials
- [x] 12.3 Test helper: start daemon in temp directory with temp socket and temp policy
- [x] 12.4 End-to-end test: shim invokes `git clone` against harness URL, succeeds, working tree exists [expert]
- [x] 12.5 End-to-end test: shim invokes `git push` allowed by policy, refs/heads/main updates [expert]
- [x] 12.6 End-to-end test: shim invokes `git push`, denied by policy, exit non-zero, refs unchanged, deny audit record present
- [x] 12.7 Teardown helper that brings compose project down between tests

## Phase 6: Code Review Fixes

### Fix Group I — Resolver correctness + audit async (expert)
- [x] R1.1 Fix HEAD:main refspec bug — `branch_from_refspec` must resolve the *remote* branch from `local:remote` form [expert]
- [x] R1.2 Map `gh pr comment`, `gh pr review`, `gh issue comment` subcommands to their Operation variants (or remove variants) [expert]
- [x] R1.3 Fix `parse_authority_path` URL port handling (`github.com:443` rejected as non-github) 
- [x] R1.4 Fix `AuditLogger::write` blocking runtime thread — offload via `tokio::task::spawn_blocking` [expert]

### Fix Group II — Security + executor quality (standard)
- [x] R2.1 Tighten umask before `UnixListener::bind` to prevent socket permissions race
- [x] R2.2 Pin `UserKnownHostsFile` per-user in `ssh_env` GIT_SSH_COMMAND
- [x] R2.3 Force `0o640` mode on audit log file via `OpenOptionsExt::mode`
- [x] R2.4 Drop `biased;` from `stream_pipes` to restore fair stdout/stderr interleaving
- [x] R2.5 Add `SIGNAL_EXIT_OFFSET` named constant for `128 + sig` encoding
- [x] R2.6 Consolidate `DEFAULT_SOCKET_PATH` to a single definition (currently in `cmd/daemon.rs` and `shim.rs`)
- [x] R2.7 Refactor `write_audit` 9-arg function into an `AuditEntry` struct; drop `#[allow(clippy::too_many_arguments)]`

### Fix Group III — Test quality + cleanup (standard)
- [x] R3.1 Fix `daemon_resolves_uid_via_peercred` test to actually assert SO_PEERCRED UID resolution
- [x] R3.2 Remove dead scaffolding: `_unused_imports_silence` (broker_server.rs), duplicate spawn-failure test (executor.rs), `spec_from_request` unused fn (executor.rs), `harness_starts_and_stops_cleanly` tautology
- [x] R3.3 Assert exit code `Some(1)` in cli_dispatch shim tests
- [x] R3.4 Replace raw substring audit log assertions with serde_json parsing in harness.rs
- [x] R3.5 Fix shell injection pattern in `inject_authorized_key` (use argv-based exec)
- [x] R3.6 Remove `// argv[1..] are the forwarded arguments` comment from main.rs

## Phase 5: Verification
- [x] V.1 Run `cargo build --release` — exit 0
- [x] V.2 Run `cargo test` — 0 failures (131 pass, 2 ignored)
- [x] V.3 Run `cargo clippy --all-targets --all-features -- -D warnings` — 0 errors
- [x] V.4 Run `cargo fmt --check` — no changes
- [x] V.5 Run `cargo deny check` — exit 0 (advisories ok, bans ok, licenses ok, sources ok)
- [x] V.6 Scenario coverage audit — all 83 scenarios covered

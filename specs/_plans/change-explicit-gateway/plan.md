# Plan: change-explicit-gateway

## Summary

Replace ghbrk's transparent `git`/`gh` PATH-interception shim with an explicit privilege gateway: agents use plain `git`/`gh` for local/read-only work and invoke `ghbrk git <remote-subcommand>` / `ghbrk gh <subcommand>` only for operations that leave the machine, with new `doctor`, `explain`, and `policy` subcommands making the brokered boundary inspectable.

## Design

### Context

The transparent shim symlinks `ghbrk` as `git` and `gh` early in the agent's `PATH`, so every git/gh call is silently intercepted and classified into local-passthrough vs broker-mediated. For an AI agent this makes privileged behaviour invisible: the agent cannot tell from the command alone whether it is hitting the network under brokered credentials or running locally, and the client-side classifier must perfectly mirror git/gh semantics or it silently misroutes. The redesign inverts this: privileged authority is requested explicitly through the `ghbrk` verb, and the security boundary becomes part of the interface.

- **Goals**
  - Make every brokered (machine-leaving) operation an explicit `ghbrk git`/`ghbrk gh` invocation.
  - Remove all transparent interception: no argv[0] symlink dispatch, no client-side local/remote classifier, no shim config for real-binary paths.
  - Constrain `ghbrk` strictly to remote/authenticated operations; local git subcommands return a clear guidance error.
  - Add inspectable boundary tooling: `ghbrk doctor`, `ghbrk explain <cmd>`, `ghbrk policy <org>/<repo>`.
- **Non-Goals**
  - No backward-compatibility/transparent mode and no `install-shims` subcommand.
  - No change to the credential-isolation model, policy engine semantics, or wire framing.
  - No change to what the broker executes once an operation is allowed.

### Decision

Dispatch is clap-only. `ghbrk git`/`ghbrk gh` relay to the broker for machine-leaving operations; `ghbrk git <local-subcommand>` short-circuits with a guidance error before any socket connect. The resolver (which maps `(tool, args, cwd)` to `(operation, org, repo, branch?)`) already runs inside the broker — it is relocated from the removed `shim/` domain to `daemon/resolver` with no behavioural change. The diagnostic subcommands reuse the existing socket + length-prefixed JSON framing.

#### Architecture

```
                 ┌────────────────────── ghbrk binary (clap dispatch) ──────────────────────┐
  agent ──────▶  │ git <remote>  gh <sub>   doctor    explain <cmd>    policy <org>/<repo>   │
  (plain git     │     │             │         │           │                  │              │
   for local)    │     ▼             ▼         ▼           ▼                  ▼              │
                 │  local? ──yes──▶ guidance error (no socket)                               │
                 │     │no                                                                    │
                 └─────┼──────────────┬─────────┬───────────┬──────────────────┬─────────────┘
                       ▼              ▼         ▼           ▼                  ▼
                            ┌────────────────── broker (Unix socket) ───────────────────┐
                            │ SO_PEERCRED → user → resolver → policy engine → executor   │
                            │ relay: execute + stream      query: evaluate, do NOT exec  │
                            └────────────────────────────────────────────────────────────┘
```

#### Patterns

| Pattern | Where | Why |
|---------|-------|-----|
| Explicit verb gateway | `main.rs` clap dispatch | Privileged authority is requested by name, never inferred |
| Pre-connect guardrail | `cmd/git.rs` | Local git subcommands fail fast with guidance, never reach the socket |
| Dry-run query over existing transport | `explain`, `policy` | Reuse resolver + policy without executing; no new framing |
| Broker-side resolution | `daemon/resolver` | Single authoritative mapping; client stays a thin relay |

### Consequences

| Decision | Alternatives Considered | Rationale |
|----------|------------------------|-----------|
| Remove transparent shim entirely | Keep an optional `install-shims` compat mode | User chose explicit-only; a hidden mode would re-introduce the invisible-privilege problem |
| `ghbrk` = remote-only scope; local git rejected with guidance | Let `ghbrk git status` passthrough-exec locally | Passthrough re-creates the client-side classifier and the confusing mental model; rejecting keeps the boundary crisp |
| Keep resolver in broker, relocate feature to `daemon/` | Delete resolver and have client send pre-resolved tuple | Resolver already runs broker-side (`broker.rs::resolve_request`); moving it client-side would leak parsing and weaken the trust boundary |
| `explain`/`policy` reuse `Tool` request + StdoutChunk/Exit framing | Add new server-frame variants for structured results | Streaming text lines matches existing `check` pattern; avoids protocol churn |
| Absorb `ghbrk check` into `ghbrk doctor` | Keep `check` alongside `doctor` | One health command; doctor is a superset (daemon + creds + policy) |

## Features

| Feature | Status | Spec |
|---------|--------|------|
| infra/cli-dispatch | CHANGED | `infra/cli-dispatch/spec.md` |
| infra/deployment | CHANGED | `infra/deployment/spec.md` |
| infra/health-check | REMOVED | `infra/health-check/spec.md` |
| infra/doctor | NEW | `infra/doctor/spec.md` |
| infra/explain | NEW | `infra/explain/spec.md` |
| infra/policy-query | NEW | `infra/policy-query/spec.md` |
| daemon/broker-server | CHANGED | `daemon/broker-server/spec.md` |
| daemon/wire-protocol | CHANGED | `daemon/wire-protocol/spec.md` |
| daemon/resolver | NEW | `daemon/resolver/spec.md` |
| shim/resolver | REMOVED | `shim/resolver/spec.md` |
| shim/shim-client | REMOVED | `shim/shim-client/spec.md` |
| shim/shim-config | REMOVED | `shim/shim-config/spec.md` |
| shim/shim-passthrough | REMOVED | `shim/shim-passthrough/spec.md` |

## Migration

| Current | New |
|---------|-----|
| `git`/`gh` symlinks in PATH intercept transparently | No symlinks; agents call plain `git`/`gh`, and `ghbrk git`/`ghbrk gh` explicitly for remote ops |
| `ghbrk check` | `ghbrk doctor` (superset) |
| `shim/` domain (4 features) | removed; resolver relocated to `daemon/resolver` |
| `/etc/ghbrk/config.yaml` (real_git/real_gh) | removed; no client-side real-binary path |

## Implementation Tasks

1. Remove argv[0] symlink dispatch from `src/main.rs`; make dispatch clap-only. [expert]
2. Add `Doctor`, `Explain { args }`, `Policy { repo }` clap subcommands to `src/main.rs`; remove `Check` subcommand.
3. Delete `src/shim.rs`, `src/passthrough.rs`, `src/cmd/shim.rs`, `src/config.rs` (shim config); rewire the broker-relay transport used by `cmd/git.rs`/`cmd/gh.rs` so it no longer depends on deleted modules. [expert]
4. Rewrite `src/cmd/git.rs`: reject local-only git subcommands with a guidance error before connecting; relay only remote ops to the broker. [expert]
5. Simplify `src/cmd/gh.rs` to relay all `gh` invocations to the broker (unchanged routing, minus deleted deps).
6. Add `Tool::Explain` and `Tool::Policy` discriminants to `src/protocol.rs`.
7. Implement `src/cmd/doctor.rs`: daemon-reachability check, broker-mediated credential check (reuse `Tool::Check`), and local policy-parse check; absorb `health_check.rs` logic.
8. Implement `src/cmd/explain.rs`: send `Tool::Explain` request; broker resolves + evaluates policy without executing and streams the explanation. [expert]
9. Implement broker handling for `Tool::Explain`: resolve, evaluate policy, report credential-injection intent, MUST NOT execute. [expert]
10. Implement `src/cmd/policy.rs`: send `Tool::Policy { repo }`; print allowed vs forbidden operations.
11. Implement broker handling for `Tool::Policy`: evaluate every operation in the vocabulary for the caller against the repo and stream the grouped result. [expert]
12. Add broker defence-in-depth: deny any local-only git subcommand that bypasses the gateway filter.
13. Remove `git`/`gh` symlink creation (and non-symlink-conflict guard) from `deploy/linux/install.sh`.
14. Remove obsolete tests (`tests/passthrough.rs`, `tests/shim_client.rs`, shim-config tests, `tests/check.rs`); migrate resolver tests to the daemon resolver path.
15. Write integration tests for the new scenarios (cli-dispatch, doctor, explain, policy-query, broker deny, wire-protocol round-trips).
16. Update `specs/mission.md` prose references to the transparent shim (record-time / docs follow-up).
17. Update `README.md`: replace transparent shim installation and usage docs with explicit gateway model; update architecture overview, getting-started commands, and agent integration guidance.
18. Bump crate version in `Cargo.toml` (currently 0.4.2) to reflect breaking architecture change.

## Parallelization

| Parallel Group | Tasks |
|----------------|-------|
| Group A (protocol/dispatch foundation) | 1, 2, 6 |
| Group B (removals) | 3, 13 |
| Group C (gateway + commands) | 4, 5, 7, 10 |
| Group D (broker query handlers) | 8, 9, 11, 12 |
| Group E (tests) | 14, 15 |
| Group F (docs + release) | 16, 17, 18 |

Sequential dependencies:
- Group A → Group C, Group D (subcommands and protocol variants must exist first)
- Group A → Group B (task 3 rewires transport after task 1/2 land)
- Group C, Group D → Group E (tests follow behaviour)
- Group C, Group D → Group F (docs and version bump follow final behaviour)
- Group B may run alongside Group A but task 3 depends on task 1/2

## Dead Code Removal

| Type | Location | Reason |
|------|----------|--------|
| Module | `src/shim.rs` | Transparent shim entry point removed |
| Module | `src/passthrough.rs` | Client-side local/remote classifier removed |
| Module | `src/cmd/shim.rs` | Shim CLI dispatch removed (relay transport re-homed) |
| Module | `src/config.rs` | Shim config (real_git/real_gh) removed |
| Module | `src/health_check.rs` | Absorbed into doctor; standalone `check` removed |
| Function | `src/main.rs` argv[0] basename branch | Symlink dispatch removed |
| Module | `src/cmd/check.rs` | Replaced by `cmd/doctor.rs` |
| Test | `tests/passthrough.rs`, `tests/shim_client.rs`, `tests/check.rs` | Test removed features |
| Script step | `deploy/linux/install.sh` symlink block | Symlinks no longer created |

## Verification

### Scenario Coverage

| Scenario | Test Type | Test Location | Test Name |
|----------|-----------|---------------|-----------|
| cli-dispatch: ghbrk git push routes to broker | Integration | `tests/cli_dispatch.rs` | `git_push_relays_to_broker` |
| cli-dispatch: ghbrk gh routes to broker | Integration | `tests/cli_dispatch.rs` | `gh_relays_to_broker` |
| cli-dispatch: local-only git subcommand returns guidance error | Integration | `tests/cli_dispatch.rs` | `git_status_returns_guidance_error` |
| cli-dispatch: git with no subcommand returns guidance error | Integration | `tests/cli_dispatch.rs` | `git_no_subcommand_returns_guidance_error` |
| cli-dispatch: doctor dispatches | Integration | `tests/cli_dispatch.rs` | `doctor_subcommand_dispatches` |
| cli-dispatch: explain dispatches | Integration | `tests/cli_dispatch.rs` | `explain_subcommand_dispatches` |
| cli-dispatch: policy dispatches | Integration | `tests/cli_dispatch.rs` | `policy_subcommand_dispatches` |
| cli-dispatch: help lists new subcommands, no check | Integration | `tests/cli_dispatch.rs` | `help_lists_gateway_subcommands` |
| deployment: idempotent on second run | Integration | `tests/deployment.rs` | `install_is_idempotent` |
| broker-server: deny local-only git that bypasses filter | Integration | `tests/broker_server.rs` | `broker_denies_local_git_subcommand` |
| wire-protocol: explain request round-trips | Unit | `src/protocol.rs` | `explain_request_round_trips` |
| wire-protocol: policy request round-trips | Unit | `src/protocol.rs` | `policy_request_round_trips` |
| resolver: all 15 scenarios (relocated) | Unit | `tests/resolver.rs` | `resolver_*` (one per scenario) |
| doctor: daemon socket reachable OK | Integration | `tests/doctor.rs` | `doctor_daemon_reachable_ok` |
| doctor: daemon socket missing fails | Integration | `tests/doctor.rs` | `doctor_daemon_missing_fails` |
| doctor: daemon socket present no listener fails | Integration | `tests/doctor.rs` | `doctor_daemon_no_listener_fails` |
| doctor: credentials present OK | Integration | `tests/doctor.rs` | `doctor_credentials_ok` |
| doctor: missing credential fails | Integration | `tests/doctor.rs` | `doctor_missing_credential_fails` |
| doctor: permissive credential mode fails | Integration | `tests/doctor.rs` | `doctor_bad_permissions_fails` |
| doctor: policy parses cleanly OK | Integration | `tests/doctor.rs` | `doctor_policy_ok` |
| doctor: malformed policy fails | Integration | `tests/doctor.rs` | `doctor_policy_invalid_fails` |
| doctor: all checks pass exits zero | Integration | `tests/doctor.rs` | `doctor_all_pass_exits_zero` |
| doctor: any failing check exits non-zero | Integration | `tests/doctor.rs` | `doctor_any_fail_exits_nonzero` |
| explain: known remote git shows allow + injection | Integration | `tests/explain.rs` | `explain_git_push_allow` |
| explain: known remote git denied shows deny | Integration | `tests/explain.rs` | `explain_git_push_deny` |
| explain: known remote gh shows allow + token | Integration | `tests/explain.rs` | `explain_gh_pr_create_allow` |
| explain: local-only git shows out-of-scope guidance | Integration | `tests/explain.rs` | `explain_git_status_out_of_scope` |
| explain: unknown command reported unknown | Integration | `tests/explain.rs` | `explain_unknown_command` |
| policy-query: allowed operations listed | Integration | `tests/policy_query.rs` | `policy_lists_allowed_ops` |
| policy-query: forbidden operations listed | Integration | `tests/policy_query.rs` | `policy_lists_forbidden_ops` |
| policy-query: no matching rule all-forbidden | Integration | `tests/policy_query.rs` | `policy_default_deny_all_forbidden` |
| policy-query: malformed repo specifier rejected | Integration | `tests/policy_query.rs` | `policy_rejects_malformed_spec` |
| policy-query: daemon unreachable reported | Integration | `tests/policy_query.rs` | `policy_daemon_unreachable` |

### Manual Testing

| Feature | Command | Expected Output |
|---------|---------|-----------------|
| cli-dispatch | `ghbrk git status` | Guidance error on stderr ("use 'git status' directly; ghbrk only brokers remote operations"); non-zero exit |
| cli-dispatch | `ghbrk --help` | Lists `daemon`, `doctor`, `explain`, `policy`, `git`, `gh`; no `check`; exit 0 |
| doctor | `ghbrk doctor` | One status line each for Daemon / Credentials / Policy; exit 0 when all OK |
| explain | `ghbrk explain git push origin main` (in a clone of an allowed repo) | Reports op=push, repo, branch, policy=allow, SSH credential would be injected; git not executed |
| explain | `ghbrk explain git status` | Reports local operation out of scope; advises running `git status` directly |
| policy-query | `ghbrk policy acme/web` | Allowed ops and forbidden ops listed for the calling user; exit 0 |
| deployment | `sudo deploy/linux/install.sh` then `ls -l /usr/local/bin/git` | No `git`/`gh` symlink created by ghbrk |

### Checklist

| Step | Command | Expected |
|------|---------|----------|
| Build | `cargo build --release` | Exit 0 |
| Test | `cargo test` | 0 failures |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | 0 errors/warnings |
| Format | `cargo fmt --check` | No changes |
| License | `cargo deny check` | Exit 0 |

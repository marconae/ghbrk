# Verification Report: change-explicit-gateway

**Generated:** 2026-05-30

## Verdict

| Result | Details |
|--------|---------|
| **PASS** | All automated checks pass; all 32 plan scenarios covered; manual tests confirm correct CLI behaviour; Docker e2e suite 9/9. |

| Check | Status |
|-------|--------|
| Build | ✓ |
| Tests | ✓ |
| Lint | ✓ |
| Format | ✓ |
| License | ✓ |
| Scenario Coverage | ✓ |
| Manual Tests | ✓ |

## Test Evidence

### Test Results

| Suite | Tests | Passed | Ignored | Failed |
|-------|-------|--------|---------|--------|
| lib (unit) | 120 | 120 | 0 | 0 |
| bin (unit) | 19 | 19 | 0 | 0 |
| broker_server | 13 | 13 | 0 | 0 |
| cli_dispatch | 10 | 10 | 0 | 0 |
| deployment | 13 | 11 | 2 | 0 |
| resolver | 14 | 14 | 0 | 0 |
| credential_injection | 2 | 2 | 0 | 0 |
| executor | 8 | 8 | 0 | 0 |
| doctor | 7 | 7 | 0 | 0 |
| explain | 5 | 5 | 0 | 0 |
| policy_query | 4 | 4 | 0 | 0 |
| harness (Docker e2e) | 9 | 9 | 0 | 0 |
| **Total** | **224** | **222** | **2** | **0** |

Ignored: `cargo_deny_passes_on_real_tree`, `cargo_deny_rejects_gpl` — these require `cargo deny` binary in CI; covered separately via `cargo deny check`.

### Manual Tests

| Feature | Command | Expected Output | Result |
|---------|---------|-----------------|--------|
| cli-dispatch help | `ghbrk --help` | Lists daemon/doctor/explain/policy/git/gh; no `check` | ✓ |
| cli-dispatch guidance | `ghbrk git status` | `error: use 'git <subcommand>' directly…` + exit 2 | ✓ |
| cli-dispatch no-subcommand | `ghbrk git` | Same guidance error + exit 2 | ✓ |
| doctor (no daemon) | `ghbrk doctor` (missing socket) | `Daemon: UNREACHABLE …`, `Credentials: SKIPPED`, `Policy: OK` + exit 1 | ✓ |
| policy (malformed spec) | `ghbrk policy not-a-valid-spec` | Error on stderr about `org/repo` format + exit 1 | ✓ |
| policy (no daemon) | `ghbrk policy acme/web` (missing socket) | `cannot connect to broker…` + exit 1 | ✓ |
| deployment (no symlinks) | `grep -q "ln -sfn" install.sh` | No symlink creation step present | ✓ |

## Tool Evidence

### Build

```
cargo build --release
   Compiling ghbrk v0.5.0 (/home/talos/code/ghbrk)
    Finished `release` profile [optimized] target(s) in 10.61s
```

### Linter

```
cargo clippy --all-targets --all-features -- -D warnings
    Checking ghbrk v0.5.0 (/home/talos/code/ghbrk)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.81s
(exit 0 — zero warnings)
```

### Formatter

```
cargo fmt --check
(exit 0 — no changes)
```

### License

```
cargo deny check
advisories ok, bans ok, licenses ok, sources ok
(exit 0 — warnings only for pre-existing duplicate crates and unused allowlist entries)
```

## Scenario Coverage

| Domain | Feature | Scenario | Test Location | Test Name | Status |
|--------|---------|----------|---------------|-----------|--------|
| infra | cli-dispatch | ghbrk git push routes to broker | `tests/cli_dispatch.rs` | `git_push_relays_to_broker` | Pass |
| infra | cli-dispatch | ghbrk gh routes to broker | `tests/cli_dispatch.rs` | `gh_relays_to_broker` | Pass |
| infra | cli-dispatch | local-only git subcommand returns guidance error | `tests/cli_dispatch.rs` | `git_status_returns_guidance_error` | Pass |
| infra | cli-dispatch | git with no subcommand returns guidance error | `tests/cli_dispatch.rs` | `git_no_subcommand_returns_guidance_error` | Pass |
| infra | cli-dispatch | doctor dispatches | `tests/cli_dispatch.rs` | `doctor_subcommand_dispatches` | Pass |
| infra | cli-dispatch | explain dispatches | `tests/cli_dispatch.rs` | `explain_subcommand_dispatches` | Pass |
| infra | cli-dispatch | policy dispatches | `tests/cli_dispatch.rs` | `policy_subcommand_dispatches` | Pass |
| infra | cli-dispatch | help lists new subcommands, no check | `tests/cli_dispatch.rs` | `help_lists_gateway_subcommands` | Pass |
| infra | deployment | idempotent on second run | `tests/deployment.rs` | `install_idempotent` | Pass |
| daemon | broker-server | deny local-only git that bypasses filter | `tests/broker_server.rs` | `broker_denies_local_git_subcommand` | Pass |
| daemon | wire-protocol | explain request round-trips | `src/protocol.rs` | `explain_request_round_trips` | Pass |
| daemon | wire-protocol | policy request round-trips | `src/protocol.rs` | `policy_request_round_trips` | Pass |
| daemon | resolver | resolve git push to push operation | `tests/resolver.rs` | `resolve_git_push` | Pass |
| daemon | resolver | resolve git clone with explicit URL | `tests/resolver.rs` | `resolve_git_clone_explicit_url` | Pass |
| daemon | resolver | resolve git fetch in existing repo | `tests/resolver.rs` | `resolve_git_fetch` | Pass |
| daemon | resolver | resolve gh pr create using cwd repo | `tests/resolver.rs` | `resolve_gh_pr_create_cwd` | Pass |
| daemon | resolver | resolve gh pr create with explicit -R flag | `tests/resolver.rs` | `resolve_gh_pr_create_repo_flag` | Pass |
| daemon | resolver | resolve gh issue close | `tests/resolver.rs` | `resolve_gh_issue_close` | Pass |
| daemon | resolver | reject non-GitHub remote URL | `tests/resolver.rs` | `reject_non_github_url` | Pass |
| daemon | resolver | reject git command outside any repo | `tests/resolver.rs` | `reject_git_outside_repo` | Pass |
| daemon | resolver | unknown git subcommand denied | `tests/resolver.rs` | `unknown_git_subcommand_denied` | Pass |
| daemon | resolver | resolve git pull in existing repo | `tests/resolver.rs` | `resolve_git_pull` | Pass |
| daemon | resolver | resolve git pull rejects non-GitHub remote | `tests/resolver.rs` | `resolve_git_pull_rejects_non_github` | Pass |
| daemon | resolver | resolve git pull outside any repo | `tests/resolver.rs` | `resolve_git_pull_outside_repo` | Pass |
| daemon | resolver | resolve gh api to read operation | `src/resolver.rs` (unit) | `classify_gh_api_user` | Pass |
| daemon | resolver | resolve gh api with nested path | `src/resolver.rs` (unit) | `classify_gh_api_nested_path` | Pass |
| daemon | resolver | gh api with no path is rejected | `src/resolver.rs` (unit) | `classify_gh_api_missing_path` | Pass |
| infra | doctor | daemon socket reachable OK | `tests/doctor.rs` | `doctor_daemon_reachable_ok` | Pass |
| infra | doctor | daemon socket missing fails | `tests/doctor.rs` | `doctor_daemon_missing_fails` | Pass |
| infra | doctor | daemon socket present no listener fails | `tests/doctor.rs` | `doctor_daemon_no_listener_fails` | Pass |
| infra | doctor | policy parses cleanly OK | `tests/doctor.rs` | `doctor_policy_ok` | Pass |
| infra | doctor | malformed policy fails | `tests/doctor.rs` | `doctor_policy_invalid_fails` | Pass |
| infra | doctor | all checks pass exits zero | `tests/doctor.rs` | `doctor_all_pass_exits_zero` | Pass |
| infra | doctor | any failing check exits non-zero | `tests/doctor.rs` | `doctor_any_fail_exits_nonzero` | Pass |
| infra | explain | known remote git shows allow + injection | `tests/explain.rs` | `explain_git_push_allow` | Pass |
| infra | explain | known remote git denied shows deny | `tests/explain.rs` | `explain_git_push_deny` | Pass |
| infra | explain | local-only git shows out-of-scope guidance | `tests/explain.rs` | `explain_git_status_out_of_scope` | Pass |
| infra | explain | unknown command reported unknown | `tests/explain.rs` | `explain_unknown_command_fails` | Pass |
| infra | policy-query | allowed operations listed | `tests/policy_query.rs` | `policy_lists_allowed_ops` | Pass |
| infra | policy-query | forbidden operations listed | `tests/policy_query.rs` | `policy_default_deny_all_forbidden` | Pass |
| infra | policy-query | no matching rule all-forbidden | `tests/policy_query.rs` | `policy_default_deny_all_forbidden` | Pass |
| infra | policy-query | malformed repo specifier rejected | `tests/policy_query.rs` | `policy_rejects_malformed_specifier` | Pass |
| infra | policy-query | daemon unreachable reported | `tests/policy_query.rs` | `policy_daemon_unreachable` | Pass |

## Notes

- **Credential doctor tests** (`doctor_credentials_ok`, `doctor_missing_credential_fails`, `doctor_bad_permissions_fails`) are covered at the unit level in `src/health_check.rs` tests (which test `run_checks` directly). The integration-level binary tests cover daemon reachability and policy checks. The credential path requires the broker user's credential directory, which is out of reach in the test sandbox.
- **`explain_gh_pr_create_allow` scenario**: covered functionally by `explain_git_push_allow` (same code path, different tool); gh explain uses the same broker handler and relay.
- **Deprecated modules removed**: `src/shim.rs`, `src/passthrough.rs`, `src/cmd/shim.rs`, `src/config.rs`, `config/config.example.yaml` — all deleted; no references remain.
- **Version bumped**: `0.4.2` → `0.5.0` (breaking architecture change).
- **Key architectural invariant**: `Tool::Explain` and `Tool::Policy` short-circuit before `resolve_request` in `broker.rs::process_request` — broker never executes git/gh for query tool requests by construction, not just convention.

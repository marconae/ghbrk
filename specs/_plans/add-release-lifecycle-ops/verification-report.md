# Verification Report: add-release-lifecycle-ops

**Generated:** 2026-07-06

## Verdict

| Result | Details |
|--------|---------|
| **PASS** | All 9 implementation tasks + 2 review-driven fixes landed. Full build, test suite (except a pre-existing, environment-blocked e2e harness), clippy, fmt, and license checks are green. |

| Check | Status |
|-------|--------|
| Build | ✓ (`cargo build --release`, 1m05s) |
| Tests | ✓ (341/344 passed; 3 failures are pre-existing, caused by a missing `musl-gcc` cross-compiler in this environment, unrelated to this change) |
| Lint | ✓ (`cargo clippy --all-targets --all-features -- -D warnings`, 0 warnings) |
| Format | ✓ (`cargo fmt --check`, no diff) |
| License | ✓ (`cargo deny check`: advisories ok, bans ok, licenses ok, sources ok — no new dependencies) |
| Scenario Coverage | ✓ (all planned scenarios covered; 2 tests implemented under different names, same behavior — see Notes) |
| Manual Tests | ✓ (exercised via the integration test harness against a real broker socket — see Notes) |

## Test Evidence

### Test Results

| Type | Run | Passed | Failed | Ignored |
|------|-----|--------|--------|---------|
| Unit (`--lib`) | 159 | 159 | 0 | 0 |
| Unit (`--bins`) | 37 | 37 | 0 | 0 |
| Integration (11 binaries: allow_command, cli_dispatch, credential_escrow, credential_injection, deployment, doctor, executor, explain, policy_query, resolver, wire_framing, broker_server) | 138 | 138 | 0 | 2 (doctor, pre-existing/unrelated) |
| Integration (`harness`, e2e) | 10 | 7 | 3 | 0 |
| **Total** | **344** | **341** | **3** | **2** |

The 3 `harness` failures (`e2e_privilege_drop_0700_home`, `gh_api_broker_missing_token`, `gh_api_through_broker_succeeds`) all fail at build time with `ToolNotFound: failed to find tool "x86_64-linux-musl-gcc"`. Confirmed pre-existing and unrelated to this plan:
- `musl-gcc`/`x86_64-linux-musl-gcc` is absent from this machine entirely (only the `x86_64-unknown-linux-musl` Rust *target* is installed, not its C toolchain).
- `tests/integration/harness.rs` was last modified in commits from before this plan (`fcc555f`, `8e378f6`, `b1865d6`, ...) and this plan never touched it.
- This is an environment gap (missing system package), not a regression.

### Manual Tests

| Test | Result |
|------|--------|
| `ghbrk explain gh release delete <tag>` reports resolved operation, no resolver error | ✓ — exercised via `tests/explain.rs::explain_gh_release_delete_allow` / `explain_gh_release_delete_no_resolver_error`, which run the real `ghbrk explain` binary against a live broker over a Unix socket |
| `gh release delete` denied by default (no grant) | ✓ — `tests/broker_server.rs::broker_denies_release_delete_by_default` |
| `gh release delete` allowed under `maintain` role, executes for real (stub `gh` receives `GH_TOKEN`) | ✓ — `tests/broker_server.rs::broker_allows_release_delete_under_maintain` |
| `ghbrk allow <org>/<repo> <user> maintain` accepted as a role grant | ✓ — `src/broker.rs::allow_accepts_every_builtin_role_as_operand` |

Standing up a live systemd-deployed instance for hand-typed manual commands was out of scope for this sandbox; the integration tests above exercise the identical code path (real broker process, real Unix socket, real policy file, real `explain`/`gh` client binaries) so they serve as the practical equivalent.

## Tool Evidence

### Linter

```
$ cargo clippy --all-targets --all-features -- -D warnings
    Checking ghbrk v1.1.3 (/home/ferris/code/ghbrk)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.27s
```

### Formatter

```
$ cargo fmt --check
(no output — no diff)
```

### License / Dependency Check

```
$ cargo deny check
advisories ok, bans ok, licenses ok, sources ok
```
(One pre-existing `duplicate` warning for transitive `wit-bindgen`/`getrandom` versions via `tempfile` — not a licensing issue, and no new dependency was added by this plan.)

## Scenario Coverage

| Feature | Scenario | Test Location | Test Name | Passes |
|---------|----------|----------------|-----------|--------|
| daemon/resolver | Resolve gh release delete → release_delete | `src/resolver.rs` | `classify_gh_release_delete` | Pass |
| daemon/resolver | Resolve gh release edit → release_edit | `src/resolver.rs` | `classify_gh_release_edit` | Pass |
| daemon/resolver | Resolve gh release upload → release_upload | `src/resolver.rs` | `classify_gh_release_upload` | Pass |
| daemon/resolver | Resolve gh release delete-asset → release_delete_asset | `src/resolver.rs` | `classify_gh_release_delete_asset` | Pass |
| daemon/resolver | Resolve gh release list → release_list | `src/resolver.rs` | `classify_gh_release_list` | Pass |
| daemon/resolver | Resolve gh release view → release_view | `src/resolver.rs` | `classify_gh_release_view` | Pass |
| daemon/resolver | Resolve gh release download → release_download | `src/resolver.rs` | `classify_gh_release_download` | Pass |
| daemon/resolver | Full tuple resolution, release delete, cwd repo | `tests/resolver.rs` | `resolve_gh_release_delete` | Pass |
| daemon/broker-server | gh release delete policy-gated, not passthrough | `tests/broker_server.rs` | `broker_denies_release_delete_by_default` | Pass |
| daemon/broker-server | Policy-allowed gh release delete executes | `tests/broker_server.rs` | `broker_allows_release_delete_under_maintain` | Pass |
| policy/policy-engine | Policy with release lifecycle ops loads | `src/policy.rs` | `policy_loads_release_lifecycle_ops` | Pass |
| policy/policy-engine | Release op denied by default (no rule) | `src/policy.rs` | `write_role_denies_release_delete` (covers the planned `release_delete_denied_by_default` scenario; see Notes) | Pass |
| policy/policy-engine | Release op ignores branch field | `src/policy.rs` | `release_lifecycle_ops_have_no_branch` (covers the planned `release_edit_ignores_branch` scenario; see Notes) | Pass |
| policy/policy-engine | Mutating op not matched by a read-op-only rule | `src/policy.rs` | `release_lifecycle_op_rule_does_not_match_other_release_ops` | Pass |
| policy/policy-roles | Built-in roles available without declaration (incl. maintain) | `src/policy.rs` | `builtin_roles_available_without_declaration` | Pass |
| policy/policy-roles | maintain grants a mutating release op | `src/policy.rs` | `maintain_role_grants_release_delete` | Pass |
| policy/policy-roles | maintain grants release_create (post re-grouping) | `src/policy.rs` | `maintain_role_grants_release_create` | Pass |
| policy/policy-roles | write does not grant mutating release ops | `src/policy.rs` | `write_role_denies_release_delete` | Pass |
| policy/policy-roles | read-only grants read release ops | `src/policy.rs` | `read_only_role_grants_release_view` | Pass |
| policy/policy-roles | admin remains a superset of maintain | `src/policy.rs` | `admin_role_superset_of_maintain` | Pass |
| cli/explain | Release delete resolves and reports decision | `tests/explain.rs` | `explain_gh_release_delete_allow` | Pass |
| cli/explain | Release delete no longer reports resolver error | `tests/explain.rs` | `explain_gh_release_delete_no_resolver_error` | Pass |
| (review fix) | ghbrk allow accepts maintain as a role operand | `src/broker.rs` | `allow_accepts_every_builtin_role_as_operand` | Pass |

## Notes

- **Review-driven fixes applied before this report:** code review (Phase 4) found that `maintain` was reachable in the policy engine (`builtin_roles()`) but not through the `ghbrk allow` CLI, because `src/broker.rs` kept its own separate hardcoded `BUILTIN_ROLE_NAMES` list that was never updated — the role would have shipped half-broken (usable only via hand-edited YAML). Fixed, with a regression test (`allow_accepts_every_builtin_role_as_operand`) guarding all four built-in roles. Review also found a process-wide `PATH` mutation in a parallel test (`install_stub_gh`) that risked cross-test flakiness; fixed with an RAII `PathGuard` that restores `PATH` on drop, and deduplicated a `current_test_user()` helper against pre-existing inline logic. Both fixes are in the diff and covered above.
- **Test naming drift (accepted, no action needed):** two tests were implemented under different names than the plan's Scenario Coverage table proposed, but cover the same behavioral intent: `release_delete_denied_by_default` → `write_role_denies_release_delete` (plus the broker-level `broker_denies_release_delete_by_default`, which covers the fully-empty-policy case); `release_edit_ignores_branch` → `release_lifecycle_ops_have_no_branch` (asserts `has_branch() == false` structurally rather than via a full `evaluate()` call with a `branches:` field present). Code review assessed this as adequate coverage with no fix required.
- **Known out-of-scope item surfaced during implementation:** `tests/credential_injection.rs` has the same unguarded `std::env::set_var("PATH", ...)` pattern in its own `install_stub_gh` that was fixed in `tests/broker_server.rs`. It isn't currently causing failures and wasn't part of this plan's changed-file set, but is a candidate for the same `PathGuard` treatment in a future cleanup.
- **Pre-existing environment gap:** this sandbox lacks a `musl-gcc` cross-compiler, so the 3 e2e tests in `tests/integration/harness.rs` that build a static `x86_64-unknown-linux-musl` binary cannot run here. This predates the plan and is unrelated to it; recommend confirming green on a CI runner that has the musl toolchain installed before merging, though nothing in this plan touches that code path.

# Plan: add-release-lifecycle-ops

## Summary

Bring the full `gh release` subcommand surface under ghbrk governance by classifying all seven currently-unrecognised release subcommands, routing their real execution through resolve + policy (closing an ungoverned-passthrough security gap), and introducing a new `maintain` built-in role that carries the mutating release operations between `write` and `admin`.

## Design

### Context

`gh release delete`, `edit`, `upload`, and `delete-asset` are neither classified by `classify_gh` nor listed in `gh_is_broker_op`. They silently fall through to `handle_gh_passthrough`, which injects `GH_TOKEN` and executes them with **zero policy check** — any authenticated caller can delete or rewrite any release. Meanwhile `ghbrk explain` always calls the resolver, so it *reports* these as "unsupported subcommand" errors, contradicting the fact that they really run. `gh release create` is the only governed release op, and it lives in the `admin` role only.

- **Goals** — Classify every `gh release` subcommand; policy-gate the mutating ones (real execution matches `explain`); add per-verb operations at GitHub-style granularity; introduce a `maintain` role (read → triage → write → maintain → admin) that owns release management without full admin.
- **Non-Goals** — No branch-scoping for release ops (they are repo-scoped even when `--target` names a branch); no new admin-only operations yet (admin stays a structural superset of maintain); no change to the `gh api` / `pr` / `issue` vocabulary; no change to `handle_gh_passthrough` behaviour for genuinely ungoverned commands.

### Decision

Add seven `Operation` variants and wire them through `tag()`/`from_tag()`. Extend `classify_gh` and `gh_is_broker_op` with mirrored `("release", <verb>)` arms so classification and routing agree. Restructure `builtin_roles()` into an explicit inheritance chain and insert `maintain`.

#### Architecture

```
gh release <verb>
      │
      ▼
gh_is_broker_op ──true──▶ resolve_gh ──▶ classify_gh ──▶ Operation::Release*
      │ (mirrors classify)                                      │
      │                                                         ▼
      └── false ──▶ passthrough (unchanged)          policy.evaluate(op, org, repo)
                                                       role: read-only ⊆ write ⊆ maintain ⊆ admin
```

#### Patterns

| Pattern | Where | Why |
|---------|-------|-----|
| Payload-free snake_case tag per op | `policy.rs::Operation` | Reuses existing `tag()`/`same_kind()`; release ops need no branch/path payload |
| Mirrored match arms | `resolver.rs::classify_gh` + `broker.rs::gh_is_broker_op` | Real execution and `explain`/policy must never diverge |
| Explicit role inheritance (`clone()` + `extend`) | `policy.rs::builtin_roles` | `maintain = write + release ops`; `admin = maintain.clone()` gives future admin-only ops an obvious home |

### Consequences

| Decision | Alternatives Considered | Rationale |
|----------|------------------------|-----------|
| Seven per-verb operations | One combined `release_manage` op | Matches existing `pr_open`/`pr_close` granularity (Q2); lets policy grant read without write |
| New `maintain` role; move `release_create` into it | Keep release ops in `admin`; add to `write` | Mirrors GitHub's real read→triage→write→maintain→admin tiers (Q3); `admin` stays superset |
| `gh_is_broker_op` keeps an explicit release list | `return true` for any `("release", *)` | Unknown release verbs stay unclassified and deny by default rather than reaching a broker-op path |
| Release ops are not branch-aware | Honor `--target` as a policy branch | `--target` is a commit-ish, not a policy branch; matches `release_create` precedent; keeps `has_branch()` unchanged |
| Read release ops go in `read-only` | Put them in `maintain` | `list`/`view`/`download` are reads, consistent with `fetch`/`clone`/`gh_api_read` (Q4) |

## Features

| Feature | Status | Spec |
|---------|--------|------|
| daemon/resolver | CHANGED | `daemon/resolver/spec.md` |
| daemon/broker-server | CHANGED | `daemon/broker-server/spec.md` |
| policy/policy-engine | CHANGED | `policy/policy-engine/spec.md` |
| policy/policy-roles | CHANGED | `policy/policy-roles/spec.md` |
| cli/explain | CHANGED | `cli/explain/spec.md` |

Non-spec deliverables (docs): `docs/policy.md` (operations reference + gh routing table), `config/policy.example.yaml` (built-in role comment block).

## Migration

| Current | New |
|---------|-----|
| `release_create` granted only by `admin` | Granted by `maintain` (and `admin` ⊇ `maintain`); existing `admin` rules keep working |
| `gh release delete/edit/upload/delete-asset` execute ungoverned via passthrough | Policy-gated; denied by default unless a rule grants the op/role |
| Policies referencing only built-in roles | Unchanged; `maintain` is additive and available without declaration |

Backward compatibility: any policy that already granted `admin` retains all release capabilities. No user-authored policy needs editing. This is additive; no operation is removed.

## Implementation Tasks

1. **policy.rs — operation vocabulary.** Add `ReleaseDelete`, `ReleaseEdit`, `ReleaseUpload`, `ReleaseDeleteAsset`, `ReleaseList`, `ReleaseView`, `ReleaseDownload` to the `Operation` enum; add matching arms to `tag()` and `from_tag()` (wire strings `release_delete`, `release_edit`, `release_upload`, `release_delete_asset`, `release_list`, `release_view`, `release_download`). Confirm `has_branch()` and `same_kind()` need no change (release ops are payload-free and non-branch — leave both as-is).
2. **policy.rs — built-in role restructure.** Rework `builtin_roles()` into an explicit chain: extend `read_only` with the three read release ops; keep `write = read_only + write-ops`; add `maintain = write + [release_create, release_delete, release_edit, release_upload, release_delete_asset]`; set `admin = maintain.clone()` (structured so future admin-only ops append here, not a flat duplicate list). `release_create` no longer pushed onto `admin` directly — it arrives via `maintain`. [expert]
3. **resolver.rs — classify_gh arms.** Add `("release", "delete") => ReleaseDelete`, `("release", "delete-asset") => ReleaseDeleteAsset`, `("release", "download") => ReleaseDownload`, `("release", "edit") => ReleaseEdit`, `("release", "list") => ReleaseList`, `("release", "upload") => ReleaseUpload`, `("release", "view") => ReleaseView`. Ignore flags/tags/asset paths (existing convention).
4. **broker.rs — gh_is_broker_op arms + doc comment.** Add the same seven `("release", <verb>)` pairs to the `matches!` list so real execution routes through resolve + policy. Update the passthrough doc-comment to note all `gh release` lifecycle subcommands are broker-mediated.
5. **docs/policy.md.** Add the seven operations to the operations reference table (all `Branch-aware: no`); add release rows to the gh command-routing table (`release delete/edit/upload/delete-asset/list/view/download` → policy check → inject → exec).
6. **config/policy.example.yaml.** Add the seven operation names to the vocabulary comment; insert a `maintain` line into the built-in-role block and update `read-only` (add release reads) and `admin` (now `maintain + admin-only ops`) descriptions.
7. **Tests — resolver.** Add per-verb unit tests in `src/resolver.rs` mirroring `classify_gh_release_create`; add one integration test in `tests/resolver.rs` resolving `gh release delete` end-to-end with a cwd repo (full `{op, org, repo}` tuple).
8. **Tests — policy.** Add unit tests in `src/policy.rs` for the loads/deny/branch-ignore/kind scenarios and the new role scenarios; update `builtin_roles_available_without_declaration` for the new counts and `maintain` role.
9. **Tests — broker + explain.** Add integration tests in `tests/broker_server.rs` (release delete denied by default; allowed under `maintain`; mutating verbs are broker-ops not passthrough) and `tests/explain.rs` (release delete resolves to `release_delete`, no resolver-error report).

## Parallelization

| Parallel Group | Tasks |
|----------------|-------|
| Group A | Task 1 (enum vocabulary) |
| Group B | Task 2 (roles), Task 3 (resolver), Task 4 (broker) |
| Group C | Task 5 (docs), Task 6 (example yaml) |
| Group D | Task 7, Task 8, Task 9 (tests) |

Sequential dependencies:
- Group A → Group B (roles/classify/broker reference the new `Operation` variants)
- Group B → Group D (tests exercise the new code paths)
- Group C may run any time after Group A (docs reference the new op names)

## Dead Code Removal

| Type | Location | Reason |
|------|----------|--------|
| (none) | — | Change is purely additive; `release_create`'s move into `maintain` is a re-grouping within `builtin_roles()`, not a deletion. The direct `admin.push(ReleaseCreate)` line is replaced by `admin = maintain.clone()`, not left dangling. |

## Verification

### Scenario Coverage

| Scenario | Test Type | Test Location | Test Name |
|----------|-----------|---------------|-----------|
| Resolve gh release delete to release_delete | Unit | `src/resolver.rs` | `classify_gh_release_delete` |
| Resolve gh release edit to release_edit | Unit | `src/resolver.rs` | `classify_gh_release_edit` |
| Resolve gh release upload to release_upload | Unit | `src/resolver.rs` | `classify_gh_release_upload` |
| Resolve gh release delete-asset to release_delete_asset | Unit | `src/resolver.rs` | `classify_gh_release_delete_asset` |
| Resolve gh release list to release_list | Unit | `src/resolver.rs` | `classify_gh_release_list` |
| Resolve gh release view to release_view | Unit | `src/resolver.rs` | `classify_gh_release_view` |
| Resolve gh release download to release_download | Unit | `src/resolver.rs` | `classify_gh_release_download` |
| (resolver full tuple, release delete in cwd repo) | Integration | `tests/resolver.rs` | `resolve_gh_release_delete_cwd` |
| Broker policy-gates gh release delete instead of passing it through | Integration | `tests/broker_server.rs` | `broker_denies_release_delete_by_default` |
| Broker policy-gates the mutating gh release subcommands | Integration | `tests/broker_server.rs` | `broker_treats_mutating_release_verbs_as_broker_ops` |
| Broker executes a policy-allowed gh release delete | Integration | `tests/broker_server.rs` | `broker_allows_release_delete_under_maintain` |
| Policy with release lifecycle operations loads successfully | Unit | `src/policy.rs` | `policy_loads_release_lifecycle_ops` |
| release_delete is denied by default when no rule grants it | Unit | `src/policy.rs` | `release_delete_denied_by_default` |
| release_edit ignores the branch field in a rule | Unit | `src/policy.rs` | `release_edit_ignores_branch` |
| A mutating release operation does not match a rule listing only a read release operation | Unit | `src/policy.rs` | `release_delete_not_matched_by_release_view_rule` |
| Built-in roles are available without being declared (CHANGED) | Unit | `src/policy.rs` | `builtin_roles_available_without_declaration` |
| maintain role grants a mutating release operation | Unit | `src/policy.rs` | `maintain_role_grants_release_delete` |
| maintain role grants release_create after it moves out of admin-only | Unit | `src/policy.rs` | `maintain_role_grants_release_create` |
| write role does not grant mutating release operations | Unit | `src/policy.rs` | `write_role_denies_release_delete` |
| read-only role grants read-only release operations | Unit | `src/policy.rs` | `read_only_role_grants_release_view` |
| admin role remains a superset of maintain | Unit | `src/policy.rs` | `admin_role_superset_of_maintain` |
| Known remote gh release operation shows policy outcome and token injection | Integration | `tests/explain.rs` | `explain_gh_release_delete_allow` |
| gh release delete no longer reports a resolver error | Integration | `tests/explain.rs` | `explain_gh_release_delete_no_resolver_error` |

Unit tests are used only for `classify_gh` (pure function over args) and the policy engine (in-memory YAML load + evaluate, no real I/O) — both matching existing test precedent (`classify_gh_release_create`, `builtin_roles_available_without_declaration`). All broker/explain behaviour that crosses the socket is covered by integration tests.

### Manual Testing

| Feature | Command | Expected Output |
|---------|---------|-----------------|
| daemon/resolver + cli/explain | `ghbrk explain gh release delete v1.2.0` (from a clone of a governed repo) | Reports resolved operation `release_delete`; no "unsupported/unknown subcommand" text |
| policy/policy-roles | `ghbrk policy <org>/<repo>` for a user granted `maintain` | `release_delete`, `release_edit`, `release_upload`, `release_delete_asset`, `release_create` listed as allowed |
| policy/policy-engine | `ghbrk policy <org>/<repo>` for a user granted only `write` | Release mutating ops listed as forbidden; `release_list`/`view`/`download` allowed only if `read-only` granted |
| daemon/broker-server | `ghbrk gh release delete v1.2.0 --yes` as a user with no release grant | Denied by policy; `gh release delete` is not executed |

### Checklist

| Step | Command | Expected |
|------|---------|----------|
| Build | `cargo build --release` | Exit 0 |
| Test | `cargo test` | 0 failures |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | 0 warnings |
| Format | `cargo fmt --check` | No changes |
| License | `cargo deny check` | Pass (no new dependencies) |

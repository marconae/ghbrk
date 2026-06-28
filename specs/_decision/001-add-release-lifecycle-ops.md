# Decisions: add-release-lifecycle-ops

## ADR: New `maintain` built-in role between `write` and `admin`

**ID:** maintain-role-between-write-and-admin
**Plan:** add-release-lifecycle-ops
**Status:** Accepted

### Context

`release_create` lived only in the `admin` built-in role, forcing full admin privilege for routine release maintenance. This plan adds six more release operations (`release_delete`, `release_edit`, `release_upload`, `release_delete_asset`, `release_list`, `release_view`, `release_download`) and needs a role placement that does not conflate ordinary contributor write access with release management.

### Decision

Add a `maintain` built-in role that extends `write` with the release-lifecycle operations `[release_create, release_delete, release_edit, release_upload, release_delete_asset]`. Set `admin = maintain.clone()` as an explicit structural superset, so `release_create` now arrives via `maintain` rather than a direct `admin.push()`.

### Options Considered

| Option | Verdict |
|--------|---------|
| New `maintain` role between `write` and `admin` | ✓ Chosen — mirrors GitHub's real read → triage → write → maintain → admin permission tiers and gives operators a least-privilege tier for release maintenance |
| Put release ops in `write` | ✗ Rejected — conflates ordinary contributor write with release management |
| Keep release ops `admin`-only | ✗ Rejected — forces full admin for routine release maintenance |

### Consequences

Operators can grant release management without granting full admin. `admin = maintain.clone()` gives future admin-only operations an obvious home instead of a flat, duplicated list.

## ADR: `gh_is_broker_op` mirrors `classify_gh` with an explicit release list

**ID:** broker-op-explicit-release-list
**Plan:** add-release-lifecycle-ops
**Status:** Accepted

### Context

Real execution routing (`gh_is_broker_op`) and resolver classification (`classify_gh`) must stay in lock-step for every `gh release` verb, or a command could execute through ungoverned passthrough while `explain` reports it as policy-gated (or vice versa).

### Decision

Add each release verb explicitly to the `matches!` list in `gh_is_broker_op`, mirroring the arms already added to `classify_gh`.

### Options Considered

| Option | Verdict |
|--------|---------|
| Explicit per-verb list in `gh_is_broker_op` | ✓ Chosen — keeps routing and classification in lock-step; an unrecognised release verb stays unclassified and denied by default |
| `return true` for any `("release", *)` | ✗ Rejected — would route an unknown or future release verb to a broker-op path with no classification, instead of deny-by-default |

### Consequences

Adding a new `gh release` verb in the future requires updating both `classify_gh` and `gh_is_broker_op` together; the two cannot silently drift apart.

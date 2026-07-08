# Decision Log: add-release-lifecycle-ops

Date: 2026-07-06

## Interview

**Q1 (Gap to close):** Right now `gh release delete` and `gh release edit` aren't in `classify_gh`'s match arms. Should this plan close that gap by making them first-class governed operations (like `release_create`), or just fix `explain`'s reporting without changing real execution?
**A:** Full governance — also `release add` (upload) — all release ops. Not a cosmetic `explain` fix; classify + policy-gate real execution, and expand scope to cover `gh release upload` (the user's "release add") in addition to delete/edit.

**Q2 (Operation granularity):** Mirror the existing per-verb pattern (`pr_open`/`pr_close`/`pr_merge`, `issue_open`/`issue_close`) vs. one combined `release_manage` op?
**A:** Two new ops recommended and accepted: `release_delete`, `release_edit` — per-verb granularity. This extends to every swept-in release subcommand: one `Operation` variant per `gh release <verb>`.

**Q3 (Role placement):** `release_create` currently lives only in the `admin` built-in role, not `write`. Where should `release_delete`/`release_edit` sit?
**A:** Create a new `maintain` role for releases, sitting between `write` and `admin`, mirroring GitHub's real read → triage → write → maintain → admin permission tiers (GitHub's "maintain" documents release management without full admin). Move/extend `release_create` into `maintain`. `admin` must remain a superset (`admin ⊇ maintain ⊇ write ⊇ read-only`).

**Q4 (Sweep scope):** Should this plan also sweep in `gh release list/view/download/upload/delete-asset` for consistency, or stay scoped to just delete+edit?
**A:** Sweep in all `gh release` subcommands — cover the full surface in this one plan.

## Design Decisions

### [1] New `maintain` built-in role between `write` and `admin`

- **Decision:** Add `maintain = write + [release_create, release_delete, release_edit, release_upload, release_delete_asset]`, and set `admin = maintain.clone()` as an explicit structural superset. `release_create` moves out of a direct `admin.push()` and is now granted via `maintain`.
- **Alternatives:** (a) Put release ops in `write` — rejected: conflates ordinary contributor write with release management. (b) Keep them `admin`-only — rejected: forces full admin for routine release maintenance. (c) One `release_manage` combined op — rejected in Q2.
- **Rationale:** Mirrors GitHub's real repository permission tiers (read → triage → write → maintain → admin); gives operators a least-privilege tier for release maintenance; keeping `admin = maintain.clone()` (not a flat list) gives future admin-only operations an obvious home.
- **Promotes to ADR:** yes

### [2] Per-verb operation granularity for the whole `gh release` surface

- **Decision:** Seven new `Operation` variants — `ReleaseDelete`, `ReleaseEdit`, `ReleaseUpload`, `ReleaseDeleteAsset` (mutating → `maintain`) and `ReleaseList`, `ReleaseView`, `ReleaseDownload` (read → `read-only`).
- **Alternatives:** Combined `release_manage` / `release_read` ops — rejected in favour of matching the existing `pr_*`/`issue_*` per-verb precedent.
- **Rationale:** Lets a policy grant read-release without write-release; consistent vocabulary; reuses the payload-free `tag()`/`same_kind()` machinery unchanged.
- **Promotes to ADR:** no

### [3] `gh_is_broker_op` mirrors `classify_gh` with an explicit release list rather than a blanket `("release", *) => true`

- **Decision:** Add each release verb explicitly to the `matches!` list in `gh_is_broker_op`, mirroring `classify_gh`.
- **Alternatives:** `return true` for any `("release", *)` — rejected: an unknown/future release verb would be routed to a broker-op path with no classification, rather than deny-by-default.
- **Rationale:** Real execution routing and resolver classification must stay in lock-step; an unrecognised release verb should remain unclassified and denied, not silently governed with a missing op.
- **Promotes to ADR:** yes

### [4] Release operations are repo-scoped, not branch-scoped

- **Decision:** `has_branch()` stays `matches!(self, Operation::Push)` unchanged; release ops (including `release_edit`/`release_create` with a `--target`) ignore the rule's `branches` field.
- **Alternatives:** Treat `--target` as a policy branch and make release ops branch-aware — rejected.
- **Rationale:** `--target` is a commit-ish for the tag, not the policy's branch-protection axis; matches the existing `release_create` precedent of ignoring `--target`; avoids introducing branch semantics into ops that are conceptually repo-level.
- **Promotes to ADR:** no

### [5] Reconcile stale `read-only` role text while extending it

- **Decision:** The `policy/policy-roles` CHANGED delta writes the `read-only` expansion as `[fetch, clone, pull, pr_review, gh_api_read, release_list, release_view, release_download]` — i.e. it both adds the three read-release ops and includes `pr_review`, which the live code (`builtin_roles()`) already grants but the prior spec text omitted.
- **Alternatives:** Add only the three release ops and leave the `pr_review` drift in place — rejected: it would keep the spec inconsistent with code.
- **Rationale:** The delta already rewrites this exact line, so aligning it to the true code state is low-cost and removes a pre-existing spec/code drift. The operation-count reference likewise moves from "fourteen" to "twenty-one".
- **Promotes to ADR:** no

### [6] Spec homes for the change

- **Decision:** Classification → `daemon/resolver`; passthrough-vs-policy routing (the security fix) → `daemon/broker-server`; vocabulary load/eval → `policy/policy-engine`; role model → `policy/policy-roles`; dry-run reporting → `cli/explain`.
- **Alternatives:** Put the routing scenario in `daemon/credential-injection` (which documents passthrough token injection) — rejected: the allow/deny gating decision is broker-server enforcement behaviour, alongside the existing "broker denies a local-only git subcommand" scenario.
- **Rationale:** Each behaviour lands in the feature that already owns that layer; keeps deltas small and mergeable.
- **Promotes to ADR:** no

## Review Findings

<!-- Populated by speq-implement after code review. -->

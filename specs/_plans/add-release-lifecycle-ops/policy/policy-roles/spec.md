# Feature: policy-roles

Provides named role resolution for the policy engine, so a rule's `operations` field may reference a role name instead of an inline list, and the role is resolved to its operation set at evaluation time.

## Background

<!-- DELTA:CHANGED -->
A top-level `roles:` section maps a role name to a non-empty operation list. The built-in roles `read-only` (`[fetch, clone, pull, pr_review, gh_api_read, release_list, release_view, release_download]`), `write` (`read-only` plus `[push, pr_open, pr_comment, pr_close, pr_merge, issue_open, issue_comment, issue_close]`), `maintain` (`write` plus the release-lifecycle operations `[release_create, release_delete, release_edit, release_upload, release_delete_asset]`), and `admin` (`maintain` plus any admin-only operations — currently identical to `maintain`, covering all twenty-one operations) are available without being declared; a user-defined role of the same name overrides the built-in. The inheritance chain is `read-only ⊆ write ⊆ maintain ⊆ admin`, so each role is a strict superset of the one below it. The four mutating release operations and `release_create` live in `maintain` (not `write`); the three read-only release operations live in `read-only`. A rule's `operations` field may be either an inline operation list (resolved without role lookup) or a single role name (resolved against the roles table on every `evaluate()`). Role names are stored literally in the rule — redefining a role immediately affects every rule that references it. Loading MUST fail when a rule references an unknown role, when a role definition names an unknown operation, or when a role's operation set is empty.
<!-- /DELTA:CHANGED -->

## Scenarios

<!-- DELTA:CHANGED -->
### Scenario: Built-in roles are available without being declared

* *GIVEN* a policy document with no top-level `roles:` section
* *WHEN* the engine loads the document
* *THEN* the engine MUST make the built-in roles `read-only`, `write`, `maintain`, and `admin` available for reference by rules
* *AND* the `read-only` role MUST expand to exactly `[fetch, clone, pull, pr_review, gh_api_read, release_list, release_view, release_download]`
* *AND* the `write` role MUST expand to the `read-only` operations plus `[push, pr_open, pr_comment, pr_close, pr_merge, issue_open, issue_comment, issue_close]`
* *AND* the `maintain` role MUST expand to the `write` operations plus `[release_create, release_delete, release_edit, release_upload, release_delete_asset]`
* *AND* the `admin` role MUST expand to all twenty-one operations in the fixed vocabulary
<!-- /DELTA:CHANGED -->

<!-- DELTA:NEW -->
### Scenario: maintain role grants a mutating release operation

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: maintain, effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_delete, branch=None)`
* *THEN* the engine MUST resolve the `maintain` role to its operation set at evaluation time
* *AND* the engine MUST return `allow` because `release_delete` is a member of the `maintain` role

### Scenario: maintain role grants release_create after it moves out of admin-only

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: maintain, effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_create, branch=None)`
* *THEN* the engine MUST return `allow` because `release_create` is now a member of the `maintain` role

### Scenario: write role does not grant mutating release operations

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: write, effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_delete, branch=None)`
* *THEN* the engine MUST return `deny` because `release_delete` is not a member of the `write` role

### Scenario: read-only role grants read-only release operations

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: read-only, effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_view, branch=None)`
* *THEN* the engine MUST return `allow` because `release_view` is a member of the `read-only` role
* *AND* the engine MUST return `deny` when the same rule is evaluated for `op=release_delete` because `release_delete` is not a member of `read-only`

### Scenario: admin role remains a superset of maintain

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: admin, effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_upload, branch=None)`
* *THEN* the engine MUST return `allow` because `admin` includes every operation the `maintain` role grants
<!-- /DELTA:NEW -->

# Feature: policy-release-ops

Documents the seven `gh release` lifecycle operations in the policy engine's vocabulary and their evaluation semantics: each is repo-scoped rather than branch-scoped, and denied by default like any other operation absent a matching rule. Split from `policy/policy-engine`; see `policy/policy-roles` for how the built-in `maintain` and `read-only` roles grant these operations.

## Background

The seven release operations — `release_create`, `release_delete`, `release_edit`, `release_upload`, `release_delete_asset`, `release_list`, `release_view`, `release_download` — are repo-scoped, not branch-scoped: `has_branch()` is `false` for all of them, so the `branches` field on a matching rule is ignored even when a release operation such as `release_edit` or `release_create` carries a `--target` branch value on the command line. A mutating release operation (e.g. `release_delete`) does not match a rule whose `operations` list contains only a read release operation (e.g. `release_view`).

## Scenarios

### Scenario: Policy with release lifecycle operations loads successfully

* *GIVEN* a YAML policy file containing a rule with `operations: [release_delete, release_edit, release_upload, release_delete_asset, release_list, release_view, release_download]`
* *WHEN* the engine loads the file
* *THEN* loading MUST succeed without errors
* *AND* the rule's operations list MUST include each of the seven release operations

### Scenario: release_delete is denied by default when no rule grants it

* *GIVEN* a policy with no rule whose operations include `release_delete`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_delete, branch=None)`
* *THEN* the engine MUST return `deny`

### Scenario: release_edit ignores the branch field in a rule

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: [release_edit], branches: ["release/*"], effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_edit, branch=None)`
* *THEN* the engine MUST return `allow`
* *AND* the engine MUST NOT apply the rule's `branches` globs because `release_edit` has no branch concept

### Scenario: A mutating release operation does not match a rule listing only a read release operation

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: [release_view], branches: ["*"], effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=release_delete, branch=None)`
* *THEN* the rule MUST NOT match
* *AND* the engine MUST return `deny`

# Feature: policy-roles

Provides named role resolution for the policy engine, so a rule's `operations` field may reference a role name instead of an inline list, and the role is resolved to its operation set at evaluation time.

## Background

A top-level `roles:` section maps a role name to a non-empty operation list. The built-in roles `read-only` (`[fetch, pull, clone, gh_api_read]`), `write` (`[push, pr_open, pr_comment, pr_review, issue_open, issue_comment]`), and `admin` (all fourteen operations) are available without being declared; a user-defined role of the same name overrides the built-in. A rule's `operations` field may be either an inline operation list (resolved without role lookup) or a single role name (resolved against the roles table on every `evaluate()`). Role names are stored literally in the rule — redefining a role immediately affects every rule that references it. Loading MUST fail when a rule references an unknown role, when a role definition names an unknown operation, or when a role's operation set is empty.

## Scenarios

### Scenario: Built-in roles are available without being declared

* *GIVEN* a policy document with no top-level `roles:` section
* *WHEN* the engine loads the document
* *THEN* the engine MUST make the built-in roles `read-only`, `write`, and `admin` available for reference by rules
* *AND* the `read-only` role MUST expand to exactly `[fetch, pull, clone, gh_api_read]`
* *AND* the `write` role MUST expand to exactly `[push, pr_open, pr_comment, pr_review, issue_open, issue_comment]`
* *AND* the `admin` role MUST expand to all fourteen operations in the fixed vocabulary

### Scenario: Rule references a built-in role in its operations field

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: write, effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=push, branch=main)`
* *THEN* the engine MUST resolve the `write` role to its operation set at evaluation time
* *AND* the engine MUST return `allow` because `push` is a member of the `write` role

### Scenario: Role membership is resolved at evaluation time not write time

* *GIVEN* a policy whose `roles:` section redefines `write` to `[fetch]`
* *AND* a rule `{ user: alice, org: acme, repo: web, operations: write, effect: allow }`
* *WHEN* the engine evaluates `(user=alice, op=push)`
* *THEN* the engine MUST return `deny` because the redefined `write` role no longer contains `push`
* *AND* the rule's stored `operations` field MUST still contain the literal role name `write`, not an expanded operation list

### Scenario: User-defined role extends the role vocabulary

* *GIVEN* a policy with a top-level `roles:` section defining `triager: [issue_open, issue_comment, issue_close]`
* *AND* a rule referencing `operations: triager`
* *WHEN* the engine evaluates an `issue_close` request matching that rule
* *THEN* the engine MUST resolve `triager` to its declared operation set
* *AND* the engine MUST return `allow`

### Scenario: User-defined role may override a built-in role name

* *GIVEN* a policy whose `roles:` section defines `read-only: [fetch]`
* *WHEN* the engine resolves the `read-only` role
* *THEN* the engine MUST use the user-defined `[fetch]` definition
* *AND* the engine MUST NOT use the built-in `read-only` definition

### Scenario: Rule referencing an unknown role fails to load

* *GIVEN* a policy with a rule whose `operations` field is the unknown role name `superuser`
* *AND* no built-in or user-defined role named `superuser` exists
* *WHEN* the engine loads the document
* *THEN* loading MUST fail with a descriptive error mentioning `superuser`
* *AND* the daemon MUST refuse to start

### Scenario: Role definition containing an unknown operation fails to load

* *GIVEN* a policy whose `roles:` section defines `weird: [frobnicate]`
* *WHEN* the engine loads the document
* *THEN* loading MUST fail with a descriptive error mentioning `frobnicate`

### Scenario: A role with an empty operation set fails to load

* *GIVEN* a policy whose `roles:` section defines `empty: []`
* *WHEN* the engine loads the document
* *THEN* loading MUST fail with an error indicating an empty operations list for the role

### Scenario: Role expansion preserves branch semantics for push

* *GIVEN* a rule `{ user: alice, org: acme, repo: web, operations: write, branches: ["release/*"], effect: allow }`
* *WHEN* the engine evaluates a `push` request with `branch=main`
* *THEN* the engine MUST apply the rule's branch globs to the role-resolved `push` operation
* *AND* the engine MUST return `deny` because `main` is outside the `release/*` glob

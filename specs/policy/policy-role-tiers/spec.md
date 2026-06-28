# Feature: policy-role-tiers

Verifies the built-in role tiers' extent at the operation level: `maintain` grants the mutating release operations (and `release_create`, moved out of admin-only), `write` does not reach into release-lifecycle operations, `read-only` grants only the three read release operations, and `admin` remains a strict superset of `maintain`. Split from `policy/policy-roles`.

## Background

See `policy/policy-roles` for the `roles:` section syntax, the built-in role definitions, and the `read-only ⊆ write ⊆ maintain ⊆ admin` inheritance chain. The scenarios here exercise that chain at the release-operation boundaries introduced when `maintain` was added between `write` and `admin`: the four mutating release operations and `release_create` live in `maintain` (not `write`), and the three read-only release operations live in `read-only`.

## Scenarios

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

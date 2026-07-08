# Feature: policy-engine

Loads `/etc/ghbrk/policy.yaml` and evaluates each `(caller, repo, operation, branch)` tuple against the configured rules so the broker has a single authoritative allow/deny decision.

## Background

Rules are evaluated in document order; the first matching rule wins. If no rule matches, the decision is `deny`. A rule has shape:

```yaml
- user: alice            # required; "*" matches any user
  org: acme              # required; "*" matches any org
  repo: web              # required; "*" matches any repo
  operations: [push, pr_open]   # required; non-empty
  branches: [main, "release/*"] # optional; default "*"
  effect: allow          # required; "allow" | "deny"
```

<!-- DELTA:CHANGED -->
The operations vocabulary is fixed: `push`, `fetch`, `clone`, `pull`, `pr_open`, `pr_comment`, `pr_close`, `pr_merge`, `pr_review`, `issue_open`, `issue_comment`, `issue_close`, `release_create`, `release_delete`, `release_edit`, `release_upload`, `release_delete_asset`, `release_list`, `release_view`, `release_download`, `gh_api_read`. Branch matching is glob-style (`*` and `?`) and applies only to operations with `has_branch() == true` (currently `push` only); every release operation is repo-scoped, not branch-scoped, so `has_branch()` is false for all of them and the `branches` field is ignored even when a release operation such as `release_edit` or `release_create` carries a `--target` branch value on the command line. The `pull` operation is treated as distinct from `fetch`: a rule listing one MUST NOT implicitly match the other. The `gh_api_read` operation is user-scoped: it carries no branch and is typically authorised by a rule with `org: "*"` and `repo: "*"`, so org/repo are matched as wildcards. Branch matching is ignored for `gh_api_read` (`has_branch() == false`).
<!-- /DELTA:CHANGED -->

A rule's `operations` field may hold either an inline operation list (as before) or a single role name. Roles are resolved at evaluation time. See `policy/policy-roles` for the full role model and scenarios.

## Scenarios

<!-- DELTA:NEW -->
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
<!-- /DELTA:NEW -->

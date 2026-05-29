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

The operations vocabulary is fixed: `push`, `fetch`, `clone`, `pull`, `pr_open`, `pr_comment`, `pr_close`, `pr_merge`, `pr_review`, `issue_open`, `issue_comment`, `issue_close`, `release_create`, `gh_api_read`. Branch matching is glob-style (`*` and `?`) and applies only to operations with `has_branch() == true` (currently `push` only). The `pull` operation is treated as distinct from `fetch`: a rule listing one MUST NOT implicitly match the other. The `gh_api_read` operation is user-scoped: it carries no branch and is typically authorised by a rule with `org: "*"` and `repo: "*"`, so org/repo are matched as wildcards. Branch matching is ignored for `gh_api_read` (`has_branch() == false`).

## Scenarios

### Scenario: Allow rule matches exact user repo and operation

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: [push], branches: [main], effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=push, branch=main)`
* *THEN* the engine MUST return `allow`

### Scenario: Default deny when no rule matches

* *GIVEN* a policy with one rule scoped to user `alice`
* *WHEN* the engine evaluates a request from user `bob`
* *THEN* the engine MUST return `deny`
* *AND* the deny reason MUST indicate "no matching rule"

### Scenario: First-matching rule wins over later rules

* *GIVEN* a policy with rules in order `[deny acme/web push, allow acme/* push]`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=push, branch=main)`
* *THEN* the engine MUST return `deny`
* *AND* the engine MUST NOT evaluate the second rule's effect

### Scenario: Wildcard user matches any caller

* *GIVEN* a rule with `user: "*"`
* *WHEN* the engine evaluates a request from user `dave`
* *THEN* the rule MUST be considered a candidate match for the user field

### Scenario: Branch glob release wildcard matches release/v1

* *GIVEN* a rule with `branches: ["release/*"]`
* *WHEN* the engine evaluates a request with `branch=release/v1.2`
* *THEN* the engine MUST consider the branch a match

### Scenario: Operation not in rule's operations list does not match

* *GIVEN* a rule with `operations: [push, fetch]`
* *WHEN* the engine evaluates a request with `op=pr_merge`
* *THEN* the rule MUST NOT match
* *AND* evaluation MUST continue with subsequent rules

### Scenario: Policy with unknown operation name fails to load

* *GIVEN* a YAML policy file containing operation `frobnicate`
* *WHEN* the engine loads the file
* *THEN* loading MUST fail with a descriptive error mentioning `frobnicate`
* *AND* the daemon MUST refuse to start

### Scenario: Policy with empty operations list fails to load

* *GIVEN* a YAML policy file with a rule whose `operations` list is empty
* *WHEN* the engine loads the file
* *THEN* loading MUST fail with an error indicating an empty operations list

### Scenario: Operations without a branch concept ignore branch field

* *GIVEN* a rule with `operations: [issue_open]`
* *WHEN* the engine evaluates an `issue_open` request that has no associated branch
* *THEN* the engine MUST evaluate the rule without requiring a branch match

### Scenario: Policy with pull operation loads successfully

* *GIVEN* a YAML policy file containing a rule with `operations: [pull]`
* *WHEN* the engine loads the file
* *THEN* loading MUST succeed without errors
* *AND* the rule's operations list MUST include the `pull` operation

### Scenario: Pull operation is matched independently of fetch

* *GIVEN* a policy with one rule `{ user: alice, org: acme, repo: web, operations: [fetch], branches: ["*"], effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=acme, repo=web, op=pull, branch=None)`
* *THEN* the rule MUST NOT match
* *AND* the engine MUST return `deny`

### Scenario: Pull operation ignores branch field in rule

* *GIVEN* a rule with `operations: [pull]` and `branches: [main]`
* *WHEN* the engine evaluates a `pull` request with no associated branch
* *THEN* the engine MUST evaluate the rule without requiring a branch match

### Scenario: Policy with gh_api_read operation loads successfully

* *GIVEN* a YAML policy file containing a rule with `operations: [gh_api_read]`
* *WHEN* the engine loads the file
* *THEN* loading MUST succeed without errors
* *AND* the rule's operations list MUST include the `gh_api_read` operation

### Scenario: gh_api_read is allowed by a user-scoped wildcard-repo rule

* *GIVEN* a policy with one rule `{ user: alice, org: "*", repo: "*", operations: [gh_api_read], branches: ["*"], effect: allow }`
* *WHEN* the engine evaluates `(user=alice, org=*, repo=*, op=gh_api_read, branch=None)`
* *THEN* the engine MUST return `allow`

### Scenario: gh_api_read ignores branch field in rule

* *GIVEN* a rule with `operations: [gh_api_read]` and `branches: [main]`
* *WHEN* the engine evaluates a `gh_api_read` request with no associated branch
* *THEN* the engine MUST evaluate the rule without requiring a branch match

### Scenario: gh_api_read is denied by default when no rule grants it

* *GIVEN* a policy with one rule scoped to `operations: [push]`
* *WHEN* the engine evaluates a `gh_api_read` request from the same user
* *THEN* the engine MUST return `deny`
* *AND* the deny reason MUST indicate "no matching rule"

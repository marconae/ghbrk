# Feature: policy-operation-families

Documents operation-specific matching quirks in the policy engine: `pull` is evaluated as distinct from `fetch` even though both are read-side git operations, and `gh_api_read` is a user-scoped operation typically authorised by an org/repo-wildcard rule. Split from `policy/policy-engine`, which owns the general rule-matching and vocabulary-loading mechanics.

## Background

The `pull` operation is treated as distinct from `fetch`: a rule listing one MUST NOT implicitly match the other. The `gh_api_read` operation is user-scoped: it carries no branch and is typically authorised by a rule with `org: "*"` and `repo: "*"`, so org/repo are matched as wildcards. Branch matching is ignored for `gh_api_read` (`has_branch() == false`), and for `pull` for the same reason — neither operation carries a branch concept. See `policy/policy-engine` for the fixed operations vocabulary and the general branch-matching rule.

## Scenarios

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

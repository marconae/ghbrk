# Feature: policy-query

Provides a `ghbrk policy <org>/<repo>` subcommand that reports which operations the calling user is permitted to perform on a given repository and which are forbidden, so the effective policy is discoverable without reading `/etc/ghbrk/policy.yaml` (which the caller cannot read) or trial-and-error against the broker.

## Background

`ghbrk policy <org>/<repo>` takes a repository specifier and asks the broker, over the socket, to evaluate every operation in the fixed operations vocabulary (`push`, `fetch`, `clone`, `pull`, `pr_open`, `pr_comment`, `pr_close`, `pr_merge`, `pr_review`, `issue_open`, `issue_comment`, `issue_close`, `release_create`, `gh_api_read`) for the calling user against that repo. The broker identifies the caller via `SO_PEERCRED` and evaluates each operation under the same first-match-wins, default-deny rules the policy engine uses for live requests. The command groups the results into allowed and forbidden operations and prints them. No git/gh process is executed and nothing leaves the machine. Branch-scoped operations are evaluated at the operation level for this summary.

## Scenarios

### Scenario: Allowed operations are listed for a repo

* *GIVEN* the policy allows the calling user to `push` and `pr_open` on `acme/web`
* *WHEN* the user runs `ghbrk policy acme/web`
* *THEN* the command MUST list `push` and `pr_open` under the allowed operations
* *AND* the command MUST exit with status zero

### Scenario: Forbidden operations are listed for a repo

* *GIVEN* the policy allows the calling user only `push` on `acme/web`
* *WHEN* the user runs `ghbrk policy acme/web`
* *THEN* the command MUST list the operations that are not allowed (e.g. `pr_merge`, `issue_close`) under the forbidden operations

### Scenario: Repo with no matching rule returns all-forbidden by default

* *GIVEN* the policy has no rule matching the calling user for `other/unknown`
* *WHEN* the user runs `ghbrk policy other/unknown`
* *THEN* the command MUST report that no operations are allowed
* *AND* the command MUST report that every operation is forbidden by the default-deny rule

### Scenario: Malformed repo specifier is rejected

* *GIVEN* the binary is installed
* *WHEN* the user runs `ghbrk policy not-a-valid-spec`
* *THEN* the command MUST print an error describing the expected `<org>/<repo>` format to stderr
* *AND* the command MUST exit with a non-zero status

### Scenario: Daemon unreachable is reported

* *GIVEN* no broker is listening on `/var/run/ghbrk/broker.sock`
* *WHEN* the user runs `ghbrk policy acme/web`
* *THEN* the command MUST print an error indicating the broker is unavailable to stderr
* *AND* the command MUST exit with a non-zero status

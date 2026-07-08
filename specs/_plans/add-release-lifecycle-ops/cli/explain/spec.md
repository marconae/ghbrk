# Feature: explain

Provides a `ghbrk explain <command>` subcommand that tells the user exactly what ghbrk would do with a given git/gh command — whether it is a brokered remote operation, what policy decision it would receive, and whether a credential would be injected — without actually executing it, so the privilege boundary is inspectable rather than invisible.

## Background

`ghbrk explain` takes a full command line (e.g. `git push origin main` or `gh pr create`) as trailing arguments and performs a dry run: it classifies the command, resolves the `(operation, org, repo, branch?)` tuple, evaluates the policy for the calling user, and reports whether a credential would be injected — but it MUST NOT execute git/gh or perform any operation that leaves the machine. Resolution and policy evaluation that require the caller's credentials and policy view are performed by the broker over the socket; the broker returns a structured explanation. Local-only git subcommands are recognised and reported as out of ghbrk's scope. Commands that map to no known operation are reported as unknown.

## Scenarios

<!-- DELTA:NEW -->
### Scenario: Known remote gh release operation shows policy outcome and token injection

* *GIVEN* the calling user is allowed the `maintain` role on `acme/web`
* *WHEN* the user runs `ghbrk explain gh release delete v1.2.0 --yes` from a clone of `acme/web`
* *THEN* the command MUST report the resolved operation as `release_delete` on `acme/web`
* *AND* the command MUST report the policy decision as `allow`
* *AND* the command MUST report that the GitHub token would be injected
* *AND* the command MUST NOT execute gh

### Scenario: gh release delete no longer reports a resolver error

* *GIVEN* the binary is installed
* *WHEN* the user runs `ghbrk explain gh release delete v1.2.0`
* *THEN* the command MUST report the resolved operation as `release_delete`
* *AND* the command MUST NOT report that the `release delete` subcommand is unsupported or unknown
<!-- /DELTA:NEW -->

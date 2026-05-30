# Feature: explain

Provides a `ghbrk explain <command>` subcommand that tells the user exactly what ghbrk would do with a given git/gh command — whether it is a brokered remote operation, what policy decision it would receive, and whether a credential would be injected — without actually executing it, so the privilege boundary is inspectable rather than invisible.

## Background

`ghbrk explain` takes a full command line (e.g. `git push origin main` or `gh pr create`) as trailing arguments and performs a dry run: it classifies the command, resolves the `(operation, org, repo, branch?)` tuple, evaluates the policy for the calling user, and reports whether a credential would be injected — but it MUST NOT execute git/gh or perform any operation that leaves the machine. Resolution and policy evaluation that require the caller's credentials and policy view are performed by the broker over the socket; the broker returns a structured explanation. Local-only git subcommands are recognised and reported as out of ghbrk's scope. Commands that map to no known operation are reported as unknown.

## Scenarios

### Scenario: Known remote git operation shows policy outcome and credential injection

* *GIVEN* the calling user is allowed to push to `acme/web` on branch `main`
* *WHEN* the user runs `ghbrk explain git push origin main` from a clone of `acme/web`
* *THEN* the command MUST report the resolved operation as `push` on `acme/web` branch `main`
* *AND* the command MUST report the policy decision as `allow`
* *AND* the command MUST report that the SSH credential would be injected
* *AND* the command MUST NOT execute git

### Scenario: Known remote git operation that policy denies shows the denial

* *GIVEN* the calling user is not allowed to push to `acme/web`
* *WHEN* the user runs `ghbrk explain git push origin main` from a clone of `acme/web`
* *THEN* the command MUST report the resolved operation as `push` on `acme/web`
* *AND* the command MUST report the policy decision as `deny`
* *AND* the command MUST NOT execute git

### Scenario: Known remote gh operation shows policy outcome and token injection

* *GIVEN* the calling user is allowed to open pull requests on `acme/web`
* *WHEN* the user runs `ghbrk explain gh pr create --title foo` from a clone of `acme/web`
* *THEN* the command MUST report the resolved operation as `pr_open` on `acme/web`
* *AND* the command MUST report the policy decision as `allow`
* *AND* the command MUST report that the GitHub token would be injected
* *AND* the command MUST NOT execute gh

### Scenario: Local-only git operation shows out-of-scope guidance

* *GIVEN* the binary is installed
* *WHEN* the user runs `ghbrk explain git status`
* *THEN* the command MUST report that `git status` is a local operation outside ghbrk's scope
* *AND* the command MUST advise running `git status` directly
* *AND* the command MUST report that no credential would be injected and no policy is evaluated

### Scenario: Unknown command is reported as unknown

* *GIVEN* the binary is installed
* *WHEN* the user runs `ghbrk explain git frobnicate`
* *THEN* the command MUST report the operation as `unknown`
* *AND* the command MUST report that the broker would deny the request by default
* *AND* the command MUST NOT execute git

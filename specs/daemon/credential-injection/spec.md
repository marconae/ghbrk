# Feature: credential-injection

Selects and injects the correct stored credential (HTTPS token or GH API token) into the child process environment for each resolved request, so that the agent never sees the raw credential while the executed git/gh command still authenticates as the broker-managed user. SSH key escrow is handled separately in `daemon/ssh-agent-escrow`.

## Background

Credentials live under `/etc/ghbrk/credentials/<user>/` owned by the `ghbrk` user with mode `0600`. The token file is `token` (HTTPS / GH API token). The daemon process must have read access; the calling user's processes must not — the token file is not readable nor reachable by the calling user. The token value is injected via `GH_TOKEN` env vars read by the daemon from the token file. For any `gh` invocation the injector sets `GH_TOKEN` from the token file contents — this applies both to policy-gated broker operations (`gh api`, `gh pr`, `gh issue`, `gh release`) and to passthrough `gh` invocations (e.g. `gh repo view`, `gh auth status`) that skip policy evaluation but are still executed by the broker so the token can be supplied.

## Scenarios

### Scenario: HTTPS URL selects token injection for git

* *GIVEN* the resolved URL is `https://github.com/acme/web.git`
* *AND* the file `/etc/ghbrk/credentials/alice/token` exists with mode `0600`
* *WHEN* the injector prepares the child environment for user `alice` invoking `git`
* *THEN* the environment MUST set a credential helper or `GIT_ASKPASS` value that supplies the token to git
* *AND* the token contents MUST NOT appear on the child's argv

### Scenario: gh CLI receives GH_TOKEN

* *GIVEN* the request tool is `gh`
* *AND* `/etc/ghbrk/credentials/alice/token` exists
* *WHEN* the injector prepares the child environment
* *THEN* the environment MUST set `GH_TOKEN` to the token file contents

### Scenario: HTTPS URL with missing token returns explicit error

* *GIVEN* the resolved URL is `https://github.com/acme/web.git`
* *AND* `/etc/ghbrk/credentials/alice/token` does not exist
* *WHEN* the injector prepares the child environment
* *THEN* the injector MUST return an error indicating the token is missing for user `alice`

### Scenario: Credential file with permissive mode is rejected

* *GIVEN* `/etc/ghbrk/credentials/alice/id_rsa` exists with mode `0644` owned by `ghbrk:ghbrk`
* *WHEN* the injector prepares the child environment
* *THEN* the injector MUST return an error indicating the credential mode is too permissive
* *AND* the injector MUST NOT load the key into an agent
* *AND* the injector MUST NOT pass the path to the child

### Scenario: Token contents are not logged

* *GIVEN* tracing is configured at debug level
* *WHEN* the injector prepares a child environment containing a token
* *THEN* the token contents MUST NOT appear in any tracing event

### Scenario: gh api request receives GH_TOKEN

* *GIVEN* the request tool is `gh` for a `gh_api_read` operation
* *AND* the token file exists for the caller with mode `0600`
* *WHEN* the injector prepares the child environment
* *THEN* the environment MUST set `GH_TOKEN` to the token file contents
* *AND* the token contents MUST NOT appear on the child's argv

### Scenario: Passthrough gh invocation receives GH_TOKEN

* *GIVEN* the request tool is `gh` with args `["repo", "view"]` that do not match any broker operation
* *AND* `/etc/ghbrk/credentials/alice/token` exists with mode `0600`
* *WHEN* the broker handles the passthrough invocation for user `alice`
* *THEN* the broker MUST skip policy evaluation for the invocation
* *AND* the environment MUST set `GH_TOKEN` to the token file contents
* *AND* the broker MUST execute the real `gh` binary with the original arguments
* *AND* the token contents MUST NOT appear on the child's argv

### Scenario: Token file is not readable by the calling user

* *GIVEN* `/etc/ghbrk/credentials/alice/token` exists with mode `0600` owned by `ghbrk:ghbrk`
* *AND* the calling Unix user `alice` is not `ghbrk` and is not in the `ghbrk` group
* *WHEN* a process running as `alice` attempts to open `token` for reading
* *THEN* the open MUST fail with `EACCES`
* *AND* the daemon (running as `ghbrk`) MUST still be able to read `token` to inject `GH_TOKEN`

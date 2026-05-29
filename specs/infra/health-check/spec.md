# Feature: health-check

Provides a `ghbrk check` subcommand that lets an operator or agent verify, in one command, that the current Unix user's broker credentials are present, correctly permissioned, and accepted by GitHub, so that misconfiguration is caught before a real `git`/`gh` operation fails opaquely.

## Background

`ghbrk check` runs as the invoking Unix user but does NOT read the credential directory itself. The credential directory `/etc/ghbrk/credentials/<user>/` is owned by the `ghbrk` system user with mode `0700`, so a normal agent user cannot `stat()` or read its contents. Instead, the `check` shim connects to the broker socket and sends a `Request { tool: check, args: [], cwd }`. The broker identifies the caller via `SO_PEERCRED`, then performs all credential checks as the `ghbrk` user against that caller's credential directory: it verifies `id_rsa` (SSH private key) and `token` (GitHub API token) each exist with mode `0600`, then pings the GitHub REST API (`GET https://api.github.com/user`) using the token and reports the authenticated login. The broker streams one human-readable status line per check back to the shim as `StdoutChunk` frames, then sends an `Exit { code }` frame: code `0` when all checks pass, non-zero when any check fails. The shim prints the streamed lines and exits with the returned code. No policy evaluation is performed for `check` — it inspects only the caller's own credentials and never executes a git/gh operation.

## Scenarios

### Scenario: SSH key present with correct mode reports OK

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/id_rsa` exists with mode `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `SSH key: OK`

### Scenario: SSH key missing is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/id_rsa` does not exist
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `SSH key: MISSING` that names the expected path
* *AND* the command MUST exit with a non-zero status

### Scenario: SSH key with permissive mode is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/id_rsa` exists with a mode more permissive than `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `SSH key: BAD PERMISSIONS` that names the actual mode
* *AND* the command MUST exit with a non-zero status

### Scenario: Token present with correct mode reports OK

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/token` exists with mode `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `Token: OK`

### Scenario: Token missing is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/token` does not exist
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `Token: MISSING` that names the expected path
* *AND* the command MUST exit with a non-zero status

### Scenario: Token with permissive mode is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/token` exists with a mode more permissive than `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `Token: BAD PERMISSIONS` that names the actual mode
* *AND* the command MUST exit with a non-zero status

### Scenario: GitHub API ping succeeds reports login

* *GIVEN* a valid token exists with mode `0600`
* *AND* the GitHub API `GET /user` returns HTTP 200 with a JSON body containing a `login` field
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `GitHub API: OK` that includes the authenticated `login`

### Scenario: GitHub API ping with invalid token is reported

* *GIVEN* a token exists with mode `0600`
* *AND* the GitHub API `GET /user` returns HTTP 401
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `GitHub API: INVALID TOKEN`
* *AND* the command MUST exit with a non-zero status

### Scenario: GitHub API ping with network failure is reported

* *GIVEN* a token exists with mode `0600`
* *AND* the GitHub API host is unreachable (transport-level error)
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `GitHub API: UNREACHABLE`
* *AND* the command MUST exit with a non-zero status

### Scenario: All checks passing exits zero

* *GIVEN* the SSH key and token both exist with mode `0600`
* *AND* the GitHub API `GET /user` returns HTTP 200
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST exit with status zero

### Scenario: Any failing check exits non-zero

* *GIVEN* at least one of the credential or GitHub API checks fails
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST exit with a non-zero status
* *AND* the command MUST still print a status line for every check that was attempted

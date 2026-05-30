# Feature: health-check

Provides a `ghbrk check` subcommand that verifies the caller's broker credentials. This standalone subcommand is removed; its credential checks are absorbed into the new `ghbrk doctor` command (see infra/doctor), which also checks daemon reachability and policy parseability.

## Background

`ghbrk check` ran as the invoking user and asked the broker to inspect the caller's `id_rsa` and `token` and ping the GitHub API. The broker-side credential logic and the `Tool::Check` wire discriminant are retained and reused by `doctor`; only the standalone `check` subcommand and its scenarios are removed.

## Scenarios

<!-- DELTA:REMOVED -->
### Scenario: SSH key present with correct mode reports OK

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/id_rsa` exists with mode `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `SSH key: OK`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: SSH key missing is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/id_rsa` does not exist
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `SSH key: MISSING` that names the expected path
* *AND* the command MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: SSH key with permissive mode is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/id_rsa` exists with a mode more permissive than `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `SSH key: BAD PERMISSIONS` that names the actual mode
* *AND* the command MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Token present with correct mode reports OK

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/token` exists with mode `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `Token: OK`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Token missing is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/token` does not exist
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `Token: MISSING` that names the expected path
* *AND* the command MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Token with permissive mode is reported and fails

* *GIVEN* the file `/etc/ghbrk/credentials/<user>/token` exists with a mode more permissive than `0600`
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `Token: BAD PERMISSIONS` that names the actual mode
* *AND* the command MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: GitHub API ping succeeds reports login

* *GIVEN* a valid token exists with mode `0600`
* *AND* the GitHub API `GET /user` returns HTTP 200 with a JSON body containing a `login` field
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `GitHub API: OK` that includes the authenticated `login`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: GitHub API ping with invalid token is reported

* *GIVEN* a token exists with mode `0600`
* *AND* the GitHub API `GET /user` returns HTTP 401
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `GitHub API: INVALID TOKEN`
* *AND* the command MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: GitHub API ping with network failure is reported

* *GIVEN* a token exists with mode `0600`
* *AND* the GitHub API host is unreachable (transport-level error)
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST print a line reporting `GitHub API: UNREACHABLE`
* *AND* the command MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: All checks passing exits zero

* *GIVEN* the SSH key and token both exist with mode `0600`
* *AND* the GitHub API `GET /user` returns HTTP 200
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST exit with status zero
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Any failing check exits non-zero

* *GIVEN* at least one of the credential or GitHub API checks fails
* *WHEN* the user runs `ghbrk check`
* *THEN* the command MUST exit with a non-zero status
* *AND* the command MUST still print a status line for every check that was attempted
<!-- /DELTA:REMOVED -->

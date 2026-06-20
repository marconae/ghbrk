# Feature: doctor

Provides a `ghbrk doctor` subcommand that verifies, in one command, that the local ghbrk environment is healthy — the broker daemon is reachable, the caller's credentials are present and correctly permissioned, and the policy file parses cleanly — so misconfiguration is surfaced explicitly before a real brokered operation fails opaquely.

## Background

`ghbrk doctor` runs as the invoking Unix user and makes the privilege boundary explicit: it reports the state of each precondition rather than hiding it. It subsumes the former `ghbrk check` credential checks. Because the credential directory `/etc/ghbrk/credentials/<user>/` is owned by the `ghbrk` system user (mode `0700`), the caller cannot stat it directly; credential checks are performed by the broker on the caller's behalf over the socket and the results are streamed back. The daemon-reachability check connects to `/var/run/ghbrk/broker.sock`. The policy-parse check confirms that `/etc/ghbrk/policy.yaml` deserialises under the policy engine schema. `doctor` prints one human-readable status line per check and exits zero only when no check emitted an ERROR; warnings are tolerated.

## Scenarios

### Scenario: Daemon socket reachable reports OK

* *GIVEN* the broker daemon is running and listening on `/var/run/ghbrk/broker.sock`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Daemon: OK`

### Scenario: Daemon socket missing is reported and fails

* *GIVEN* no socket exists at `/var/run/ghbrk/broker.sock`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Daemon: UNREACHABLE` that names the socket path
* *AND* the command MUST exit with a non-zero status

### Scenario: Daemon socket present but no listener is reported and fails

* *GIVEN* the socket file `/var/run/ghbrk/broker.sock` exists but no process is listening
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Daemon: UNREACHABLE`
* *AND* the command MUST exit with a non-zero status

### Scenario: Credentials present with correct mode report OK

* *GIVEN* the broker is reachable
* *AND* the caller's `id_rsa` and `token` both exist with mode `0600`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Credentials: OK`

### Scenario: Missing credential is reported and fails

* *GIVEN* the broker is reachable
* *AND* the caller's `token` credential does not exist
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting the missing credential and the expected path
* *AND* the command MUST exit with a non-zero status

### Scenario: Credential with permissive mode is reported and fails

* *GIVEN* the broker is reachable
* *AND* the caller's `id_rsa` exists with a mode more permissive than `0600`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting bad permissions that names the actual mode
* *AND* the command MUST exit with a non-zero status

### Scenario: Policy file parses cleanly reports OK

* *GIVEN* `/etc/ghbrk/policy.yaml` exists and deserialises under the policy engine schema
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Policy: OK`

### Scenario: Malformed policy file is reported and fails

* *GIVEN* `/etc/ghbrk/policy.yaml` contains content that is not valid for the policy schema
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Policy: INVALID` that names the parse error
* *AND* the command MUST exit with a non-zero status

### Scenario: Policy file writable by group or other is reported and fails

* *GIVEN* `/etc/ghbrk/policy.yaml` is owned by `ghbrk` but has a group or other write bit set (for example `0660` or `0666`)
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Policy permissions: ERROR` that names the actual mode found
* *AND* the command MUST exit with a non-zero status

### Scenario: Policy file owned by wrong user is reported and fails

* *GIVEN* `/etc/ghbrk/policy.yaml` has mode `0600` but is owned by a user other than `ghbrk`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Policy permissions: ERROR` that names the actual owner found
* *AND* the command MUST exit with a non-zero status

### Scenario: All checks passing exits zero

* *GIVEN* the daemon is reachable, the credentials are present with mode `0600`, and every audited file and directory — `/etc/ghbrk/`, `/etc/ghbrk/policy.yaml`, `/run/ghbrk/ghbrk.sock`, the credential directory, and the credential files — has its expected owner and a mode no broader than its expectation
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print one status line per check that was attempted, each tagged `OK`
* *AND* the command MUST exit with status zero

### Scenario: Policy file not readable by invoking user is silently skipped

* *GIVEN* `/etc/ghbrk/policy.yaml` is owned by `ghbrk` with mode `0600` and the invoking user is not `ghbrk`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST NOT print any `Policy:` parse line
* *AND* the command MUST exit with status zero (the `Policy permissions: OK` line already confirms the file exists and is correctly locked down)

### Scenario: Warnings without errors still exit zero

* *GIVEN* one or more checks emit a `WARNING` (a read-path exposure such as a `0640` policy file or credential) and no check emits an `ERROR`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a `WARNING` status line for each read-path exposure
* *AND* the command MUST exit with status zero

### Scenario: Any check emitting an error exits non-zero

* *GIVEN* at least one check — daemon, credential-mode, policy-parse, policy-permission, config-dir-permission, socket-permission, credential-dir-permission, or credential-file-permission — emits an `ERROR`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST exit with a non-zero status
* *AND* the command MUST still print a status line for every check that was attempted
* *AND* the presence of `WARNING` lines MUST NOT by itself change the exit status

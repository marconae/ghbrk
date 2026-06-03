# Feature: doctor-permissions

Extends `ghbrk doctor` with a tiered OS-level permission audit over every security-relevant file and directory `ghbrk` manages, so a misconfigured installation (world-writable policy file, wrong socket mode, permissive credential directory) is surfaced explicitly and actionably.

## Background

The permission audit complements the existing parse and credential checks. Two classes of file exist. Files and directories the invoking user can `stat()` directly — `/etc/ghbrk/`, `/etc/ghbrk/policy.yaml`, and `/run/ghbrk/ghbrk.sock` — are checked locally, because `/etc/ghbrk/` and `/run/ghbrk/` are world-traversable (mode `0755`). Files under `/var/lib/ghbrk/` (the credential root, mode `0700` owned by `ghbrk`) cannot be stat'd by the caller, so their owner/mode are reported by the broker over the socket and the results are streamed back, reusing the existing broker-proxied credential mechanism.

The verdict tiering is uniform across every checked path. A write-path exposure means a user who is neither `root` nor `ghbrk` could overwrite the file (group/other write bit set, a directory whose group/other write or execute bit grants traversal that permits replacing children, an owner other than the expected one, or a socket mode such as `0666`/`0777` that lets an unauthorised user connect and issue requests). A write-path exposure is reported as `ERROR` and forces a non-zero exit. A read-path exposure means a non-authorised user could read sensitive content but not write it (for example a policy or credential file at mode `0640`/`0644`, or a credential directory whose group/other read bit is set without the write/execute bit). A read-path exposure is reported as `WARNING` and does not, on its own, change the exit status.

## Scenarios

### Scenario: Policy file has correct owner and mode reports OK

* *GIVEN* `/etc/ghbrk/policy.yaml` exists, is owned by the `ghbrk` user, and has mode `0600`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Policy permissions: OK`

### Scenario: Policy file readable by group or other warns but does not fail

* *GIVEN* `/etc/ghbrk/policy.yaml` is owned by `ghbrk` and has no group or other write bit, but has a group or other read bit set (for example `0640` or `0644`)
* *WHEN* the user runs `ghbrk doctor`
* *AND* no other check emits an ERROR
* *THEN* the command MUST print a line reporting `Policy permissions: WARNING` that names the actual mode found
* *AND* the command MUST exit with status zero

### Scenario: Config directory world-traversable reports OK

* *GIVEN* `/etc/ghbrk/` exists, is owned by `root`, and has mode `0755`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Config dir permissions: OK`

### Scenario: World-writable config directory is reported and fails

* *GIVEN* `/etc/ghbrk/` is owned by `root` but has a group or other write bit set (for example `0777` or `0775`)
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Config dir permissions: ERROR` that names the actual mode found
* *AND* the command MUST exit with a non-zero status

### Scenario: Socket with restricted mode reports OK

* *GIVEN* `/run/ghbrk/ghbrk.sock` exists, is owned by `ghbrk`, has group `ghbrk-clients`, and has mode `0660`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Socket permissions: OK`

### Scenario: World-connectable socket is reported and fails

* *GIVEN* `/run/ghbrk/ghbrk.sock` has a mode that permits connections from users outside the `ghbrk-clients` group (for example `0666` or `0777`)
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a line reporting `Socket permissions: ERROR` that names the actual mode found
* *AND* the command MUST exit with a non-zero status

### Scenario: Socket owned by an unexpected user warns but does not fail

* *GIVEN* `/run/ghbrk/ghbrk.sock` has a restricted mode (no broader than `0660`) but is owned by a user other than `ghbrk`
* *WHEN* the user runs `ghbrk doctor`
* *AND* no other check emits an ERROR
* *THEN* the command MUST print a line reporting `Socket permissions: WARNING` that names the actual owner found
* *AND* the command MUST exit with status zero

### Scenario: Credential directory with correct mode reports OK

* *GIVEN* the broker is reachable
* *AND* the caller's credential directory `/var/lib/ghbrk/credentials/<user>/` is owned by `ghbrk` and has mode `0700`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the broker MUST report the directory owner and mode over the socket
* *AND* the command MUST print a line reporting `Credential dir permissions: OK`

### Scenario: Credential directory writable or traversable by group or other is reported and fails

* *GIVEN* the broker is reachable
* *AND* the caller's credential directory `/var/lib/ghbrk/credentials/<user>/` has a group or other write or execute bit set (for example `0770` or `0711`)
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the broker MUST report the directory owner and mode over the socket
* *AND* the command MUST print a line reporting `Credential dir permissions: ERROR` that names the actual mode found
* *AND* the command MUST exit with a non-zero status

### Scenario: Credential directory readable by group or other warns but does not fail

* *GIVEN* the broker is reachable
* *AND* the caller's credential directory `/var/lib/ghbrk/credentials/<user>/` has a group or other read bit set but no group or other write or execute bit (for example `0740`)
* *WHEN* the user runs `ghbrk doctor` and no other check emits an ERROR
* *THEN* the broker MUST report the directory owner and mode over the socket
* *AND* the command MUST print a line reporting `Credential dir permissions: WARNING` that names the actual mode found
* *AND* the command MUST exit with status zero

### Scenario: Credential file writable by group or other is reported and fails

* *GIVEN* the broker is reachable
* *AND* the caller's `gh-token` or `id_ed25519` has a group or other write bit set (for example `0660` or `0620`)
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the broker MUST report the credential owner and mode over the socket
* *AND* the command MUST print a line reporting bad permissions as `ERROR` that names the credential and the actual mode found
* *AND* the command MUST exit with a non-zero status

### Scenario: Credential file readable by group or other warns but does not fail

* *GIVEN* the broker is reachable
* *AND* the caller's `gh-token` or `id_ed25519` has a group or other read bit set but no group or other write bit (for example `0640`)
* *WHEN* the user runs `ghbrk doctor` and no other check emits an ERROR
* *THEN* the broker MUST report the credential owner and mode over the socket
* *AND* the command MUST print a line reporting `WARNING` that names the credential and the actual mode found
* *AND* the command MUST exit with status zero

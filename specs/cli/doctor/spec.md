# Feature: doctor

Provides a `ghbrk doctor` subcommand that verifies, in one command, that the local ghbrk environment is healthy — the broker daemon is reachable, the caller's credentials are present and correctly permissioned, and the policy file parses cleanly — so misconfiguration is surfaced explicitly before a real brokered operation fails opaquely.

## Background

`ghbrk doctor` runs as the invoking Unix user and makes the privilege boundary explicit: it reports the state of each precondition rather than hiding it. It subsumes the former `ghbrk check` credential checks. Because the credential directory `/etc/ghbrk/credentials/<user>/` is owned by the `ghbrk` system user (mode `0700`), the caller cannot stat it directly; credential checks are performed by the broker on the caller's behalf over the socket and the results are streamed back. The daemon-reachability check connects to `/var/run/ghbrk/broker.sock`. The policy-parse check confirms that `/etc/ghbrk/policy.yaml` deserialises under the policy engine schema. `doctor` prints one human-readable status line per check and exits zero only when no check emitted an ERROR; warnings are tolerated.

One further precondition is invisible from either side alone: the daemon and the caller must see the same filesystem. Every brokered command that names a file — `gh release create v1.0.0 /tmp/app.tar.gz`, `gh pr comment --body-file /tmp/body.md` — is executed by a child of the daemon, and that child opens the path itself. When the daemon runs inside a mount namespace of its own, the path the caller can list does not exist for the child, and `gh` reports `no such file or directory` for a file that demonstrably exists. The most common cause is a `PrivateTmp=` directive in an installed unit that predates `infra/systemd-unit`'s requirement to omit it.

`doctor` turns that into a named check whose evidence is an identity, not a file exchange. `doctor` stats `/tmp` in its own namespace and puts the resulting device and inode pair in the `check` request as the caller `/tmp` identity. The broker stats `/tmp` in its own namespace and compares the two pairs. Equal pairs prove both processes resolve `/tmp` to one directory, so every path under it names the same file for both; unequal pairs prove they do not. The broker emits one `Shared filesystem:` status line, which contributes to the pass/fail result exactly as every other broker-side check does.

The broker inspects only its own `/tmp`. It resolves no caller-supplied path, opens no caller-named file, and creates and deletes nothing. The caller contributes two integers, and the broker compares them against a value it derived itself rather than using them to reach a file. `stat("/tmp")` runs on a fixed, root-owned path: no symlink a local user plants can redirect it, no FIFO can block the calling thread on it, and it opens no time-of-check/time-of-use window. The broker discloses its own pair in the mismatch line, so the comparison is not an oracle over a value the caller could not otherwise learn.

The check reports the property it establishes and no more: the two processes resolve `/tmp` to the same directory. It states nothing about whether the service account can read any one file there, which is a per-file permission question rather than the host misconfiguration this check exists to name.

On a mismatch the broker names the cause from its own view. It reads `/proc/self/mountinfo`, takes the last entry whose mount point is `/tmp`, and reports that entry's mount root when the root is not `/`. `PrivateTmp=true` makes systemd bind-mount `/tmp/systemd-private-<machine-id>-<unit>-<random>/tmp` onto `/tmp` inside the unit's namespace, so the mount point stays `/tmp` and the mount root is the only field recording the substitution. A root beginning `/systemd-private-` names the directive outright. Resolving `/tmp` with `realpath` would not: inside the namespace it yields `/tmp`.

A version-skewed pair degrades to silence rather than to a false verdict. The broker emits the status line only when the request carries a caller `/tmp` identity, so a client released before this change gets the credential checks and nothing else. A daemon released before this change emits no such line at all; `doctor` then reports the check as unsupported by the running daemon and leaves its exit status unchanged, because an absent line proves nothing about the filesystem either way. `doctor` draws that conclusion only from a request that carried an identity: if it cannot stat its own `/tmp`, it sends none, prints that reason once, and leaves the exit status unchanged there too.

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

* *GIVEN* the daemon is reachable, the credentials are present with mode `0600`, the daemon and the caller resolve `/tmp` to the same directory, and every audited file and directory — `/etc/ghbrk/`, `/etc/ghbrk/policy.yaml`, `/run/ghbrk/ghbrk.sock`, the credential directory, and the credential files — has its expected owner and a mode no broader than its expectation
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

* *GIVEN* at least one check — daemon, credential-mode, policy-parse, policy-permission, config-dir-permission, socket-permission, credential-dir-permission, credential-file-permission, or shared-filesystem — emits an `ERROR`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST exit with a non-zero status
* *AND* the command MUST still print a status line for every check that was attempted
* *AND* the presence of `WARNING` lines MUST NOT by itself change the exit status

### Scenario: Daemon sharing the caller's /tmp reports OK

* *GIVEN* the broker is reachable
* *AND* the daemon runs without a private `/tmp` mount namespace
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST send the device and inode of its own `/tmp` in the `check` request
* *AND* the command MUST print a line reporting `Shared filesystem: OK`
* *AND* the command MUST NOT create any file under `/tmp`

### Scenario: Daemon with a private /tmp is reported with the directive to remove

* *GIVEN* the broker is reachable and the daemon runs with `PrivateTmp=true`, so its `/tmp` is a directory of its own
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the command MUST print a `Shared filesystem: ERROR` line reporting that the daemon's `/tmp` is not the caller's `/tmp`
* *AND* the line MUST carry both device and inode pairs and MUST name the `PrivateTmp=` directive as the cause
* *AND* the line MUST give removing that directive from `/etc/systemd/system/ghbrk.service` and restarting the service as the remediation, and MUST NOT give reinstalling, because an installer that still emits the directive reproduces the fault
* *AND* the command MUST exit with a non-zero status

### Scenario: Broker names the mount root it reads from its own mountinfo

* *GIVEN* a `/proc/self/mountinfo` whose last entry with mount point `/tmp` carries the mount root `/systemd-private-9302792eb91a42bbbeef83f282d9bd85-ghbrk.service-ApAiqY/tmp`
* *WHEN* the broker derives the cause of a `/tmp` identity mismatch from that content
* *THEN* the broker MUST report that mount root
* *AND* the broker MUST report no mount root when the last entry with mount point `/tmp` carries the root `/`
* *AND* the broker MUST derive the cause from its own mountinfo alone, taking no input from the caller

### Scenario: Check request carrying no caller /tmp identity omits the shared-filesystem line

* *GIVEN* a client released before this change sends a `check` request carrying no caller `/tmp` identity
* *WHEN* the broker runs the checks
* *THEN* the broker MUST run every credential check as before
* *AND* the broker MUST NOT print a `Shared filesystem:` line
* *AND* the broker MUST NOT fail the check for the absent identity

### Scenario: Daemon that omits the shared-filesystem line leaves the exit status unchanged

* *GIVEN* a daemon released before this change, which ignores the caller `/tmp` identity and returns no `Shared filesystem:` line
* *WHEN* the user runs `ghbrk doctor` against that daemon
* *THEN* the command MUST print a line reporting the shared-filesystem check as unsupported by the running daemon
* *AND* the absent line MUST NOT change the command's exit status
* *AND* the command MUST report every other check exactly as it does against a current daemon

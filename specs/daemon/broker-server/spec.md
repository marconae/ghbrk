# Feature: broker-server

Provides the `ghbrk daemon` Unix socket server that accepts shim connections, identifies callers via SO_PEERCRED, and orchestrates per-request policy evaluation and execution.

## Background

The daemon binds `/var/run/ghbrk/broker.sock` with mode `0660` and group `ghbrk-clients`. The supported deployment sets the daemon's primary group to `ghbrk-clients` via the systemd unit's `Group=ghbrk-clients` directive, so the socket inherits the correct group on `bind(2)` without requiring a runtime `chown`. A defence-in-depth `chown` remains for daemons started outside systemd or with a non-standard `Group=`; when that chown fails, the daemon logs at `error` level with diagnostic guidance. Linux only — peer credential reading uses `SO_PEERCRED`. Each accepted connection is handled by an independent Tokio task. The daemon must remain running across malformed-request errors and child process failures; it only exits on SIGINT, SIGTERM, or fatal bind errors.

## Scenarios

### Scenario: Daemon binds socket with correct permissions

* *GIVEN* the directory `/var/run/ghbrk/` exists and the daemon has write access
* *WHEN* the daemon starts
* *THEN* the daemon MUST create `/var/run/ghbrk/broker.sock`
* *AND* the socket file mode MUST be `0660`
* *AND* the socket file group MUST be `ghbrk-clients` when that group exists
* *AND* when the daemon's primary group is already `ghbrk-clients`, the socket MUST inherit that group on `bind(2)` without requiring a subsequent `chown` for correctness
* *AND* when the daemon's primary group is not `ghbrk-clients`, the daemon MUST attempt to `chown` the socket to the `ghbrk-clients` group as a defence-in-depth check
* *AND* if that defence-in-depth `chown` fails, the daemon MUST log the failure at `error` level (not `warn`)
* *AND* the error message MUST name the systemd `Group=ghbrk-clients` directive in `deploy/linux/ghbrk.service` as the supported fix so an operator reading `journalctl -u ghbrk` can locate the misconfiguration without consulting the source

### Scenario: Daemon refuses to start when socket path already exists with active listener

* *GIVEN* another process is listening on `/var/run/ghbrk/broker.sock`
* *WHEN* the daemon starts
* *THEN* the daemon MUST print a fatal error
* *AND* the daemon MUST exit with a non-zero status

### Scenario: Daemon resolves caller UID via SO_PEERCRED

* *GIVEN* a shim connects from a process running as UID 1001
* *WHEN* the daemon accepts the connection
* *THEN* the daemon MUST read the peer UID via `SO_PEERCRED`
* *AND* the daemon MUST resolve UID 1001 to its Unix username via the password database

### Scenario: Daemon rejects request when caller UID has no Unix user

* *GIVEN* a shim connects from a process running as UID 65534
* *AND* UID 65534 does not resolve to a known username
* *WHEN* the daemon attempts to map the UID
* *THEN* the daemon MUST send a `Denied { reason: "unknown caller" }` frame
* *AND* the daemon MUST close the connection

### Scenario: Daemon handles multiple concurrent connections

* *GIVEN* the daemon is running
* *WHEN* three shims connect simultaneously, each issuing a different request
* *THEN* the daemon MUST process all three connections concurrently
* *AND* none of the connections MUST block another from receiving its response

### Scenario: Daemon survives malformed request frame

* *GIVEN* a connected client sends a frame with declared length 16 but only 4 bytes of garbage payload
* *WHEN* the daemon decodes the frame
* *THEN* the daemon MUST close that connection with a protocol error
* *AND* the daemon MUST continue accepting new connections

### Scenario: Daemon shuts down cleanly on SIGTERM

* *GIVEN* the daemon is running and serving connections
* *WHEN* the process receives SIGTERM
* *THEN* the daemon MUST stop accepting new connections
* *AND* the daemon MUST remove `/var/run/ghbrk/broker.sock`
* *AND* the daemon MUST exit with status zero

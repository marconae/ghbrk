# Feature: broker-server

Provides the `ghbrk daemon` Unix socket server that accepts gateway connections, identifies callers via SO_PEERCRED, and orchestrates per-request policy evaluation and execution.

## Background

The daemon binds `/var/run/ghbrk/broker.sock` with mode `0660` and group `ghbrk-clients`. The supported deployment sets the daemon's primary group to `ghbrk-clients` via the systemd unit's `Group=ghbrk-clients` directive, so the socket inherits the correct group on `bind(2)` without requiring a runtime `chown`. A defence-in-depth `chown` remains for daemons started outside systemd or with a non-standard `Group=`; when that chown fails, the daemon logs at `error` level with diagnostic guidance. Linux only — peer credential reading uses `SO_PEERCRED`. Each accepted connection is handled by an independent Tokio task. The daemon must remain running across malformed-request errors and child process failures; it only exits on SIGINT, SIGTERM, or fatal bind errors.

Before executing a `gh` invocation, the broker decides whether it is a broker-mediated operation (subject to resolve + policy via `src/broker.rs::gh_is_broker_op`) or an ungoverned passthrough. Passthrough invocations still receive `GH_TOKEN` injection but bypass resolve and policy. Every `gh release` lifecycle subcommand — `create`, `delete`, `edit`, `upload`, `delete-asset`, `list`, `view`, `download` — is a broker-mediated operation and MUST be policy-gated; `gh_is_broker_op` mirrors `classify_gh`'s release arms so real execution and policy evaluation agree.

See `daemon/peer-identity` for how the broker resolves the connecting peer's UID, GID, supplementary groups, and home directory. See `daemon/policy-admin` for broker-side enforcement of the privileged `ghbrk allow` mutation request.

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

### Scenario: Daemon handles multiple concurrent connections

* *GIVEN* the daemon is running
* *WHEN* three gateway clients connect simultaneously, each issuing a different request
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

### Scenario: Broker denies a local-only git subcommand that bypasses the gateway filter

* *GIVEN* the broker is running
* *AND* a request arrives carrying a local-only git subcommand such as `status` (e.g. from a hand-crafted client)
* *WHEN* the broker resolves the request
* *THEN* the broker MUST NOT execute a git process for the request
* *AND* the broker MUST send a `Denied` frame
* *AND* the broker MUST write a deny entry to the audit log

### Scenario: Broker holds a reloadable policy handle

* *GIVEN* the daemon has loaded `/etc/ghbrk/policy.yaml` at startup
* *WHEN* an in-process policy reload replaces the active policy document
* *THEN* connections accepted after the reload MUST evaluate against the new policy
* *AND* in-flight connections that already captured the prior policy MUST complete without panicking

### Scenario: Broker policy-gates gh release delete instead of passing it through

* *GIVEN* the broker is running and no policy rule grants the calling user `release_delete` on `acme/web`
* *AND* `cwd` is a clone of `acme/web`
* *WHEN* the user runs `ghbrk gh release delete v1.2.0 --yes`
* *THEN* the broker MUST route the invocation through resolve and policy as a broker-mediated operation, not passthrough
* *AND* the broker MUST deny the `release_delete` request on `acme/web` by default
* *AND* the broker MUST NOT execute `gh release delete`

### Scenario: Broker policy-gates the mutating gh release subcommands

* *GIVEN* the broker is running
* *WHEN* the broker receives a `gh release edit`, `gh release upload`, or `gh release delete-asset` invocation
* *THEN* `gh_is_broker_op` MUST report each as a broker-mediated operation
* *AND* the broker MUST route each through resolve and policy before any execution
* *AND* the broker MUST NOT fall through to ungoverned passthrough for any of them

### Scenario: Broker executes a policy-allowed gh release delete

* *GIVEN* a policy rule grants the calling user the `maintain` role on `acme/web`
* *AND* `cwd` is a clone of `acme/web`
* *WHEN* the user runs `ghbrk gh release delete v1.2.0 --yes`
* *THEN* the broker MUST evaluate the policy and obtain an `allow` decision for `release_delete`
* *AND* the broker MUST inject `GH_TOKEN` and execute the wrapped `gh release delete`

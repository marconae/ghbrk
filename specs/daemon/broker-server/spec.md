# Feature: broker-server

Provides the `ghbrk daemon` Unix socket server that accepts gateway connections, identifies callers via SO_PEERCRED, and orchestrates per-request policy evaluation and execution.

## Background

In addition to the peer username, the broker now resolves the peer's full identity from `SO_PEERCRED` plus the password database: `uid`, primary `gid`, supplementary GIDs, and home directory. This identity is attached to the `ChildSpec` for every executing tool (brokered git and gh passthrough) so the executor can drop the child to the requesting user. Supplementary group lookup failure is non-fatal — the broker logs a warning and proceeds with the primary GID. All prior binding, peer-credential, policy, and concurrency behaviour is unchanged.

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

* *GIVEN* a gateway client connects from a process running as UID 1001
* *WHEN* the daemon accepts the connection
* *THEN* the daemon MUST read the peer UID via `SO_PEERCRED`
* *AND* the daemon MUST resolve UID 1001 to its Unix username via the password database

### Scenario: Daemon rejects request when caller UID has no Unix user

* *GIVEN* a gateway client connects
* *AND* UID 65534 does not resolve to a known username
* *WHEN* the daemon attempts to resolve the peer identity
* *THEN* the daemon MUST send a `Denied { reason: "unknown caller" }` frame
* *AND* the daemon MUST NOT spawn any child process

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

### Scenario: Daemon resolves the full peer identity for privilege drop

* *GIVEN* a gateway client connects whose `SO_PEERCRED` reports UID 1001
* *AND* UID 1001 resolves to a passwd entry with a primary GID and a home directory
* *WHEN* the daemon prepares to execute the request
* *THEN* the daemon MUST resolve the peer's primary GID from the password database
* *AND* the daemon MUST look up the peer's supplementary group memberships
* *AND* the daemon MUST attach the resolved `uid`, `gid`, supplementary GIDs, and home directory to the `ChildSpec` it builds for every executing tool (brokered git, gh passthrough)

### Scenario: Daemon proceeds with primary GID when supplementary group lookup fails

* *GIVEN* a gateway client whose peer UID resolves to a valid passwd entry
* *AND* the supplementary group lookup for that user fails or returns no groups
* *WHEN* the daemon builds the `ChildSpec`
* *THEN* the daemon SHOULD log the supplementary group lookup failure at `warn` level
* *AND* the daemon MUST still build the `ChildSpec` with the peer's `uid` and primary `gid`
* *AND* the daemon MUST NOT deny the request solely because the supplementary group lookup failed

### Scenario: Broker holds a reloadable policy handle

* *GIVEN* the daemon has loaded `/etc/ghbrk/policy.yaml` at startup
* *WHEN* an in-process policy reload replaces the active policy document
* *THEN* connections accepted after the reload MUST evaluate against the new policy
* *AND* in-flight connections that already captured the prior policy MUST complete without panicking

### Scenario: Broker enforces privilege for the allow request

* *GIVEN* the broker receives a `Request { tool: allow, ... }`
* *AND* the connecting peer's `SO_PEERCRED` reports an effective UID other than 0
* *WHEN* the broker processes the request
* *THEN* the broker MUST send a `Denied` frame indicating elevated privileges are required
* *AND* the broker MUST NOT write to the policy file
* *AND* the broker MUST write a deny entry to the audit log

### Scenario: Broker appends a rule and reloads on a privileged allow request

* *GIVEN* the broker receives a `Request { tool: allow, args: ["acme/web", "write"] }`
* *AND* the connecting peer's `SO_PEERCRED` reports effective UID 0
* *WHEN* the broker processes the request
* *THEN* the broker MUST append a validated allow rule to the policy file
* *AND* the broker MUST reload the policy handle so subsequent connections see the new rule
* *AND* the broker MUST write an allow entry to the audit log and stream a confirmation followed by an `Exit { code: 0 }` frame

### Scenario: Allow request validates operands before mutating the policy file

* *GIVEN* the broker receives a privileged `Request { tool: allow, args: ["acme/web", "frobnicate"] }`
* *WHEN* the broker validates the operands against the loaded policy vocabulary and roles
* *THEN* the broker MUST reject the request with a `Denied` frame mentioning `frobnicate`
* *AND* the broker MUST leave the policy file byte-for-byte unchanged

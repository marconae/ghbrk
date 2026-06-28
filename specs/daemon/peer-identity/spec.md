# Feature: peer-identity

Resolves the connecting peer's full identity — Unix username, `uid`, primary `gid`, supplementary GIDs, and home directory — from `SO_PEERCRED` plus the password database, so the broker can identify the caller for policy evaluation and attach the resolved identity to the `ChildSpec` used for privilege drop. Split from `daemon/broker-server`.

## Background

The broker reads the peer UID via `SO_PEERCRED` on each accepted connection and resolves it to a Unix username via the password database; a UID with no matching passwd entry is rejected before any child process is spawned. In addition to the username, the broker resolves the peer's primary `gid`, supplementary GIDs, and home directory. This identity is attached to the `ChildSpec` for every executing tool (brokered git and gh passthrough) so the executor can drop the child to the requesting user — see `daemon/executor-privilege-drop` for the drop itself. Supplementary group lookup failure is non-fatal: the broker logs a warning and proceeds with the primary GID rather than denying the request.

## Scenarios

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

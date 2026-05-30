# Feature: broker-server

Provides the `ghbrk daemon` Unix socket server that accepts gateway connections, identifies callers via SO_PEERCRED, and orchestrates per-request policy evaluation and execution.

## Background

The broker is reached only via the explicit `ghbrk git` / `ghbrk gh` gateway; the transparent argv[0] shim and client-side local/remote split are removed. The `ghbrk git` gateway filters local-only git subcommands before any socket connection, so in normal operation the broker receives only remote/authenticated git operations plus all `gh` invocations (for credential injection). As defence-in-depth the broker still resolves every request and denies anything it cannot map to a known remote operation. All prior binding, peer-credential, and concurrency behaviour is unchanged.

## Scenarios

<!-- DELTA:NEW -->
### Scenario: Broker denies a local-only git subcommand that bypasses the gateway filter

* *GIVEN* the broker is running
* *AND* a request arrives carrying a local-only git subcommand such as `status` (e.g. from a hand-crafted client)
* *WHEN* the broker resolves the request
* *THEN* the broker MUST NOT execute a git process for the request
* *AND* the broker MUST send a `Denied` frame
* *AND* the broker MUST write a deny entry to the audit log
<!-- /DELTA:NEW -->

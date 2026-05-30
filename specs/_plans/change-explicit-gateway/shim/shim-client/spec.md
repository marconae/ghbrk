# Feature: shim-client

Implements the `git` / `gh` shim role so an agent's invocation is transparently relayed through the broker. This feature is removed from the `shim/` domain; the relay-transport behaviour (connect, stream stdout/stderr, propagate exit code) is re-homed under the explicit `ghbrk git` / `ghbrk gh` gateway in infra/cli-dispatch.

## Background

The shim connected to `/var/run/ghbrk/broker.sock` and acted as a thin pipe between the agent's stdio and the broker's response stream. With transparent interception removed, this role no longer exists as an implicit shim; the same transport is exercised only via the explicit `ghbrk git`/`ghbrk gh` subcommands.

## Scenarios

<!-- DELTA:REMOVED -->
### Scenario: Shim relays git push and forwards exit code

* *GIVEN* the broker is running and the user is allowed to push to the current repo
* *WHEN* the agent invokes `git push origin main` via the shim
* *THEN* the shim MUST send a `Request { tool: "git", args: ["push", "origin", "main"], cwd: <agent cwd> }` frame
* *AND* the shim MUST exit with the code from the broker's `Exit` frame
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Shim streams stdout in real time

* *GIVEN* the broker is producing stdout chunks at 100 ms intervals
* *WHEN* each `StdoutChunk` frame arrives at the shim
* *THEN* the shim MUST write the chunk bytes to its own stdout before reading the next frame
* *AND* the shim MUST NOT buffer all output until the `Exit` frame
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Shim streams stderr in real time

* *GIVEN* the broker is producing stderr chunks during a long clone
* *WHEN* each `StderrChunk` frame arrives at the shim
* *THEN* the shim MUST write the chunk bytes to its own stderr
* *AND* the shim MUST NOT mix them into stdout
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Shim reports denial to stderr and exits non-zero

* *GIVEN* the broker denies the request with reason `"repo not in allow list"`
* *WHEN* the shim receives the `Denied` frame
* *THEN* the shim MUST print `ghbrk: denied: repo not in allow list` (or equivalent) to stderr
* *AND* the shim MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Shim reports broker socket missing

* *GIVEN* the broker socket `/var/run/ghbrk/broker.sock` does not exist
* *WHEN* the shim attempts to connect
* *THEN* the shim MUST print an error to stderr indicating the broker is unavailable
* *AND* the shim MUST exit with a non-zero status
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Shim sends current working directory

* *GIVEN* the agent's process cwd is `/home/alice/projects/foo`
* *WHEN* the shim builds the `Request` frame
* *THEN* the `cwd` field MUST equal `/home/alice/projects/foo`
<!-- /DELTA:REMOVED -->

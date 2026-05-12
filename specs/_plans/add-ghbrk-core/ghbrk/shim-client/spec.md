# Feature: shim-client

Implements the `git` / `gh` shim role so an agent's invocation is transparently relayed through the broker, with stdout and stderr streamed in real time and the broker's exit code propagated.

## Background

The shim connects to `/var/run/ghbrk/broker.sock`. The shim does not itself read credentials, evaluate policy, or execute any git/gh process. It is a thin pipe between the agent's stdio and the broker's response stream.

## Scenarios

### Scenario: Shim relays git push and forwards exit code

* *GIVEN* the broker is running and the user is allowed to push to the current repo
* *WHEN* the agent invokes `git push origin main` via the shim
* *THEN* the shim MUST send a `Request { tool: "git", args: ["push", "origin", "main"], cwd: <agent cwd> }` frame
* *AND* the shim MUST exit with the code from the broker's `Exit` frame

### Scenario: Shim streams stdout in real time

* *GIVEN* the broker is producing stdout chunks at 100 ms intervals
* *WHEN* each `StdoutChunk` frame arrives at the shim
* *THEN* the shim MUST write the chunk bytes to its own stdout before reading the next frame
* *AND* the shim MUST NOT buffer all output until the `Exit` frame

### Scenario: Shim streams stderr in real time

* *GIVEN* the broker is producing stderr chunks during a long clone
* *WHEN* each `StderrChunk` frame arrives at the shim
* *THEN* the shim MUST write the chunk bytes to its own stderr
* *AND* the shim MUST NOT mix them into stdout

### Scenario: Shim reports denial to stderr and exits non-zero

* *GIVEN* the broker denies the request with reason `"repo not in allow list"`
* *WHEN* the shim receives the `Denied` frame
* *THEN* the shim MUST print `ghbrk: denied: repo not in allow list` (or equivalent) to stderr
* *AND* the shim MUST exit with a non-zero status

### Scenario: Shim reports broker socket missing

* *GIVEN* the broker socket `/var/run/ghbrk/broker.sock` does not exist
* *WHEN* the shim attempts to connect
* *THEN* the shim MUST print an error to stderr indicating the broker is unavailable
* *AND* the shim MUST exit with a non-zero status

### Scenario: Shim sends current working directory

* *GIVEN* the agent's process cwd is `/home/alice/projects/foo`
* *WHEN* the shim builds the `Request` frame
* *THEN* the `cwd` field MUST equal `/home/alice/projects/foo`

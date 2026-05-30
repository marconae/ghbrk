# Feature: shim-passthrough

Lets the `git` and `gh` shims run local and unsupported subcommands directly against the real binary. This entire feature is removed: there is no transparent shim and no client-side local/remote classification — agents call plain `git`/`gh` for local work and invoke `ghbrk git`/`ghbrk gh` explicitly for brokered remote operations.

## Background

The shim was invoked as `git` or `gh` (via symlink or `ghbrk git` / `ghbrk gh`) and classified each invocation into local-passthrough versus broker-mediated. With the explicit gateway, this classification logic and all passthrough behaviour are deleted.

## Scenarios

<!-- DELTA:REMOVED -->
### Scenario: Local git subcommand passes through to the real binary

* *GIVEN* the shim is invoked as `git status` inside a git repository
* *AND* the resolved real git binary exists at the configured path
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as passthrough
* *AND* the shim MUST exec the real git binary with the original arguments
* *AND* the shim MUST NOT open a connection to the broker socket
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Known remote git subcommand is routed to the broker

* *GIVEN* the shim is invoked as `git push origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real git binary directly
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: git invoked with no subcommand passes through

* *GIVEN* the shim is invoked as `git` with no arguments
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as passthrough
* *AND* the shim MUST exec the real git binary so the real usage text is shown
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: git global flags before the subcommand are skipped during classification

* *GIVEN* the shim is invoked as `git -c core.pager=cat push origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST treat `push` as the subcommand
* *AND* the shim MUST classify the invocation as broker-mediated
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Informational gh command is routed to the broker for credential injection

* *GIVEN* the shim is invoked as `gh auth status`
* *AND* the agent's environment contains no `GH_TOKEN`
* *WHEN* the shim evaluates the connection decision
* *THEN* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly
* *AND* the broker MUST inject `GH_TOKEN` before executing the real gh binary
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Known remote gh operation is routed to the broker

* *GIVEN* the shim is invoked as `gh pr create --title x`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the `(pr, create)` pair as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Unknown gh subcommand is routed to the broker for credential injection

* *GIVEN* the shim is invoked as `gh repo view`
* *AND* the agent's environment contains no `GH_TOKEN`
* *WHEN* the shim evaluates the connection decision
* *THEN* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly
* *AND* the broker MUST inject `GH_TOKEN` before executing the real gh binary
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Passthrough preserves the real binary exit code

* *GIVEN* the shim passes through to a real binary that exits non-zero
* *WHEN* the real binary terminates
* *THEN* the shim process MUST terminate with the identical exit code
* *AND* the shim MUST NOT buffer or rewrite the real binary's stdout or stderr
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Passthrough with a missing real binary reports a clear error

* *GIVEN* the configured real binary path does not exist
* *WHEN* the shim attempts to exec the real binary for a passthrough invocation
* *THEN* the shim MUST exit non-zero
* *AND* the shim MUST print an error naming the missing binary path to stderr
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Known remote git pull is routed to the broker

* *GIVEN* the shim is invoked as `git pull origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real git binary directly
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: git pull with global flags before the subcommand is broker-mediated

* *GIVEN* the shim is invoked as `git -c http.sslVerify=false pull origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST treat `pull` as the subcommand
* *AND* the shim MUST classify the invocation as broker-mediated
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Broker socket permission-denied (EACCES) silently passes through to real binary

* *GIVEN* the shim is invoked for a broker-mediated subcommand
* *AND* the broker socket file exists but the calling process lacks filesystem permission to connect (the `UnixStream::connect` call returns `EACCES`, POSIX errno 13)
* *WHEN* the shim attempts to connect to the broker
* *THEN* the shim MUST exec the real binary with the original arguments, preserving stdio, signals, and exit code via `exec()`
* *AND* the shim MUST NOT print any `ghbrk:` message to stderr
* *AND* the EACCES fallthrough MUST require no operator configuration (no config flag, no environment variable)
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Broker socket missing (ENOENT) still hard-fails

* *GIVEN* the shim is invoked for a broker-mediated subcommand
* *AND* the broker socket file does not exist (the `UnixStream::connect` call returns `ENOENT`)
* *WHEN* the shim attempts to connect to the broker
* *THEN* the shim MUST print a connection-error message naming the socket path to stderr
* *AND* the shim MUST exit with the shim-error exit code
* *AND* the shim MUST NOT exec the real binary
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Broker connection refused (ECONNREFUSED) still hard-fails

* *GIVEN* the shim is invoked for a broker-mediated subcommand
* *AND* the broker socket file exists but no process is listening (the `UnixStream::connect` call returns `ECONNREFUSED`)
* *WHEN* the shim attempts to connect to the broker
* *THEN* the shim MUST print a connection-error message naming the socket path to stderr
* *AND* the shim MUST exit with the shim-error exit code
* *AND* the shim MUST NOT exec the real binary
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: gh api subcommand is routed to the broker

* *GIVEN* the shim is invoked as `gh api user`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly
<!-- /DELTA:REMOVED -->

# Feature: shim-passthrough

Lets the `git` and `gh` shims run local and unsupported subcommands directly against the real binary, so that everyday commands like `git status` or `gh auth status` work normally instead of being denied by the broker. Only the small set of remote operations the broker understands is routed through the daemon; everything else passes through transparently.

## Background

The shim is invoked as `git` or `gh` (via symlink or `ghbrk git` / `ghbrk gh`). The shim no longer decides the passthrough/broker split for `gh` locally: every `gh` invocation is sent to the broker so the broker can inject `GH_TOKEN` even for informational commands the agent could not otherwise authenticate. The broker performs the broker-op classification on its side — policy-gated execution for known remote operations (`pr`/`issue`/`release`/`api`), and credential-injected passthrough for everything else. For `git`, the shim still classifies locally: the first non-flag token is the git subcommand; local subcommands (e.g. `status`) are exec'd directly and remote ones (`push`/`fetch`/`pull`/`clone`) are routed to the broker. When the shim does exec a real binary directly, it replaces the current process image via `exec()` so all stdio, signals, and the exit code are preserved with no buffering. If the broker socket connect returns `EACCES`, the shim falls back to exec'ing the real binary directly with no credential injection.

## Scenarios

### Scenario: Local git subcommand passes through to the real binary

* *GIVEN* the shim is invoked as `git status` inside a git repository
* *AND* the resolved real git binary exists at the configured path
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as passthrough
* *AND* the shim MUST exec the real git binary with the original arguments
* *AND* the shim MUST NOT open a connection to the broker socket

### Scenario: Known remote git subcommand is routed to the broker

* *GIVEN* the shim is invoked as `git push origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real git binary directly

### Scenario: git invoked with no subcommand passes through

* *GIVEN* the shim is invoked as `git` with no arguments
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as passthrough
* *AND* the shim MUST exec the real git binary so the real usage text is shown

### Scenario: git global flags before the subcommand are skipped during classification

* *GIVEN* the shim is invoked as `git -c core.pager=cat push origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST treat `push` as the subcommand
* *AND* the shim MUST classify the invocation as broker-mediated

### Scenario: Informational gh command is routed to the broker for credential injection

* *GIVEN* the shim is invoked as `gh auth status`
* *AND* the agent's environment contains no `GH_TOKEN`
* *WHEN* the shim evaluates the connection decision
* *THEN* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly
* *AND* the broker MUST inject `GH_TOKEN` before executing the real gh binary

### Scenario: Known remote gh operation is routed to the broker

* *GIVEN* the shim is invoked as `gh pr create --title x`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the `(pr, create)` pair as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly

### Scenario: Unknown gh subcommand is routed to the broker for credential injection

* *GIVEN* the shim is invoked as `gh repo view`
* *AND* the agent's environment contains no `GH_TOKEN`
* *WHEN* the shim evaluates the connection decision
* *THEN* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly
* *AND* the broker MUST inject `GH_TOKEN` before executing the real gh binary

### Scenario: Passthrough preserves the real binary exit code

* *GIVEN* the shim passes through to a real binary that exits non-zero
* *WHEN* the real binary terminates
* *THEN* the shim process MUST terminate with the identical exit code
* *AND* the shim MUST NOT buffer or rewrite the real binary's stdout or stderr

### Scenario: Passthrough with a missing real binary reports a clear error

* *GIVEN* the configured real binary path does not exist
* *WHEN* the shim attempts to exec the real binary for a passthrough invocation
* *THEN* the shim MUST exit non-zero
* *AND* the shim MUST print an error naming the missing binary path to stderr

### Scenario: Known remote git pull is routed to the broker

* *GIVEN* the shim is invoked as `git pull origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real git binary directly

### Scenario: git pull with global flags before the subcommand is broker-mediated

* *GIVEN* the shim is invoked as `git -c http.sslVerify=false pull origin main`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST treat `pull` as the subcommand
* *AND* the shim MUST classify the invocation as broker-mediated

### Scenario: Broker socket permission-denied (EACCES) silently passes through to real binary

* *GIVEN* the shim is invoked for a broker-mediated subcommand
* *AND* the broker socket file exists but the calling process lacks filesystem permission to connect (the `UnixStream::connect` call returns `EACCES`, POSIX errno 13)
* *WHEN* the shim attempts to connect to the broker
* *THEN* the shim MUST exec the real binary with the original arguments, preserving stdio, signals, and exit code via `exec()`
* *AND* the shim MUST NOT print any `ghbrk:` message to stderr
* *AND* the EACCES fallthrough MUST require no operator configuration (no config flag, no environment variable)

### Scenario: Broker socket missing (ENOENT) still hard-fails

* *GIVEN* the shim is invoked for a broker-mediated subcommand
* *AND* the broker socket file does not exist (the `UnixStream::connect` call returns `ENOENT`)
* *WHEN* the shim attempts to connect to the broker
* *THEN* the shim MUST print a connection-error message naming the socket path to stderr
* *AND* the shim MUST exit with the shim-error exit code
* *AND* the shim MUST NOT exec the real binary

### Scenario: Broker connection refused (ECONNREFUSED) still hard-fails

* *GIVEN* the shim is invoked for a broker-mediated subcommand
* *AND* the broker socket file exists but no process is listening (the `UnixStream::connect` call returns `ECONNREFUSED`)
* *WHEN* the shim attempts to connect to the broker
* *THEN* the shim MUST print a connection-error message naming the socket path to stderr
* *AND* the shim MUST exit with the shim-error exit code
* *AND* the shim MUST NOT exec the real binary

### Scenario: gh api subcommand is routed to the broker

* *GIVEN* the shim is invoked as `gh api user`
* *WHEN* the shim evaluates the passthrough decision
* *THEN* the shim MUST classify the invocation as broker-mediated
* *AND* the shim MUST attempt to connect to the broker socket
* *AND* the shim MUST NOT exec the real gh binary directly

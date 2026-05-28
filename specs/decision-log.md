# Architecture Decision Records

<!-- ADRs are numbered sequentially starting from ADR-001. Never renumber. -->
<!-- recorder-agent appends new ADRs from plan decision logs. -->

---

## ADR-001: Passthrough gate lives in the shim, in front of broker contact

**Date:** 2026-05-27
**Plan:** `fix-local-git-gh-passthrough`
**Status:** Accepted

### Context

The shim relayed every `git` and `gh` invocation to the broker. The broker's resolver classifies only a small set of remote operations (`git push/fetch/clone`; selected `gh pr/issue/release` subcommands), so any other subcommand — including ubiquitous local commands like `git status`, `git add`, `git commit`, and informational `gh auth status` — produced a hard denial. This made the shimmed `PATH` unusable for normal development work.

### Decision

Add an `is_passthrough` decision in the shim entrypoints (`cmd/git.rs`, `cmd/gh.rs`) that runs before any broker connection. Local and unsupported subcommands are exec'd directly against the real binary; only the known remote operations reach the broker.

### Options Considered

| Option | Verdict |
|--------|---------|
| Passthrough gate in the shim, before broker contact | Chosen — keeps local commands entirely on the client side; cheap pure classification keeps the broker out of the hot path |
| Teach the broker/resolver to recognise and locally execute non-remote subcommands | Rejected — would route local commands across the socket and through the privileged daemon for no benefit, expanding the daemon's attack surface and adding latency |

### Consequences

Local commands (`git status`, `git add`, `git log`, `gh auth status`, etc.) execute instantly with no daemon involvement. The broker retains its unknown-subcommand denial for defence in depth. The classification set must be kept in sync between the shim passthrough gate and the broker resolver; a unit test pins both sides.

---

## ADR-002: Use exec() process replacement for passthrough

**Date:** 2026-05-27
**Plan:** `fix-local-git-gh-passthrough`
**Status:** Accepted

### Context

When the shim decides a command is passthrough, it must hand execution to the real binary. The approach chosen determines how stdio, signals, tty control, and exit codes are handled.

### Decision

Passthrough replaces the process image via `std::os::unix::process::CommandExt::exec()` rather than spawning a child and forwarding I/O.

### Options Considered

| Option | Verdict |
|--------|---------|
| `exec()` process replacement | Chosen — inherits stdio, the controlling tty, and signals natively with zero buffering; exit code is exactly the real binary's; simpler code with no streaming machinery |
| Spawn the real binary as a child, pipe stdout/stderr, forward exit code | Rejected — reintroduces buffering and signal-forwarding concerns; equivalent to the broker path's streaming machinery applied to a use case that needs none of it |

### Consequences

Passthrough is transparent: tty-aware programs (pagers, colour output, interactive prompts) work correctly. `exec()` does not return on success, so error handling on the passthrough path is limited to the exec failure case, which is handled by printing a clear error and exiting non-zero.

---

## ADR-003: Broker retains its unknown-subcommand denial

**Date:** 2026-05-27
**Plan:** `fix-local-git-gh-passthrough`
**Status:** Accepted

### Context

With the shim-side passthrough gate in place, unsupported subcommands are intercepted before reaching the broker. A question arose whether the broker's existing `Unknown*` rejection is now redundant and should be removed.

### Decision

Leave the resolver's `Unknown*` rejection in place; do not relax the broker's failure-closed behaviour now that the shim filters.

### Options Considered

| Option | Verdict |
|--------|---------|
| Retain the broker's unknown-subcommand denial | Chosen — defence in depth; a direct `ghbrk git <unknown>` invocation, or any future caller that reaches the broker without the shim gate, still fails closed |
| Remove the rejection as redundant now that the shim filters | Rejected — would rely solely on the shim gate for policy enforcement; any bypass (direct broker call, future caller) would silently succeed |

### Consequences

The broker continues to fail closed on unknown subcommands. This is intentional: the shim passthrough gate is a usability improvement, not the only line of policy defence. Operator-facing error messages for direct broker invocations remain informative.

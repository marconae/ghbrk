# Decisions: fix-credential-access-check-and-gh-passthrough

## ADR: Route `ghbrk check` through the broker instead of reading credentials client-side

**ID:** route-ghbrk-check-through-broker
**Plan:** fix-credential-access-check-and-gh-passthrough
**Status:** Superseded by explicit-gateway-replaces-transparent-shim

### Context

`ghbrk check` originally ran as the invoking Unix user and directly read the credential directory `/etc/ghbrk/credentials/<user>/`. That directory is owned by the `ghbrk` system user with mode `0700`, so a normal agent user cannot `stat()` or read its contents. This caused a "Permission denied" failure for any agent or operator user who ran `ghbrk check`. A privileged path to credential inspection was needed without loosening the `0700` isolation or introducing a new setuid/sudo helper.

> Superseded note (`change-explicit-gateway`): the standalone `ghbrk check` subcommand is absorbed into `ghbrk doctor`. The broker-side `Tool::Check` credential mechanism established here is retained and reused by `doctor`.

### Decision

Add a `Tool::Check` wire variant. `cmd/check.rs` becomes a shim client: it connects to the broker socket, sends a `Request { tool: check, args: [], cwd }`, and streams output back to the caller. The broker identifies the caller via `SO_PEERCRED`, runs the credential checks as `ghbrk`, and streams `StdoutChunk` frames followed by an `Exit { code }` frame. No policy evaluation is performed.

### Options Considered

| Option | Verdict |
|--------|---------|
| Route `ghbrk check` through the broker via `Tool::Check` | Chosen — reuses the existing socket + peer-cred machinery; the broker already runs as `ghbrk` with read access to the credential directory; no new privilege escalation path |
| setuid/sudo helper to read credentials as `ghbrk` from the client | Rejected — introduces a new privileged execution path; more complex deployment; no reuse of broker plumbing |
| Loosen credential directory permissions to allow agent user reads | Rejected — defeats the `0700` isolation that prevents agents from reading each other's credentials |

### Consequences

`ghbrk check` works for any user without filesystem access to the credential directory. The broker socket and `SO_PEERCRED` are the single trusted channel for all credential-adjacent operations. The `health_check` module is extracted into `src/health_check.rs` and callable from both the broker and tests.

## ADR: Centralise all `gh` broker/passthrough classification on the broker side

**ID:** centralise-gh-classification-on-broker-side
**Plan:** fix-credential-access-check-and-gh-passthrough
**Status:** Accepted

### Context

The shim previously classified `gh` invocations locally: broker-ops (`pr`, `issue`, `release`, `api`) were routed to the broker; everything else was exec'd directly as passthrough. Passthrough `gh` calls had no access to `GH_TOKEN`, since the token is owned by the `ghbrk` user and not available in the agent's environment. Commands like `gh repo view` and `gh auth status` failed with authentication errors even though the system held the token.

### Decision

`cmd/gh.rs` always connects to the broker for every `gh` invocation. The broker calls `gh_is_broker_op` to classify: true routes to the existing policy pipeline; false loads credentials, injects `GH_TOKEN`, execs real `gh` as credential-injected passthrough, and records an `AuditDecision::Passthrough` entry. The shim-side classification for `gh` is removed entirely.

### Options Considered

| Option | Verdict |
|--------|---------|
| Centralise `gh` classification on the broker side | Chosen — the broker is the only process with access to `GH_TOKEN`; any shim-side passthrough exec leaves the agent without a token; classification lives where the token lives |
| Keep shim-side `is_passthrough` for gh and inject tokens only on broker-op calls | Rejected — passthrough `gh` invocations still fail to authenticate; the agent has no `GH_TOKEN` |

### Consequences

All `gh` invocations receive `GH_TOKEN` injection. The broker audit log now records passthrough `gh` calls with `decision=passthrough`, keeping them visible and distinguishable from policy-evaluated `Allow` decisions. The operator's access gate remains unchanged: `ghbrk-clients` group membership controls whether the gateway client can reach the broker socket.

**2026-09-22 audit note (not part of the original decision):** the original text also said `gh_is_broker_op` "is made `pub` so the broker can call it from `broker.rs`." That is no longer true — `gh_is_broker_op` is a private function inside `broker.rs` today. The decision itself (broker-side classification, always-connect) still holds; only that one implementation detail is stale.

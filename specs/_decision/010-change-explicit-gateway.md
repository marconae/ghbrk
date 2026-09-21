# Decisions: change-explicit-gateway

## ADR: Explicit gateway replaces the transparent shim

**ID:** explicit-gateway-replaces-transparent-shim
**Plan:** change-explicit-gateway
**Status:** Accepted

### Context

The transparent shim symlinked `ghbrk` as `git` and `gh` early in the agent's `PATH`, silently intercepting every invocation and classifying it client-side into local-passthrough vs broker-mediated. For an AI agent this makes privileged, machine-leaving behaviour invisible: the agent cannot tell from the command alone whether it is hitting the network under brokered credentials, and the client-side classifier must perfectly mirror git/gh semantics or it silently misroutes. Invisible privileged authority gives agents no way to reason about the security boundary.

### Decision

Remove all transparent PATH-interception: no argv[0] symlink dispatch, no client-side local/remote passthrough classifier, and no shim config for real-binary paths. Privileged authority is requested explicitly by name via `ghbrk git <remote-subcommand>` and `ghbrk gh <subcommand>`. The security boundary becomes part of the interface.

### Options Considered

| Option | Verdict |
|--------|---------|
| Explicit `ghbrk git`/`ghbrk gh` verb gateway, no symlinks | Chosen — privilege is requested by name and never inferred; the boundary is inspectable rather than hidden |
| Keep an optional `install-shims` transparent compat mode | Rejected — a hidden mode re-introduces the invisible-privilege problem the redesign exists to eliminate |

### Consequences

Agents call plain `git`/`gh` for local work and `ghbrk git`/`ghbrk gh` only when an operation leaves the machine. No symlinks are created at install time, and there is no `install-shims` step. The mental model is crisp: ghbrk does exactly one thing, broker remote operations. Existing automation that relied on transparent interception must be updated to call the gateway explicitly (breaking change).

**2026-09-22 audit note (not part of the original decision):** the original text also said the crate was "bumped to 0.5.0" for this breaking change. No `0.5.0` version ever appears in `Cargo.toml`'s git history; the decision's substance (explicit gateway, no symlinks) is unaffected, and the version claim is left unresolved rather than guessed at, since the pre-reset commit history is squashed and no longer inspectable.

## ADR: ghbrk scope is remote/authenticated operations only

**ID:** ghbrk-scope-is-remote-authenticated-ops-only
**Plan:** change-explicit-gateway
**Status:** Accepted

### Context

With the explicit gateway, a decision was needed on what `ghbrk git <local-subcommand>` (for example `status`, `log`, `commit`) should do. Allowing it to passthrough-exec the local binary would re-create the client-side classifier and the confusing "is this brokered?" mental model the redesign removes.

### Decision

`ghbrk git <local-subcommand>` returns a clear guidance error before any socket connection, telling the user to run the command with plain `git`. Only machine-leaving (remote/authenticated) operations are relayed to the broker. `ghbrk` constrains itself strictly to the remote/authenticated boundary.

### Options Considered

| Option | Verdict |
|--------|---------|
| Reject local git subcommands with a pre-connect guidance error | Chosen — keeps the authority boundary crisp; ghbrk only ever brokers remote operations |
| Let `ghbrk git status` passthrough-exec the local binary | Rejected — re-creates the client-side classifier and the confusing brokered-or-not mental model |

### Consequences

The gateway never executes local git. Users get an immediate, actionable error directing them to plain `git`. As defence-in-depth the broker still resolves every request and denies any local-only subcommand that reaches the socket from a hand-crafted client, so the default-deny invariant holds at the trust boundary.

## ADR: Resolver stays broker-side; feature relocated to the daemon domain

**ID:** resolver-stays-broker-side
**Plan:** change-explicit-gateway
**Status:** Accepted

### Context

The resolver maps `(tool, args, cwd)` to a normalised `(operation, org, repo, branch?)` tuple. It already ran broker-side (`src/broker.rs::resolve_request`) but its spec feature was filed under the now-removed `shim/` domain. With the shim gone, a decision was needed on where resolution belongs and where its spec should live.

### Decision

Keep the resolver in the broker, unchanged, and relocate its spec feature from `shim/` to `daemon/resolver`. The gateway client stays a thin relay; the broker remains the single authoritative mapping from command to operation.

### Options Considered

| Option | Verdict |
|--------|---------|
| Keep resolver broker-side; relocate the feature to `daemon/resolver` | Chosen — resolution was always a broker-side concern; a single authoritative mapping stays inside the trust boundary |
| Delete the resolver and have the client send a pre-resolved tuple | Rejected — leaks repo-context parsing out of the trust boundary and lets a malicious client spoof the resolved operation |

### Consequences

Parsing and repo-context logic stay inside the privileged daemon, so a client cannot spoof the operation it is requesting. The relocation was behaviour-preserving at the time: the resolver scenarios moved unchanged to `daemon/resolver`. The client relays `(tool, args, cwd)` and streams the response.

**2026-09-22 audit note (not part of the original decision):** the client-cannot-spoof claim has since narrowed. The `Request` protocol now also carries client-computed `remote_url`/`head_branch` hints (see `daemon/repo-discovery`); when present, the broker trusts the hint instead of doing its own filesystem discovery. This is a recorded, deliberate later design, not a defect, but it means "the client is reduced to relaying `(tool, args, cwd)`" is no longer accurate as written. Treat this ADR as superseded in spirit by `daemon/repo-discovery`'s spec; no formal superseding ADR exists yet.

# Decision Log: change-explicit-gateway

Date: 2026-05-30

## Interview

**Q:** What is the fate of the existing `shim/` domain?
**A:** Remove entirely. Delete the shim symlink mechanism and all related code. No compatibility mode needed.

**Q:** Which new top-level subcommands should be added?
**A:** Three: `ghbrk doctor` (checks environment — daemon running, credentials present, policy valid), `ghbrk explain <cmd>` (explains what ghbrk would do with a given command: policy check, credential injection, etc.), and `ghbrk policy <org>/<repo>` (returns what operations are permitted and what is forbidden for a given repo).

**Q:** Should `ghbrk git` / `ghbrk gh` routing change?
**A:** Yes. Remove all local-only operations from ghbrk scope. ghbrk is only for operations that leave the machine (network/remote ops). `ghbrk git status`, `ghbrk git log`, etc. must not be valid — return a clear guidance error: "use 'git status' directly; ghbrk only brokers remote operations." The clean scope: ghbrk = remote/authenticated boundary only.

## Design Decisions

### [1] Explicit gateway replaces transparent shim

- **Decision:** Remove all transparent PATH-interception (argv[0] symlink dispatch, client-side passthrough classifier, shim config). Privileged authority is requested explicitly via `ghbrk git`/`ghbrk gh`.
- **Alternatives:** Keep an optional `install-shims` transparent compat mode. Rejected by the user.
- **Rationale:** Invisible privileged behaviour gives AI agents no way to know whether a command leaves the machine under brokered credentials. Making the boundary part of the interface is the core principle of the redesign.
- **Promotes to ADR:** yes

### [2] ghbrk scope is remote/authenticated operations only

- **Decision:** `ghbrk git <local-subcommand>` (e.g. `status`, `log`) returns a guidance error before any socket connect, telling the user to run the command with plain `git`. Only machine-leaving operations are relayed.
- **Alternatives:** Let `ghbrk git status` passthrough-exec the local binary (the old behaviour). Rejected.
- **Rationale:** Passthrough re-creates the client-side classifier and the confusing "is this brokered?" mental model. A crisp authority boundary means ghbrk does exactly one thing: broker remote operations.
- **Promotes to ADR:** yes

### [3] Resolver stays broker-side; feature relocated to daemon domain

- **Decision:** The resolver (`(tool, args, cwd)` -> `(operation, org, repo, branch?)`) is kept in the broker (`src/broker.rs::resolve_request`) unchanged, and its spec feature is relocated from the removed `shim/` domain to `daemon/resolver`.
- **Alternatives:** Delete the resolver and have the client send a pre-resolved tuple to the broker. Rejected.
- **Rationale:** The resolver already runs broker-side; it was only ever filed under `shim/`. Moving parsing to the client would leak repo-context logic out of the trust boundary and let a malicious client spoof the resolved operation. Keeping it in the broker preserves the single authoritative mapping.
- **Promotes to ADR:** yes

### [4] Diagnostic subcommands reuse existing transport and framing

- **Decision:** `explain` and `policy` add `Tool::Explain` / `Tool::Policy` request discriminants but reuse the existing length-prefixed JSON framing and the `StdoutChunk` + `Exit` server-frame variants to stream results. They never execute git/gh.
- **Alternatives:** Add dedicated structured server-frame variants for explanation/policy results.
- **Rationale:** Streaming human-readable lines matches the existing `check` pattern and avoids protocol churn; the broker simply evaluates and reports instead of executing.
- **Promotes to ADR:** no

### [5] ghbrk check absorbed into ghbrk doctor

- **Decision:** Remove the standalone `ghbrk check` subcommand and the `infra/health-check` feature; fold its credential checks into `ghbrk doctor`, which also adds daemon-reachability and policy-parse checks. The `Tool::Check` wire discriminant and broker-side credential logic are retained and reused.
- **Alternatives:** Keep both `check` and `doctor`.
- **Rationale:** One health entry point; doctor is a strict superset, and keeping two commands invites drift.
- **Promotes to ADR:** no

### [6] Broker defence-in-depth deny for local git subcommands

- **Decision:** Even though the `ghbrk git` gateway filters local-only subcommands before connecting, the broker still resolves every request and denies anything it cannot map to a known remote operation.
- **Alternatives:** Trust the client filter alone.
- **Rationale:** A hand-crafted client could send a local subcommand directly to the socket; the broker must not execute it. Belt-and-braces preserves the default-deny invariant at the trust boundary.
- **Promotes to ADR:** no

## Review Findings

<!-- Populated by speq-implement after code review. -->

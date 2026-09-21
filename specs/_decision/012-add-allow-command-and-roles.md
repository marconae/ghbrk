# Decisions: add-allow-command-and-roles

## ADR: Roles resolved at evaluation time, stored as role-name strings

**ID:** roles-resolved-at-evaluation-time
**Plan:** add-allow-command-and-roles
**Status:** Accepted

### Context

When a policy rule references a named role (for example `operations: write`), the rule could expand the role into a concrete operation list at load or write time, or store the role name literally and resolve it on every evaluation. Expanding at load/write time freezes the rule against the role definition at the moment of writing and breaks the requirement that redefining a role immediately affects all referencing rules.

### Decision

A rule's `operations` field may hold a role name (string) or an inline operation list. The role name is stored literally and resolved against the roles table on every `evaluate()`.

### Options Considered

| Option | Verdict |
|--------|---------|
| Store role name literally; resolve at evaluation time | Chosen — directly satisfies the requirement; one role edit fans out to all referencing rules; keeps rules human-readable |
| Expand role into concrete operation list at load or write time | Rejected — freezes the rule against the role definition at the moment of writing; breaks the "redefine a role, all rules update" requirement |

### Consequences

Redefining a role immediately affects every rule that references it without editing those rules. The stored rule text remains readable (role name visible). The roles table must be consulted on each evaluation, which is negligible overhead for the operation count involved.

## ADR: Built-in roles available implicitly; user `roles:` entries shadow built-ins

**ID:** builtin-roles-implicit-user-roles-shadow
**Plan:** add-allow-command-and-roles
**Status:** Accepted

### Context

Common permission patterns (`read-only`, `write`, `admin`) are useful out of the box. A decision was needed on whether to require explicit declaration or provide built-ins, and how to handle collisions between user-defined and built-in role names.

### Decision

`read-only`, `write`, and `admin` are always available without declaration. A user-defined role with the same name overrides the built-in.

### Options Considered

| Option | Verdict |
|--------|---------|
| Built-in roles implicit; user definitions shadow built-ins | Chosen — ergonomic defaults; one clear precedence rule (user wins); validated at load |
| Require explicit declaration of all roles | Rejected — less ergonomic; every policy would need boilerplate role declarations |
| Reserve built-in names (forbid user override) | Rejected — removes an escape hatch without safety benefit; user-defined override is validated at load |

### Consequences

Operators can use `read-only`, `write`, and `admin` without declaring them. They can narrow a built-in (for example redefine `write` to omit certain operations) by adding a `roles:` section. Load validation catches unknown role references and invalid operations in user-defined roles. A later plan (`add-release-lifecycle-ops`) added a fourth built-in, `maintain`, between `write` and `admin`.

## ADR: Broker is the sole policy-file writer; privilege gate on the daemon via SO_PEERCRED

**ID:** broker-sole-policy-writer-so-peercred-gate
**Plan:** add-allow-command-and-roles
**Status:** Accepted

### Context

`ghbrk allow` must write a new rule to `/etc/ghbrk/policy.yaml`. Two approaches exist: the CLI writes the file directly after `sudo` elevation, or the CLI sends a request and the broker performs the write. The threat model requires that the policy file stays owned and mutated by the privileged daemon.

### Decision

The CLI never writes `/etc/ghbrk/policy.yaml`. It sends an `allow` request; the broker checks effective UID 0 via `SO_PEERCRED`, validates the operands, writes (temp-then-rename), and reloads.

### Options Considered

| Option | Verdict |
|--------|---------|
| Broker validates and writes; privilege gate via SO_PEERCRED UID 0 | Chosen — trust boundary stays on the daemon; consistent with existing SO_PEERCRED identity model and audit logging |
| sudo-elevated CLI writes the file directly | Rejected — violates the privilege-separation mission; trust would depend on the agent process rather than the privileged daemon |

### Consequences

The policy file remains owned and mutated exclusively by the `ghbrk` daemon. The `allow` handler follows the same audit-log pattern as all other request types. Any future mutation operation can reuse the same privilege-gate pattern.

## ADR: Hot-reload via a swappable `arc-swap` policy handle

**ID:** hot-reload-via-swappable-arc-swap-policy-handle
**Plan:** add-allow-command-and-roles
**Status:** Accepted

### Context

The broker holds a single `Arc<Policy>` that is cloned per connection. `ghbrk allow` requires the daemon to observe a freshly written file without restart. An atomic, lock-free reload mechanism is needed so in-flight connections keep their snapshot and new connections see the updated policy.

### Decision

Replace `Arc<Policy>` with `Arc<ArcSwap<Policy>>`; each connection reads a snapshot via `.load()`; the allow handler swaps a freshly parsed policy in atomically via `.store()`. `arc-swap` is MIT OR Apache-2.0, compatible with ghbrk's MIT-only dependency policy.

### Options Considered

| Option | Verdict |
|--------|---------|
| `arc-swap` swappable handle | Chosen — lock-free reads; atomic swap; in-flight connections keep their snapshot; tiny, permissively-licensed crate |
| `RwLock<Arc<Policy>>` | Rejected — adds lock contention on the read path for every connection |
| Daemon restart on every policy change | Rejected — drops all in-flight connections; unacceptable for a live system |

### Consequences

Policy reloads are transparent to in-flight connections. New connections immediately evaluate against the updated policy after an `allow` write. The `arc-swap` crate is added to `Cargo.toml`; `cargo deny check` must pass.

## ADR: Policy file is `ghbrk:ghbrk` mode `0600`; doctor stat-checks it

**ID:** policy-file-mode-0600-doctor-stat-checks-it
**Plan:** add-allow-command-and-roles
**Status:** Accepted

### Context

The socket privilege gate (`SO_PEERCRED` UID 0) only governs daemon-mediated writes; it does not stop a local user from editing `/etc/ghbrk/policy.yaml` directly on disk. A misconfigured install could leave the file world-writable, bypassing the privilege gate entirely.

### Decision

`/etc/ghbrk/policy.yaml` is owned `ghbrk:ghbrk` with mode `0600`. `install.sh` creates it with that owner/mode (never overwriting existing content) and re-asserts owner/mode on re-run. `ghbrk doctor` adds a policy-permission check that `stat()`s the file and reports `Policy permissions: OK`/`WARNING`/`ERROR`, failing non-zero on a write-path exposure (wrong owner or group/other write bit).

### Options Considered

| Option | Verdict |
|--------|---------|
| `0600` owned by `ghbrk`; doctor stat-checks owner and mode | Chosen — tightest mode that still supports emergency manual edits via `sudo`; stat-check turns a silent misconfiguration into an explicit failure |
| `0640` owner `ghbrk:ghbrk-clients` (group-readable) | Rejected — clients have no need to read the raw file; widens the attack surface for no benefit |
| `0644` / world-readable | Rejected — the policy may encode org/repo structure that need not be world-readable; one `chmod` from world-writable |
| Rely solely on the socket gate | Rejected — the gate only governs daemon-mediated writes; direct on-disk edits are uncontrolled |

### Consequences

`ghbrk` daemon can read/write the policy file; `root` can edit it via `sudo`; all other users are denied both read and write. A re-run of `install.sh` corrects a drifted owner/mode. `doctor` surfaces a misconfigured file before it can be abused.

## ADR: Doctor permission audit is tiered: write-path = ERROR, read-path = WARNING

**ID:** doctor-permission-audit-tiered-error-warning
**Plan:** add-allow-command-and-roles
**Status:** Accepted

### Context

After adding the policy-file permission check (`policy-file-mode-0600-doctor-stat-checks-it`), the same direct-on-disk exposure exists for every other security-relevant path `ghbrk` owns: the config directory, the socket, the per-user credential directory, and the credential files. A tiered severity model was needed so `doctor` remains usable as a routine health check while still loudly failing on exposures that actively bypass the privilege model.

### Decision

`ghbrk doctor` audits the config directory, the policy file, the socket, the per-user credential directory, and each credential file. A write-path exposure (group/other write bit on a file; group/other write or execute bit on a directory; unexpected owner; socket connectable by non-group users) is `ERROR` and forces a non-zero exit. A read-path exposure (group/other read bit without write/execute) is `WARNING` and does not change exit status. `doctor` always runs every check, prints one line per check, and exits zero if and only if no `ERROR` was emitted.

### Options Considered

| Option | Verdict |
|--------|---------|
| Write-path = ERROR, read-path = WARNING | Chosen — maps directly onto the threat model; write access enables subversion, read access leaks information; keeps `doctor` usable as a routine health check |
| Every deviation = hard ERROR | Rejected — world-readable credential is a real concern but not an escalation; failing CI on it would train operators to ignore `doctor` |
| Every deviation = WARNING only | Rejected — world-writable policy file or 0666 socket is an active bypass of the privilege model; must fail loudly |
| Audit only the policy file (status quo) | Rejected — same exposure exists for socket, credential directory, and credential files |

### Consequences

`doctor` is usable as a routine, exit-code-driven health check. Write-path misconfigurations fail loudly. Read-path misconfigurations are surfaced without blocking automation. Every checked path is explicit and actionable in the output.

**2026-09-22 audit note (not part of the original decision):** the original text named the audited paths as `/run/ghbrk/ghbrk.sock` and `/var/lib/ghbrk/credentials/<user>/`. The actual paths are `/run/ghbrk/broker.sock` and `/etc/ghbrk/credentials/<user>/`. The tiering decision itself is unaffected; only the two path literals were wrong in the original text.

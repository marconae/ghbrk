# Decisions: add-pull-fix-path-coverage

## ADR: `pull` is a distinct `Operation` variant, not an alias for `fetch`

**ID:** pull-is-distinct-operation-variant
**Plan:** add-pull-fix-path-coverage
**Status:** Accepted

### Context

`git pull` is semantically richer than `git fetch`: it additionally mutates the working tree via merge or rebase. Operators may want to allow one without allowing the other. If `pull` were treated as `fetch`, that policy axis would be permanently collapsed with no way to separate the two.

### Decision

Add `Operation::Pull` as a new enum variant in `src/policy.rs`, serialised as `pull` in YAML. A rule listing `fetch` does not match a `pull` request and vice versa.

### Options Considered

| Option | Verdict |
|--------|---------|
| `Operation::Pull` as a distinct variant | Chosen — preserves the policy axis; minimal change; serialises cleanly as `"pull"` in YAML |
| Reuse `Operation::Fetch` for both `git fetch` and `git pull` | Rejected — permanently erases the ability to allow one without the other |
| Add a sub-flag on `Fetch` (e.g. `Fetch { merge: bool }`) | Rejected — complicates the YAML schema and rule-matching code for negligible benefit |

### Consequences

Operators can write separate rules for `fetch` and `pull`. Existing policies that allow `fetch` do not implicitly allow `pull`. The operations vocabulary now includes `pull` as a first-class member; unknown-operation loading validation treats it exactly like any other member of the set.

## ADR: Coverage gap mitigated via `/usr/local/bin` symlinks in `install.sh`

**ID:** coverage-gap-mitigated-via-usr-local-bin-symlinks
**Plan:** add-pull-fix-path-coverage
**Status:** Superseded by explicit-gateway-replaces-transparent-shim

### Context

Without shim coverage, agent processes that resolve `git` or `gh` by name through `PATH` bypass ghbrk when no symlink exists at a PATH location that precedes `/usr/bin`. The ghbrk binary lives at `/usr/local/bin/ghbrk` but nothing in that directory shadows `/usr/bin/git` or `/usr/bin/gh`.

### Decision

`install.sh` creates `/usr/local/bin/git` and `/usr/local/bin/gh` as symlinks pointing to `/usr/local/bin/ghbrk`. `/usr/local/bin` precedes `/usr/bin` in the standard Linux system PATH, so PATH-resolved `git`/`gh` invocations are routed through the shim without touching the canonical binaries.

### Options Considered

| Option | Verdict |
|--------|---------|
| `/usr/local/bin` symlinks via `install.sh` | Chosen — non-invasive, reversible (`rm`), idempotent (`ln -sfn`), standard Linux mechanism |
| Replace `/usr/bin/git` itself with the shim | Rejected — system package updates clobber the change; other tooling depends on the canonical location |
| `LD_PRELOAD` to intercept `execve("/usr/bin/git", ...)` | Rejected — fragile; breaks for statically linked callers; requires per-process environment setup |
| systemd `BindPaths` to overlay `/usr/bin/git` for the daemon unit | Rejected — the gap is in agent processes, not in the daemon; systemd directives do not apply outside their unit |
| Kernel-level interception (eBPF, seccomp filters) | Rejected — excessive engineering for a defence-in-depth gap |

### Consequences

Most PATH-resolved `git`/`gh` callers (shells, agents, CI scripts) are automatically routed through ghbrk after installation. Callers that hardcode `/usr/bin/git` by absolute path are explicitly out of scope (see `absolute-path-callers-out-of-scope`). The `install.sh` idempotency contract requires checking for conflicting non-symlink files before creating each link.

## ADR: Callers using absolute path `/usr/bin/git` are explicitly out of scope

**ID:** absolute-path-callers-out-of-scope
**Plan:** add-pull-fix-path-coverage
**Status:** Superseded by explicit-gateway-replaces-transparent-shim

### Context

The `/usr/local/bin` symlink approach (`coverage-gap-mitigated-via-usr-local-bin-symlinks`) covers callers that resolve `git`/`gh` through `PATH`. Callers that bypass PATH by invoking `/usr/bin/git` directly are not covered. A decision was needed on whether to pursue coverage of this class of callers.

### Decision

Document that ghbrk does not intercept calls that bypass PATH entirely. Mitigation for those callers is left to the operator (for example policy at the host level, or reconfiguring the offending tool).

### Options Considered

| Option | Verdict |
|--------|---------|
| Accept the scope gap; document it | Chosen — scope discipline; the 80% case (PATH-resolved callers) is covered cleanly |
| Replace `/usr/bin/git` itself | Rejected — see `coverage-gap-mitigated-via-usr-local-bin-symlinks` |
| `LD_PRELOAD`, eBPF, seccomp | Rejected — see `coverage-gap-mitigated-via-usr-local-bin-symlinks` |

### Consequences

Operators must understand that hardcoded-path callers bypass ghbrk. The limitation is documented so operators can make informed deployment decisions.

## ADR: EACCES on broker socket connect triggers silent automatic passthrough

**ID:** eacces-triggers-silent-automatic-passthrough
**Plan:** add-pull-fix-path-coverage
**Status:** Superseded by ghbrk-scope-is-remote-authenticated-ops-only

### Context

The `/usr/local/bin` symlinks introduced in the same plan route every PATH-resolved `git`/`gh` invocation through the shim — including invocations by unprivileged tools (for example package managers, IDEs, `uvx` sub-processes) that have no relationship to ghbrk. When such a tool runs as a user without filesystem permission to the broker socket (`EACCES`, errno 13), the existing hard-fail behaviour prints a ghbrk error to stderr and exits non-zero, breaking the caller.

An earlier amendment proposed an opt-in `fallback_on_broker_error` config field and `GHBRK_FALLBACK` env var. That approach was rejected because it makes the symlink feature ship broken by default for unprivileged callers — the operator action required (edit config, restart, or wrap every invocation) is friction that ensures the feature is unusable out of the box.

### Decision

When `UnixStream::connect` returns `EACCES` (errno 13), the shim unconditionally and silently execs the real binary with the original arguments. No config flag, no env var, no `ShimConfig` change. All other connect errors (`ENOENT`, `ECONNREFUSED`, etc.) retain the existing hard-fail behaviour.

### Options Considered

| Option | Verdict |
|--------|---------|
| Hardcoded EACCES-only silent passthrough | Chosen — automatic, zero operator friction, no security regression (process already cannot reach the broker), silent to avoid corrupting captured stderr |
| Opt-in `fallback_on_broker_error` + `GHBRK_FALLBACK` env var | Rejected — ships the symlink feature broken by default for unprivileged callers; operator friction makes it unusable out of the box |
| Fall back on every connect error | Rejected — ENOENT and ECONNREFUSED indicate broker service is down, a real deployment problem that must remain visible |
| Auto-detect socket permissions before connecting | Rejected — TOCTOU race, more code, no real benefit over checking the actual connect errno |

### Consequences

Unprivileged tools that cannot reach the broker socket get transparent passthrough to the real `git`/`gh` binary with no stderr noise. Operators who install the system-wide symlinks accept this behaviour implicitly — no configuration is required. The broker retains hard-fail behaviour for ENOENT and ECONNREFUSED, so deployment failures remain visible. The literal `13` is used instead of adding a `libc` dependency for a single POSIX-stable constant.

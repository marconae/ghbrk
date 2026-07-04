# Architecture Decision Records

<!-- ADRs are numbered sequentially starting from ADR-001. Never renumber. -->
<!-- recorder-agent appends new ADRs from plan decision logs. -->

---

## ADR-001: Passthrough gate lives in the shim, in front of broker contact

**Date:** 2026-05-27
**Plan:** `fix-local-git-gh-passthrough`
**Status:** Superseded by ADR-017

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
**Status:** Superseded by ADR-017

### Context

When the shim decides a command is passthrough, it must hand execution to the real binary. The approach chosen determines how stdio, signals, tty control, and exit codes are handled.

> Superseded note (`change-explicit-gateway`): client-side shim passthrough is removed. Broker-side `gh` passthrough is retained, but execution flows through the daemon executor and streaming pipeline rather than client-side `exec()`.

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

---

## ADR-004: `pull` is a distinct `Operation` variant, not an alias for `fetch`

**Date:** 2026-05-28
**Plan:** `add-pull-fix-path-coverage`
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

---

## ADR-005: Coverage gap mitigated via `/usr/local/bin` symlinks in `install.sh`

**Date:** 2026-05-28
**Plan:** `add-pull-fix-path-coverage`
**Status:** Superseded by ADR-017

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

Most PATH-resolved `git`/`gh` callers (shells, agents, CI scripts) are automatically routed through ghbrk after installation. Callers that hardcode `/usr/bin/git` by absolute path are explicitly out of scope (see ADR-006). The `install.sh` idempotency contract requires checking for conflicting non-symlink files before creating each link.

---

## ADR-006: Callers using absolute path `/usr/bin/git` are explicitly out of scope

**Date:** 2026-05-28
**Plan:** `add-pull-fix-path-coverage`
**Status:** Superseded by ADR-017

### Context

The `/usr/local/bin` symlink approach (ADR-005) covers callers that resolve `git`/`gh` through `PATH`. Callers that bypass PATH by invoking `/usr/bin/git` directly are not covered. A decision was needed on whether to pursue coverage of this class of callers.

### Decision

Document that ghbrk does not intercept calls that bypass PATH entirely. Mitigation for those callers is left to the operator (e.g. policy at the host level, or reconfiguring the offending tool).

### Options Considered

| Option | Verdict |
|--------|---------|
| Accept the scope gap; document it | Chosen — scope discipline; the 80% case (PATH-resolved callers) is covered cleanly |
| Replace `/usr/bin/git` itself | Rejected — see ADR-005 |
| `LD_PRELOAD`, eBPF, seccomp | Rejected — see ADR-005 |

### Consequences

Operators must understand that hardcoded-path callers bypass ghbrk. The limitation is documented so operators can make informed deployment decisions.

---

## ADR-007: EACCES on broker socket connect triggers silent automatic passthrough

**Date:** 2026-05-28
**Plan:** `add-pull-fix-path-coverage`
**Status:** Superseded by ADR-018

### Context

The `/usr/local/bin` symlinks introduced in the same plan route every PATH-resolved `git`/`gh` invocation through the shim — including invocations by unprivileged tools (e.g. package managers, IDEs, `uvx` sub-processes) that have no relationship to ghbrk. When such a tool runs as a user without filesystem permission to the broker socket (`EACCES`, errno 13), the existing hard-fail behaviour prints a ghbrk error to stderr and exits non-zero, breaking the caller.

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

---

## ADR-008: Set daemon's primary group to `ghbrk-clients` instead of relying on runtime `chown`

**Date:** 2026-05-28
**Plan:** `fix-install-one-line`
**Status:** Accepted

### Context

`ghbrk.service` ran the daemon with `Group=ghbrk`. When the broker bound the socket, the socket file inherited the daemon's primary group (`ghbrk`). `apply_socket_group` then attempted `chown(socket, None, ghbrk-clients-gid)`, but a non-root process can only `chown` files to groups it is a member of — and the `ghbrk` user was not in `ghbrk-clients`. The `chown` failed with `EPERM`, logged only at `warn!` level, and the socket ended up `ghbrk:ghbrk` mode `0660`. Any agent user (in `ghbrk-clients` but not `ghbrk`) received `EACCES` on connect, silently triggering the passthrough fallback and bypassing the broker entirely.

### Decision

Change `Group=ghbrk` to `Group=ghbrk-clients` in `deploy/linux/ghbrk.service`. The daemon's primary GID becomes `ghbrk-clients`, so the socket file is created with that group on the first `bind(2)` call — no `chown` step required for correctness. `apply_socket_group` stays as a defence-in-depth check but logs at `error!` level with the systemd directive named in the message. `install.sh` also runs `usermod -aG ghbrk-clients ghbrk` so the defence-in-depth path can succeed if the unit is ever misconfigured.

### Options Considered

| Option | Verdict |
|--------|---------|
| `Group=ghbrk-clients` as daemon's primary group | Chosen — socket is correctly grouped from the first `bind(2)` byte; no TOCTOU window before chown runs |
| Keep `Group=ghbrk` and add `ghbrk` to `ghbrk-clients` via `usermod` so the runtime `chown` succeeds | Rejected — leaves the socket owned by the daemon's primary group (`ghbrk`) on the first `bind()` and relies on the `chown` completing before any client connects |

### Consequences

The socket is correctly grouped from the first byte on every service start, including the instant between `bind(2)` and any subsequent `chown`. The defence-in-depth `chown` path now exists only to catch misconfigurations; when it fires it logs at `error!` and names the unit directive as the fix so operators can locate and correct the misconfiguration without consulting source code.

---

## ADR-009: Use `RuntimeDirectory=ghbrk` mode `2750` for the socket parent directory

**Date:** 2026-05-28
**Plan:** `fix-install-one-line`
**Status:** Superseded by ADR-011

### Context

`/var/run` is a `tmpfs` mount on modern Linux distributions. The install script created `/var/run/ghbrk/` once, but the directory disappeared on the next reboot, leaving the daemon unable to bind the socket. The unit had no directive to recreate it. Two alternatives were considered: a `tmpfiles.d` snippet and keeping `install.sh`'s `mkdir -p` as the sole mechanism.

### Decision

Add `RuntimeDirectory=ghbrk` and `RuntimeDirectoryMode=2750` to the unit's `[Service]` section. Remove `/var/run/ghbrk` from `ReadWritePaths=`. The `install.sh` `mkdir -p /var/run/ghbrk` line is kept for the non-systemd direct-launch case but is no longer the primary mechanism.

### Options Considered

| Option | Verdict |
|--------|---------|
| `RuntimeDirectory=ghbrk` with mode `2750` in the unit | Chosen — canonical systemd mechanism; directory recreated on every service start with declared ownership and mode; removed cleanly on stop; `ReadWritePaths=` no longer needs to mention it |
| `tmpfiles.d` snippet creating the directory at boot | Rejected — adds a second moving part with no benefit; lifecycle is owned by a separate config file rather than the unit |
| Leave `install.sh` `mkdir -p` as the only mechanism | Rejected — directory is on `tmpfs` and disappears on reboot, leaving the daemon unable to bind |

### Consequences

`/run/ghbrk/` is recreated on every service start with owner `ghbrk:ghbrk-clients` and mode `2750`. The setgid bit means any file created inside (the socket, future audit shards) inherits the directory group, acting as a second seatbelt on socket group ownership. Removing `/var/run/ghbrk` from `ReadWritePaths=` eliminates the confusing dual-declaration and makes `RuntimeDirectory=` the single source of truth for the socket directory lifecycle.

---

## ADR-010: `install.sh` enables and restarts the service and wires users into `ghbrk-clients`

**Date:** 2026-05-28
**Plan:** `fix-install-one-line`
**Status:** Accepted

### Context

`install.sh` ended with instructions to manually run `systemctl enable` and `systemctl start ghbrk`. The `ghbrk` daemon user and `$SUDO_USER` were not automatically joined to `ghbrk-clients`, requiring further manual operator steps. The stated contract for the installer was: one privileged command (`sudo ./deploy/linux/install.sh`), then copy credentials and edit policy. Printing follow-up instructions violated that contract.

### Decision

After `systemctl daemon-reload`, run `systemctl enable ghbrk` and `systemctl restart ghbrk` guarded by `command -v systemctl`. Run `usermod -aG ghbrk-clients ghbrk` (idempotent) after user/group creation. When `$SUDO_USER` is set and non-empty, run `usermod -aG ghbrk-clients "$SUDO_USER"` and print a "log out and back in" notice. When `$SUDO_USER` is empty, print a manual-add instruction. Replace the closing banner with a summary of what the script did plus the remaining manual steps (copy credentials, edit policy).

### Options Considered

| Option | Verdict |
|--------|---------|
| `install.sh` enables and restarts the service and wires users into groups | Chosen — upholds the one-line install contract; `usermod -aG` is additive and idempotent; `restart` covers both first-run and re-run |
| Keep printing manual `systemctl enable`/`start` instructions | Rejected — violates the one-line install contract |
| Use `setgid`/`newgrp` magic to inject the new group into the existing shell session | Rejected — no portable mechanism to inject a supplementary group into an existing login session without spawning a new shell |

### Consequences

`sudo ./deploy/linux/install.sh` leaves the broker running with the socket reachable by every member of `ghbrk-clients` and the installing user already in that group. Operators still need to log out and back in (or use `newgrp`) before their current shell session reflects the new group membership. The `usermod -aG` invocations are idempotent so re-runs are safe. `systemctl restart` (not `start`) means re-runs after editing the unit pick up new directives without erroring on "already running."

---

## ADR-011: Use `tmpfiles.d` snippet and `ReadWritePaths=` for the socket parent directory

**Date:** 2026-05-28
**Plan:** `fix-runtimedir-namespace` (hotfix)
**Status:** Accepted

### Context

ADR-009 chose `RuntimeDirectory=ghbrk` as the canonical systemd mechanism for recreating `/run/ghbrk/` on every service start. Post-deploy inspection of `/proc/<pid>/mountinfo` revealed that `RuntimeDirectory=` combined with `ProtectSystem=strict` creates the directory inside the service's **private mount namespace**. The socket bound there is invisible to processes running in the host namespace — including the shim. Every shim connection hit `ENOENT` on the socket path, triggering the EACCES silent-fallthrough path and causing the broker to be bypassed entirely.

### Decision

Replace `RuntimeDirectory=ghbrk` and `RuntimeDirectoryMode=2750` with a `tmpfiles.d(5)` snippet (`deploy/linux/ghbrk.tmpfiles`, installed to `/etc/tmpfiles.d/ghbrk.conf`) that creates `/run/ghbrk` on the **host's** `/run` tmpfs at every boot. Add `ReadWritePaths=/run/ghbrk` to the unit so the daemon can write the socket there under `ProtectSystem=strict`. `install.sh` installs the snippet and calls `systemd-tmpfiles --create` to create the directory immediately without requiring a reboot.

### Options Considered

| Option | Verdict |
|--------|---------|
| `RuntimeDirectory=ghbrk` in the unit (ADR-009) | Superseded — with `ProtectSystem=strict` the directory is created in the service's private mount namespace; the socket is not visible to host-namespace processes |
| `tmpfiles.d` snippet + `ReadWritePaths=/run/ghbrk` | Chosen — directory is created on the host's `/run` tmpfs and is visible to all processes; lifecycle managed by `systemd-tmpfiles` at every boot |
| Socket activation (`.socket` unit) | Rejected — would require protocol changes and adds complexity with no additional security benefit |

### Consequences

`/run/ghbrk/` is created on the host's `/run` tmpfs at every boot with owner `ghbrk:ghbrk-clients` and mode `2750`. The socket is visible to shim processes in the host namespace. `install.sh` now installs a second artefact (`ghbrk.tmpfiles`) alongside the unit file. The `ReadWritePaths=` entry makes the socket directory lifecycle explicit in the unit file itself.

---

## ADR-012: Route `gh api` through the broker instead of passthrough

**Date:** 2026-05-29
**Plan:** `add-check-command-and-gh-integration-test`
**Status:** Accepted

### Context

`gh api <path>` was treated as passthrough, allowing any agent to call arbitrary GitHub read endpoints using whatever token was in its own environment — bypassing the broker entirely and defeating the privilege-separation goal for the most common scripted GitHub access pattern. Operators had no mechanism to policy-gate or audit `gh api` calls.

### Decision

`gh api <path>` is classified as broker-mediated. A new `Operation::GhApiRead { path }` flows through the existing shim → resolver → policy → executor pipeline, policy-gated by a `gh_api_read` rule and credential-injected with `GH_TOKEN`.

### Options Considered

| Option | Verdict |
|--------|---------|
| Route `gh api` through the broker (Option A) | Chosen — closes the largest bypass of broker mediation; makes API reads policy-gated and audited with the existing pipeline |
| Leave `gh api` as passthrough (Option B) | Rejected — lets agents call arbitrary GitHub read endpoints with their own ambient token, bypassing broker mediation and audit entirely |

### Consequences

`gh api` calls are now policy-gated and recorded in the audit log. Agents that previously relied on ambient `GH_TOKEN` passthrough must have a `gh_api_read` rule in the policy. Write methods (`gh api -X POST/PATCH/DELETE`) and GraphQL remain default-denied; they require distinct operations introduced in a future plan.

---

## ADR-013: Evaluate `gh_api_read` on the user only (wildcard org/repo)

**Date:** 2026-05-29
**Plan:** `add-check-command-and-gh-integration-test`
**Status:** Accepted

### Context

The `gh api` path space is not uniformly repo-scoped. Paths like `/user`, `/rate_limit`, and `/orgs/acme` carry no repo component. Deriving org and repo from the path would be unreliable for the general case and inconsistent across path patterns.

### Decision

The resolver emits unset/wildcard org and repo for `gh_api_read`. Policy authorises it via a user-scoped rule with `org: "*"` and `repo: "*"`. Branch is ignored (`has_branch() == false`). Default-deny still applies when no rule grants the operation.

### Options Considered

| Option | Verdict |
|--------|---------|
| User-scoped wildcard org/repo | Chosen — correct and consistent across all API paths; keeps the model simple; default-deny still protects against unauthorised access |
| Parse org/repo out of the API path (e.g. `repos/acme/web`) | Rejected — API paths are not uniformly repo-scoped; parsing would be unreliable and inconsistent for paths like `/user` or `/rate_limit` |

### Consequences

Operators write a single user-scoped rule to authorise all `gh api` read calls for a user. Per-repo granularity for API reads is not available; it is deferred to a future plan if needed. The user-scoping model is consistent with how branch-less operations like `clone` and `fetch` are already handled.

---

## ADR-014: Mock GitHub API over TLS so `gh api` integration tests always run

**Date:** 2026-05-29
**Plan:** `add-check-command-and-gh-integration-test`
**Status:** Accepted

### Context

The `gh api` harness tests previously skipped gracefully when `GH_TOKEN` was absent, providing no proof of the broker path in a default environment (local dev, CI without a configured secret). The `gh` CLI enforces HTTPS even when `GH_HOST` points at a non-`github.com` host and does not skip certificate verification, so a plain HTTP mock is refused.

### Decision

A `mock-github` HTTPS service is added to the Docker compose stack, returning a fixed JSON body for `GET /api/v3/user`. The mock serves real TLS via a pre-generated, self-signed test CA and server certificate (CN/SAN `mock-github`) committed under `tests/integration/certs/`. The `devenv` image installs the CA into its system trust store at build time. Harness tests point the broker at `GH_HOST=mock-github` with a synthetic token and assert stdout contains `"login": "test-user"`. A curl-based TLS smoke test from `devenv` proves trust independently of the broker, and a missing-token case asserts non-zero exit. The tests no longer skip gracefully — they always run when Docker is available.

### Options Considered

| Option | Verdict |
|--------|---------|
| HTTPS mock with committed self-signed test CA | Chosen — exercises the real `gh api` → broker → credential-injection path; always provides proof; no real token; no network dependency |
| Graceful skip when `GH_TOKEN` absent | Rejected — provides no proof in a default/CI-without-secret environment (the original approach) |
| Plain HTTP mock | Rejected — `gh` enforces HTTPS even for non-github.com `GH_HOST` and refuses non-HTTPS connections |
| Transparent TLS interception (mitmproxy-style) | Rejected — heavier dependency and more moving parts than a fixed-response mock requires |

### Consequences

`gh api` tests always run and prove the broker path whenever Docker is available, with no real token and no network dependency. Self-signed certs scoped to the Docker test network carry no security risk. The committed certs (~10-year validity) must be regenerated before they expire; the `openssl` commands are documented under `tests/integration/certs/` for rotation.

---

## ADR-015: Route `ghbrk check` through the broker instead of reading credentials client-side

**Date:** 2026-05-29
**Plan:** `fix-credential-access-check-and-gh-passthrough`
**Status:** Superseded by ADR-017

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

---

## ADR-016: Centralise all `gh` broker/passthrough classification on the broker side

**Date:** 2026-05-29
**Plan:** `fix-credential-access-check-and-gh-passthrough`
**Status:** Accepted

### Context

The shim previously classified `gh` invocations locally: broker-ops (`pr`, `issue`, `release`, `api`) were routed to the broker; everything else was exec'd directly as passthrough. Passthrough `gh` calls had no access to `GH_TOKEN` — the token is owned by the `ghbrk` user and not available in the agent's environment. Commands like `gh repo view` and `gh auth status` failed with authentication errors even though the system held the token.

### Decision

`cmd/gh.rs` always connects to the broker for every `gh` invocation. The broker calls `gh_is_broker_op` to classify: true → existing policy pipeline; false → load credentials, inject `GH_TOKEN`, exec real `gh` as credential-injected passthrough, record an `AuditDecision::Passthrough` entry. The shim-side classification for `gh` is removed entirely.

### Options Considered

| Option | Verdict |
|--------|---------|
| Centralise `gh` classification on the broker side | Chosen — the broker is the only process with access to `GH_TOKEN`; any shim-side passthrough exec leaves the agent without a token; classification lives where the token lives |
| Keep shim-side `is_passthrough` for gh and inject tokens only on broker-op calls | Rejected — passthrough `gh` invocations still fail to authenticate; the agent has no `GH_TOKEN` |

### Consequences

All `gh` invocations receive `GH_TOKEN` injection. The broker audit log now records passthrough `gh` calls with `decision=passthrough`, keeping them visible and distinguishable from policy-evaluated `Allow` decisions. The `gh_is_broker_op` function is made `pub` so the broker can call it from `broker.rs`. The operator's access gate remains unchanged: `ghbrk-clients` group membership controls whether the gateway client can reach the broker socket.

---

## ADR-017: Explicit gateway replaces the transparent shim

**Date:** 2026-05-30
**Plan:** `change-explicit-gateway`
**Status:** Accepted

### Context

The transparent shim symlinked `ghbrk` as `git` and `gh` early in the agent's `PATH`, silently intercepting every invocation and classifying it client-side into local-passthrough vs broker-mediated. For an AI agent this makes privileged, machine-leaving behaviour invisible: the agent cannot tell from the command alone whether it is hitting the network under brokered credentials, and the client-side classifier must perfectly mirror git/gh semantics or it silently misroutes. Invisible privileged authority gives agents no way to reason about the security boundary.

### Decision

Remove all transparent PATH-interception: no argv[0] symlink dispatch, no client-side local/remote passthrough classifier, and no shim config for real-binary paths. Privileged authority is requested explicitly by name via `ghbrk git <remote-subcommand>` and `ghbrk gh <subcommand>`. The security boundary becomes part of the interface.

### Options Considered

| Option | Verdict |
|--------|---------|
| Explicit `ghbrk git`/`ghbrk gh` verb gateway, no symlinks | ✓ Chosen — privilege is requested by name and never inferred; the boundary is inspectable rather than hidden |
| Keep an optional `install-shims` transparent compat mode | ✗ Rejected — a hidden mode re-introduces the invisible-privilege problem the redesign exists to eliminate |

### Consequences

Agents call plain `git`/`gh` for local work and `ghbrk git`/`ghbrk gh` only when an operation leaves the machine. No symlinks are created at install time, and there is no `install-shims` step. The mental model is crisp: ghbrk does exactly one thing — broker remote operations. Existing automation that relied on transparent interception must be updated to call the gateway explicitly (breaking change; crate bumped to 0.5.0).

---

## ADR-018: ghbrk scope is remote/authenticated operations only

**Date:** 2026-05-30
**Plan:** `change-explicit-gateway`
**Status:** Accepted

### Context

With the explicit gateway, a decision was needed on what `ghbrk git <local-subcommand>` (e.g. `status`, `log`, `commit`) should do. Allowing it to passthrough-exec the local binary would re-create the client-side classifier and the confusing "is this brokered?" mental model the redesign removes.

### Decision

`ghbrk git <local-subcommand>` returns a clear guidance error before any socket connection, telling the user to run the command with plain `git`. Only machine-leaving (remote/authenticated) operations are relayed to the broker. `ghbrk` constrains itself strictly to the remote/authenticated boundary.

### Options Considered

| Option | Verdict |
|--------|---------|
| Reject local git subcommands with a pre-connect guidance error | ✓ Chosen — keeps the authority boundary crisp; ghbrk only ever brokers remote operations |
| Let `ghbrk git status` passthrough-exec the local binary | ✗ Rejected — re-creates the client-side classifier and the confusing brokered-or-not mental model |

### Consequences

The gateway never executes local git. Users get an immediate, actionable error directing them to plain `git`. As defence-in-depth the broker still resolves every request and denies any local-only subcommand that reaches the socket from a hand-crafted client, so the default-deny invariant holds at the trust boundary.

---

## ADR-019: Resolver stays broker-side; feature relocated to the daemon domain

**Date:** 2026-05-30
**Plan:** `change-explicit-gateway`
**Status:** Accepted

### Context

The resolver maps `(tool, args, cwd)` to a normalised `(operation, org, repo, branch?)` tuple. It already ran broker-side (`src/broker.rs::resolve_request`) but its spec feature was filed under the now-removed `shim/` domain. With the shim gone, a decision was needed on where resolution belongs and where its spec should live.

### Decision

Keep the resolver in the broker, unchanged, and relocate its spec feature from `shim/` to `daemon/resolver`. The gateway client stays a thin relay; the broker remains the single authoritative mapping from command to operation.

### Options Considered

| Option | Verdict |
|--------|---------|
| Keep resolver broker-side; relocate the feature to `daemon/resolver` | ✓ Chosen — resolution was always a broker-side concern; a single authoritative mapping stays inside the trust boundary |
| Delete the resolver and have the client send a pre-resolved tuple | ✗ Rejected — leaks repo-context parsing out of the trust boundary and lets a malicious client spoof the resolved operation |

### Consequences

Parsing and repo-context logic stay inside the privileged daemon, so a client cannot spoof the operation it is requesting. The relocation is behaviour-preserving — all 15 resolver scenarios move unchanged to `daemon/resolver`. The client is reduced to relaying `(tool, args, cwd)` and streaming the response.

---

## ADR-020: Drop privilege in the child, not the daemon

**Date:** 2026-05-30
**Plan:** `change-executor-privilege-drop`
**Status:** Accepted

### Context

The daemon runs as the `ghbrk` system user. Child `git`/`gh` processes inherited that identity, preventing traversal of a `0700` home directory or writes to user-owned repositories. The workaround — `chmod o+x ~` — was fragile, easy to forget, and weakened the home directory boundary for all `other` users.

### Decision

The daemon stays running as `ghbrk`; only the forked child drops to the peer user's UID/GID/supplementary groups via `CommandExt::uid()`/`gid()` plus a `setgroups()` call in `pre_exec`.

### Options Considered

| Option | Verdict |
|--------|---------|
| Drop privilege in the child, not the daemon | Chosen — per-child drop gives the child exactly the user's permissions with no standing access for the daemon; preserves the single-process multi-user architecture; removes the manual `chmod` setup step |
| Run a separate daemon process per user | Rejected — heavy; breaks the single-daemon model |
| Keep `chmod o+x ~` workaround | Rejected — fragile, easy to forget, weakens the home directory boundary for all `other` users |
| Use filesystem ACLs on home directories | Rejected — unmaintainable; still grants `ghbrk` standing access |

### Consequences

Each child process runs with exactly the requesting user's UID, GID, and supplementary groups. The daemon's identity is unchanged. Home directories with default `0700` mode are traversable by the child without any permission change. The `chmod o+x ~` setup step is removed from the README and install documentation.

---

## ADR-021: Switch `ProtectHome=read-only` to `ProtectHome=no`

**Date:** 2026-05-30
**Plan:** `change-executor-privilege-drop`
**Status:** Accepted

### Context

The systemd unit had `ProtectHome=read-only`, which mounts home directories read-only inside the service's namespace. With executor privilege drop, child processes run as the requesting user and need write access to repositories for `git fetch`/`git pull`. A read-only home mount inside the service namespace defeats write operations even for the correctly-identified child.

### Decision

The systemd unit sets `ProtectHome=no` so user-owned children can write repositories under user home directories. The security boundary now comes from the UID/GID drop, not the namespace mount.

### Options Considered

| Option | Verdict |
|--------|---------|
| `ProtectHome=no` | Chosen — restores write access; security boundary comes from per-child privilege drop; minimal change |
| Keep `ProtectHome=read-only` | Rejected — blocks writes by the user-owned child, defeating the privilege drop for write operations |
| Enumerate per-user `ReadWritePaths` for every home directory | Rejected — impossible to maintain across arbitrary users and home directory layouts |

### Consequences

Child processes spawned as the requesting user can write to repositories under that user's home directory. The service no longer restricts home directory access via the namespace mount; home directory security relies on standard Unix permission checks with the correctly-dropped child identity.

---

## ADR-022: Keep `NoNewPrivileges=true` while adding `CAP_SETUID`/`CAP_SETGID`

**Date:** 2026-05-30
**Plan:** `change-executor-privilege-drop`
**Status:** Accepted

### Context

The systemd unit needed `CAP_SETUID` and `CAP_SETGID` (via `AmbientCapabilities` and `CapabilityBoundingSet`) so the daemon can drop child processes to the requesting user. A question arose whether `NoNewPrivileges=true` would block the `setuid(2)`/`setgid(2)` syscalls.

### Decision

Retain `NoNewPrivileges=true`; grant the two capabilities via `AmbientCapabilities` and bound them with `CapabilityBoundingSet=CAP_SETUID CAP_SETGID`.

### Options Considered

| Option | Verdict |
|--------|---------|
| Retain `NoNewPrivileges=true` | Chosen — does not block `setuid(2)` when `CAP_SETUID` is already held (no SUID transition involved); still blocks privilege escalation via SUID binaries; preserves defence-in-depth |
| Drop `NoNewPrivileges=true` | Rejected — unnecessary; weakens hardening without any benefit |

### Consequences

`NoNewPrivileges=true` does not interfere with the `setuid(2)`/`setgid(2)` syscalls used for the privilege drop, because those capabilities are already held via `AmbientCapabilities`. SUID-binary escalation is still blocked. The capability set is scoped to exactly `CAP_SETUID` and `CAP_SETGID`.

---

## ADR-024: Roles resolved at evaluation time, stored as role-name strings

**Date:** 2026-06-03
**Plan:** `add-allow-command-and-roles`
**Status:** Accepted

### Context

When a policy rule references a named role (e.g. `operations: write`), the rule could expand the role into a concrete operation list at load or write time, or store the role name literally and resolve it on every evaluation. Expanding at load/write time freezes the rule against the role definition at the moment of writing and breaks the requirement that redefining a role immediately affects all referencing rules.

### Decision

A rule's `operations` field may hold a role name (string) or an inline operation list. The role name is stored literally and resolved against the roles table on every `evaluate()`.

### Options Considered

| Option | Verdict |
|--------|---------|
| Store role name literally; resolve at evaluation time | ✓ Chosen — directly satisfies the requirement; one role edit fans out to all referencing rules; keeps rules human-readable |
| Expand role into concrete operation list at load or write time | ✗ Rejected — freezes the rule against the role definition at the moment of writing; breaks the "redefine a role, all rules update" requirement |

### Consequences

Redefining a role immediately affects every rule that references it without editing those rules. The stored rule text remains readable (role name visible). The roles table must be consulted on each evaluation, which is negligible overhead for the operation count involved.

---

## ADR-025: Built-in roles available implicitly; user `roles:` entries shadow built-ins

**Date:** 2026-06-03
**Plan:** `add-allow-command-and-roles`
**Status:** Accepted

### Context

Three common permission patterns (`read-only`, `write`, `admin`) are useful out of the box. A decision was needed on whether to require explicit declaration or provide built-ins, and how to handle collisions between user-defined and built-in role names.

### Decision

`read-only`, `write`, and `admin` are always available without declaration. A user-defined role with the same name overrides the built-in.

### Options Considered

| Option | Verdict |
|--------|---------|
| Built-in roles implicit; user definitions shadow built-ins | ✓ Chosen — ergonomic defaults; one clear precedence rule (user wins); validated at load |
| Require explicit declaration of all roles | ✗ Rejected — less ergonomic; every policy would need boilerplate role declarations |
| Reserve built-in names (forbid user override) | ✗ Rejected — removes an escape hatch without safety benefit; user-defined override is validated at load |

### Consequences

Operators can use `read-only`, `write`, and `admin` without declaring them. They can narrow a built-in (e.g. redefine `write` to omit certain operations) by adding a `roles:` section. Load validation catches unknown role references and invalid operations in user-defined roles.

---

## ADR-026: Broker is the sole policy-file writer; privilege gate on the daemon via SO_PEERCRED

**Date:** 2026-06-03
**Plan:** `add-allow-command-and-roles`
**Status:** Accepted

### Context

`ghbrk allow` must write a new rule to `/etc/ghbrk/policy.yaml`. Two approaches exist: the CLI writes the file directly after `sudo` elevation, or the CLI sends a request and the broker performs the write. The threat model requires that the policy file stays owned and mutated by the privileged daemon.

### Decision

The CLI never writes `/etc/ghbrk/policy.yaml`. It sends an `allow` request; the broker checks effective UID 0 via `SO_PEERCRED`, validates the operands, writes (temp-then-rename), and reloads.

### Options Considered

| Option | Verdict |
|--------|---------|
| Broker validates and writes; privilege gate via SO_PEERCRED UID 0 | ✓ Chosen — trust boundary stays on the daemon; consistent with existing SO_PEERCRED identity model and audit logging |
| sudo-elevated CLI writes the file directly | ✗ Rejected — violates the privilege-separation mission; trust would depend on the agent process rather than the privileged daemon |

### Consequences

The policy file remains owned and mutated exclusively by the `ghbrk` daemon. The `allow` handler follows the same audit-log pattern as all other request types. Any future mutation operation can reuse the same privilege-gate pattern.

---

## ADR-027: Hot-reload via a swappable `arc-swap` policy handle

**Date:** 2026-06-03
**Plan:** `add-allow-command-and-roles`
**Status:** Accepted

### Context

The broker holds a single `Arc<Policy>` that is cloned per connection. `ghbrk allow` requires the daemon to observe a freshly written file without restart. An atomic, lock-free reload mechanism is needed so in-flight connections keep their snapshot and new connections see the updated policy.

### Decision

Replace `Arc<Policy>` with `Arc<ArcSwap<Policy>>`; each connection reads a snapshot via `.load()`; the allow handler swaps a freshly parsed policy in atomically via `.store()`. `arc-swap` is MIT OR Apache-2.0, compatible with ghbrk's MIT-only dependency policy.

### Options Considered

| Option | Verdict |
|--------|---------|
| `arc-swap` swappable handle | ✓ Chosen — lock-free reads; atomic swap; in-flight connections keep their snapshot; tiny, permissively-licensed crate |
| `RwLock<Arc<Policy>>` | ✗ Rejected — adds lock contention on the read path for every connection |
| Daemon restart on every policy change | ✗ Rejected — drops all in-flight connections; unacceptable for a live system |

### Consequences

Policy reloads are transparent to in-flight connections. New connections immediately evaluate against the updated policy after an `allow` write. The `arc-swap` crate is added to `Cargo.toml`; `cargo deny check` must pass (it does — MIT OR Apache-2.0).

---

## ADR-028: Policy file is `ghbrk:ghbrk` mode `0600`; doctor stat-checks it

**Date:** 2026-06-03
**Plan:** `add-allow-command-and-roles`
**Status:** Accepted

### Context

The socket privilege gate (`SO_PEERCRED` UID 0) only governs daemon-mediated writes; it does not stop a local user from editing `/etc/ghbrk/policy.yaml` directly on disk. A misconfigured install could leave the file world-writable, bypassing the privilege gate entirely.

### Decision

`/etc/ghbrk/policy.yaml` is owned `ghbrk:ghbrk` with mode `0600`. `install.sh` creates it with that owner/mode (never overwriting existing content) and re-asserts owner/mode on re-run. `ghbrk doctor` adds a policy-permission check that `stat()`s the file and reports `Policy permissions: OK`/`WARNING`/`ERROR`, failing non-zero on a write-path exposure (wrong owner or group/other write bit).

### Options Considered

| Option | Verdict |
|--------|---------|
| `0600` owned by `ghbrk`; doctor stat-checks owner and mode | ✓ Chosen — tightest mode that still supports emergency manual edits via `sudo`; stat-check turns a silent misconfiguration into an explicit failure |
| `0640` owner `ghbrk:ghbrk-clients` (group-readable) | ✗ Rejected — clients have no need to read the raw file; widens the attack surface for no benefit |
| `0644` / world-readable | ✗ Rejected — the policy may encode org/repo structure that need not be world-readable; one `chmod` from world-writable |
| Rely solely on the socket gate | ✗ Rejected — the gate only governs daemon-mediated writes; direct on-disk edits are uncontrolled |

### Consequences

`ghbrk` daemon can read/write the policy file; `root` can edit it via `sudo`; all other users are denied both read and write. A re-run of `install.sh` corrects a drifted owner/mode. `doctor` surfaces a misconfigured file before it can be abused.

---

## ADR-029: Doctor permission audit is tiered: write-path = ERROR, read-path = WARNING

**Date:** 2026-06-03
**Plan:** `add-allow-command-and-roles`
**Status:** Accepted

### Context

After adding the policy-file permission check (ADR-028), the same direct-on-disk exposure exists for every other security-relevant path `ghbrk` owns: the config directory, the socket, the per-user credential directory, and the credential files. A tiered severity model was needed so `doctor` remains usable as a routine health check while still loudly failing on exposures that actively bypass the privilege model.

### Decision

`ghbrk doctor` audits `/etc/ghbrk/`, `/etc/ghbrk/policy.yaml`, `/run/ghbrk/ghbrk.sock`, `/var/lib/ghbrk/credentials/<user>/`, and each credential file. A write-path exposure (group/other write bit on a file; group/other write or execute bit on a directory; unexpected owner; socket connectable by non-group users) is `ERROR` and forces a non-zero exit. A read-path exposure (group/other read bit without write/execute) is `WARNING` and does not change exit status. `doctor` always runs every check, prints one line per check, and exits zero iff no `ERROR` was emitted.

### Options Considered

| Option | Verdict |
|--------|---------|
| Write-path = ERROR, read-path = WARNING | ✓ Chosen — maps directly onto the threat model; write access enables subversion, read access leaks information; keeps `doctor` usable as a routine health check |
| Every deviation = hard ERROR | ✗ Rejected — world-readable credential is a real concern but not an escalation; failing CI on it would train operators to ignore `doctor` |
| Every deviation = WARNING only | ✗ Rejected — world-writable policy file or 0666 socket is an active bypass of the privilege model; must fail loudly |
| Audit only the policy file (status quo after ADR-028) | ✗ Rejected — same exposure exists for socket, credential directory, and credential files |

### Consequences

`doctor` is usable as a routine, exit-code-driven health check. Write-path misconfigurations fail loudly. Read-path misconfigurations are surfaced without blocking automation. Every checked path is explicit and actionable in the output.

---

## ADR-023: Resolve home directory in the broker, carry on `ChildSpec`

**Date:** 2026-05-30
**Plan:** `change-executor-privilege-drop`
**Status:** Accepted

### Context

The executor needed to override the child's `HOME` environment variable to the peer user's home directory after privilege drop. Two options existed: resolve the home directory in the broker before fork, or re-query the passwd database inside the executor's `pre_exec` closure after fork.

### Decision

The broker resolves the peer's passwd home directory and passes it on `ChildSpec`; the executor overrides the child's `HOME` from that field rather than re-querying passwd.

### Options Considered

| Option | Verdict |
|--------|---------|
| Resolve home directory in the broker; carry on `ChildSpec` | Chosen — all passwd/group resolution happens pre-fork in the broker; the `pre_exec` closure performs only async-signal-safe syscalls |
| Re-query passwd inside the executor's `pre_exec` after fork | Rejected — passwd lookups in `pre_exec` after fork are not async-signal-safe; performing them between `fork(2)` and `execve(2)` is undefined behaviour |

### Consequences

The `pre_exec` closure is restricted to async-signal-safe syscalls (`setgroups`/`setgid`/`setuid`). All identity resolution (uid, gid, supplementary GIDs, home directory) is centralised in the broker's `peer_identity()` function and carried on `ChildSpec`. The executor has no dependency on passwd/group lookups.

---

## ADR-030: Whitelist `/etc/ghbrk` in `ReadWritePaths=` rather than migrate the policy path

**Date:** 2026-07-03
**Plan:** `fix-policy-dir-readwrite`
**Status:** Accepted

### Context

`ProtectSystem=strict` makes `/etc` read-only inside the daemon's private mount namespace. The default policy path is `Environment=GHBRK_POLICY=/etc/ghbrk/policy.yaml`, and the broker is the sole writer of that file via an atomic temp-file-plus-rename. The unit's `ReadWritePaths=/run/ghbrk /var/log/ghbrk` never included `/etc/ghbrk`, so every `sudo ghbrk allow` on a stock Linux install failed with `Read-only file system (os error 30)`.

### Decision

Add `/etc/ghbrk` to the unit's existing `ReadWritePaths=/run/ghbrk /var/log/ghbrk`, keeping the default `GHBRK_POLICY=/etc/ghbrk/policy.yaml` unchanged.

### Options Considered

| Option | Verdict |
|--------|---------|
| Whitelist `/etc/ghbrk` in `ReadWritePaths=` | ✓ Chosen — minimal, additive change; keeps the conventional path; matches the established narrow-whitelist precedent; the widened directory is owner-restricted (`ghbrk:ghbrk`, `policy.yaml` mode `0600`) and the daemon is its sole privilege-gated writer, so `ProtectSystem=strict` hardening elsewhere is preserved |
| Migrate the default policy path to `/var/etc/ghbrk/policy.yaml` (the immutable-`/etc` host workaround from commit `f7769d3`) | ✗ Rejected — larger diff spanning install.sh, README, and multiple existing specs for no user-visible benefit; abandons the conventional `/etc` config location |

### Consequences

`sudo ghbrk allow <org>/<repo> <op>` succeeds on a stock Linux install using the conventional `/etc/ghbrk/policy.yaml` path. The deployment feature's regression test derives the policy directory from the `GHBRK_POLICY` value declared in the unit and asserts that directory is present in `ReadWritePaths=`, so the two settings cannot silently drift apart. The now-removed `/var/etc/ghbrk` workaround was specific to hosts with an immutable `/etc` and is unaffected by this change.

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
**Status:** Accepted

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
**Status:** Accepted

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
**Status:** Accepted

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

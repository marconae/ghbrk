# Decisions: change-executor-privilege-drop

## ADR: Drop privilege in the child, not the daemon

**ID:** drop-privilege-in-child-not-daemon
**Plan:** change-executor-privilege-drop
**Status:** Accepted

### Context

The daemon runs as the `ghbrk` system user. Child `git`/`gh` processes inherited that identity, preventing traversal of a `0700` home directory or writes to user-owned repositories. The workaround, `chmod o+x ~`, was fragile, easy to forget, and weakened the home directory boundary for all `other` users.

### Decision

The daemon stays running as `ghbrk`; only the forked child drops to the peer user's UID/GID/supplementary groups inside the process's `pre_exec` hook.

### Options Considered

| Option | Verdict |
|--------|---------|
| Drop privilege in the child, not the daemon | Chosen — per-child drop gives the child exactly the user's permissions with no standing access for the daemon; preserves the single-process multi-user architecture; removes the manual `chmod` setup step |
| Run a separate daemon process per user | Rejected — heavy; breaks the single-daemon model |
| Keep `chmod o+x ~` workaround | Rejected — fragile, easy to forget, weakens the home directory boundary for all `other` users |
| Use filesystem ACLs on home directories | Rejected — unmaintainable; still grants `ghbrk` standing access |

### Consequences

Each child process runs with exactly the requesting user's UID, GID, and supplementary groups. The daemon's identity is unchanged. Home directories with default `0700` mode are traversable by the child without any permission change. The `chmod o+x ~` setup step is removed from the README and install documentation.

**2026-09-22 audit note (not part of the original decision):** the original text described the mechanism as "`CommandExt::uid()`/`gid()` plus a `setgroups()` call in `pre_exec`." The executor's actual implementation does not call `CommandExt::uid()`/`gid()`; it performs `setgroups`/`setresgid`/`setresuid` directly inside one `pre_exec` closure instead. The decision (drop in the child, not the daemon) is unaffected; only the named API surface is stale.

## ADR: Switch `ProtectHome=read-only` to `ProtectHome=no`

**ID:** switch-protecthome-readonly-to-no
**Plan:** change-executor-privilege-drop
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

## ADR: Keep `NoNewPrivileges=true` while adding `CAP_SETUID`/`CAP_SETGID`

**ID:** keep-nonewprivileges-while-adding-setuid-setgid-caps
**Plan:** change-executor-privilege-drop
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

## ADR: Resolve home directory in the broker, carry on `ChildSpec`

**ID:** resolve-home-directory-in-broker-childspec
**Plan:** change-executor-privilege-drop
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

The `pre_exec` closure is restricted to async-signal-safe syscalls. All identity resolution (uid, gid, supplementary GIDs, home directory) is centralised in the broker's `peer_identity()` function and carried on `ChildSpec`. The executor has no dependency on passwd/group lookups.

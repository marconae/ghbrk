# Feature: executor-privilege-drop

When `ChildSpec` carries a user identity (`uid`, `gid`, `supplementary_gids`, and the peer user's home directory), the executor drops the spawned child to that identity before `execve`. `CommandExt::uid()`/`gid()` set the credentials and a `pre_exec` closure calls `setgroups(2)` for the supplementary groups. Drop is skipped when the peer is root or equals the daemon's own user. All identity resolution happens in the broker before fork, so the `pre_exec` closure performs only async-signal-safe syscalls. Privilege drop is fail-closed — any syscall failure aborts the child before `execve`.

## Background

See the `executor-streaming` feature for streaming, cwd, and exit-code behaviour. Privilege drop applied at spawn time is specified in this feature.

## Scenarios

### Scenario: Executor drops to the peer user UID and GID before spawning the child

* *GIVEN* a `ChildSpec` whose `uid`, `gid`, and `supplementary_gids` are set to a non-root user that differs from the daemon's own user
* *WHEN* the executor spawns the child
* *THEN* the spawned process MUST run with real and effective UID equal to the spec's `uid`
* *AND* the spawned process MUST run with real and effective GID equal to the spec's `gid`
* *AND* the executor MUST apply the spec's `supplementary_gids` to the child via `setgroups(2)` in the child's `pre_exec` before `execve`
* *AND* the daemon's own process identity MUST remain unchanged after the child is spawned

### Scenario: Executor overrides HOME to the peer user home directory when dropping privilege

* *GIVEN* a `ChildSpec` whose `uid` resolves to a passwd entry with home directory `/home/alice`
* *AND* the daemon's own environment sets `HOME=/run/ghbrk`
* *WHEN* the executor spawns the child with privilege drop applied
* *THEN* the child's `HOME` environment variable MUST be `/home/alice`
* *AND* the child's `HOME` MUST NOT be `/run/ghbrk`

### Scenario: Executor skips privilege drop for the root peer

* *GIVEN* a `ChildSpec` whose `uid` is `0` (the root user)
* *WHEN* the executor spawns the child
* *THEN* the executor MUST NOT call `setuid(2)`, `setgid(2)`, or `setgroups(2)`
* *AND* the child MUST be spawned with the daemon's own identity

### Scenario: Executor skips privilege drop when the peer is the daemon user

* *GIVEN* a `ChildSpec` whose `uid` equals the daemon process's own effective UID
* *WHEN* the executor spawns the child
* *THEN* the executor MUST NOT attempt a `setuid(2)` call
* *AND* the child MUST be spawned successfully without error

### Scenario: Child aborts when the privilege drop syscall fails

* *GIVEN* a `ChildSpec` requesting a privilege drop the daemon lacks the capability to perform
* *WHEN* `setgid(2)` or `setuid(2)` fails inside the child's `pre_exec`
* *THEN* the child MUST abort before `execve` so it never runs the target program with the wrong identity
* *AND* the daemon MUST emit a `Denied { reason: ... }` frame describing the spawn failure
* *AND* the daemon MUST NOT crash

### Scenario: Child spawns with primary GID only when supplementary group lookup fails

* *GIVEN* a `ChildSpec` whose `uid` and `gid` are set but whose `supplementary_gids` is empty because the broker's supplementary group lookup failed
* *WHEN* the executor spawns the child
* *THEN* the executor MUST still drop to the spec's `uid` and `gid`
* *AND* the executor MUST set an empty supplementary group list via `setgroups(2)` rather than inheriting the daemon's supplementary groups
* *AND* the spawn MUST succeed

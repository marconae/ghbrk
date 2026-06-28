# Decisions: fix-gh-body-file-forwarding

## ADR: Stdin travels as positionally-discriminated client frames announced by a declared capability

**ID:** stdin-positional-client-frames-declared-capability
**Plan:** fix-gh-body-file-forwarding
**Status:** Accepted

### Context

The gateway client wrote exactly one `Request` frame and then only read; the executor spawned every child with `Stdio::null()`. No frame variant for standard input existed. `gh pr create --body-file -` therefore created a PR with an empty body and reported success; `gh pr comment --body-file -` failed at the GraphQL layer with `Body cannot be blank`. A released client never closes its write half after the `Request` — `src/cmd/gateway.rs:100` splits the stream and holds `write_half` open for the whole response loop — so a new daemon waiting on a first client frame from such a client would wait forever, with the child's stdin pipe still open. Rolling deployments where the daemon and its clients update at different times are the normal case for a system-wide broker.

### Decision

The first frame on a connection stays a bare `Request` object. Every frame after it is a `kind`-tagged `ClientFrame` with the variants `StdinChunk { data }` and `StdinEof`. The `Request` carries a `client_frames` boolean under `#[serde(default)]` that declares whether any client frame follows. When it decodes as `false`, the broker passes the executor an already-exhausted input source and closes the child's stdin at spawn, without waiting for a frame. When it is `true`, the client sends at least one frame, terminates with `StdinEof`, and shuts down its write half.

### Options Considered

| Option | Verdict |
|--------|---------|
| Positional `ClientFrame` after a bare `Request`, with a declared `client_frames` capability | ✓ Chosen — leaves the `Request` frame's JSON shape unchanged, so a client built before stdin forwarding still speaks the current protocol, and the daemon's wait behaviour is deterministic rather than inferred |
| Wrap `Request` inside the tagged `ClientFrame` enum | ✗ Rejected — changes the first frame's JSON from `{"tool":…}` to `{"kind":"request",…}`, breaking every old client against a new daemon and every new client against an old daemon |
| Infer the absence of client frames from end-of-file on the connection, or from a bounded wait for the first frame | ✗ Rejected — the shipped client never produces that end-of-file, so inference has nothing to observe; a timeout would replace a deterministic rule with a wall-clock race |

### Consequences

An old client works against a new daemon exactly as it does today: its `Request` carries no `client_frames` field, so the daemon closes the child's standard input at spawn and never waits for a frame that never arrives. A new client works against an old daemon because it reads response frames while it writes stdin frames, and an old daemon ignores request fields it does not know rather than rejecting the frame.

## ADR: The gateway keys stdin forwarding on the caller's terminal, never on argv

**ID:** stdin-forwarding-keyed-on-terminal-not-argv
**Plan:** fix-gh-body-file-forwarding
**Status:** Accepted

### Context

The reported defect covers any `gh` subcommand argument that takes a local file path (`--body-file`, `--notes-file`, `-F @file`) or stdin (`-`), not just `gh pr create` and `gh pr comment`. A per-subcommand table of which flags take a file would need constant upkeep as `gh` adds flags, and would have to be duplicated between the gateway and the resolver.

### Decision

`ghbrk git` and `ghbrk gh` stream the caller's standard input whenever it is not a terminal, and send a single `StdinEof` when it is. The gateway never inspects the forwarded argv to decide.

### Options Considered

| Option | Verdict |
|--------|---------|
| Key forwarding on `std::io::IsTerminal`, independent of argv | ✓ Chosen — one rule covers every subcommand of both tools, present and future, with no knowledge of what any flag means |
| Parse argv for `-` and for file-valued flags such as `--body-file` and `--notes-file` | ✗ Rejected — needs a per-subcommand flag table that goes stale the day `gh` adds one, duplicated in the resolver |
| An explicit `--stdin` opt-in flag | ✗ Rejected — leaves `cat body.md \| ghbrk gh pr create --body-file -` broken, which is the reported bug |

### Consequences

An interactive `ghbrk gh pr list` never captures the user's keystrokes. A brokered child that reads standard input now waits for the caller's standard input to close rather than seeing an immediate end-of-file; `ghbrk gh … < /dev/null` restores the old behaviour, the same opt-out `ssh -n` provides for `ssh`.

## ADR: The `--body-file <path>` failure is an installer, deployment, and spec defect, not a code defect

**ID:** private-tmp-removed-from-both-installers
**Plan:** fix-gh-body-file-forwarding
**Status:** Accepted

### Context

`--body-file /tmp/...` failed with `no such file or directory` for a file that demonstrably existed. On the reporting host, `systemctl show ghbrk -p PrivateTmp` returned `yes`. Two installers exist: `deploy/linux/install.sh` copies `deploy/linux/ghbrk.service`, which already omitted `PrivateTmp`, but the repository-root `install.sh` — the installer `README.md` and `docs/install.md` tell users to curl into `sudo bash` — writes its own unit from a heredoc that still emitted `PrivateTmp=true`, and `tests/deployment.rs` analysed only the `deploy/linux` copy. `specs/infra/systemd-unit` still required `PrivateTmp=true`, so the requirement that prevents this failure had no spec backing on either installer.

### Decision

Remove `PrivateTmp=true` from the repository-root `install.sh` heredoc, add `/etc/ghbrk` to that heredoc's `ReadWritePaths=`, and copy the shared-`/tmp` rationale comment from `deploy/linux/ghbrk.service` into it. Pin both installers with `tests/deployment.rs` assertions that neither emits `PrivateTmp=` and that their `[Service]` directive lines match. Fix `specs/infra/systemd-unit` to require the directive's absence, and add a `ghbrk doctor` shared-filesystem check that detects a daemon which cannot see the caller's `/tmp` and names the directive as the remediation.

### Options Considered

| Option | Verdict |
|--------|---------|
| Fix the root installer, pin both installers, fix the spec, and add a doctor check | ✓ Chosen — closes the defect on the documented install path and makes the requirement both specified and detectable |
| Fix `deploy/linux/ghbrk.service` and the spec only | ✗ Rejected — the root installer is the documented install path, so the reported bug survives every documented install, and the doctor remediation would tell operators to reinstall, which re-adds the directive |
| Read file-valued flag arguments client-side and forward their bytes | ✗ Rejected — needs the same stale-prone flag table rejected for stdin forwarding, and breaks `gh release upload`, whose asset can exceed the 16 MiB frame ceiling |
| `setns(2)` into the caller's mount namespace | ✗ Rejected — requires `CAP_SYS_ADMIN` against a unit that deliberately holds only `CAP_SETUID` and `CAP_SETGID` |

### Consequences

A fresh install from either documented path no longer hides caller-named `/tmp` paths from the spawned child. A host whose installed unit predates this requirement keeps its private `/tmp` until the operator removes the directive from `/etc/systemd/system/ghbrk.service` and restarts the service; `ghbrk doctor` names that remediation explicitly and never suggests reinstalling.

## ADR: A request without the client-frames declaration never waits for a client frame

**ID:** client-frames-absent-never-waits
**Plan:** fix-gh-body-file-forwarding
**Status:** Accepted

### Context

The initial design assumed a released client closes its write half after sending its `Request`, so the broker could treat that end-of-file as "no stdin to forward." Plan review established this was false: `src/cmd/gateway.rs:100` splits the stream and holds `write_half` alive for the whole response loop, calling neither `shutdown` nor drop. A new daemon waiting for a first `ClientFrame` from such a client would wait forever with the child's stdin pipe still open, turning today's silently-empty body into an unbounded hang for every version-skewed pair.

### Decision

The broker decides whether to wait for a client frame solely from the `Request`'s `client_frames` field, never from socket state. When the field decodes as `false` — including every request from a client released before this change — the broker never waits for a `ClientFrame`; it hands the executor an already-exhausted input source and closes the child's stdin at spawn.

### Options Considered

| Option | Verdict |
|--------|---------|
| Decide from the declared `client_frames` field alone | ✓ Chosen — deterministic regardless of what the client's socket does afterward |
| Treat a held-open write half as ordinary silence and wait for a bounded timeout | ✗ Rejected — trades a hang for a race and puts wall-clock time into the protocol |

### Consequences

A version-skewed pair never hangs: an old client against a new daemon gets the exact `Stdio::null()` behaviour it had before this change, because its `Request` carries no `client_frames` field.

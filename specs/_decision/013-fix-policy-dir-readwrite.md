# Decisions: fix-policy-dir-readwrite

## ADR: Whitelist `/etc/ghbrk` in `ReadWritePaths=` rather than migrate the policy path

**ID:** whitelist-etc-ghbrk-in-readwritepaths
**Plan:** fix-policy-dir-readwrite
**Status:** Accepted

### Context

`ProtectSystem=strict` makes `/etc` read-only inside the daemon's private mount namespace. The default policy path is `Environment=GHBRK_POLICY=/etc/ghbrk/policy.yaml`, and the broker is the sole writer of that file via an atomic temp-file-plus-rename. The unit's `ReadWritePaths=/run/ghbrk /var/log/ghbrk` never included `/etc/ghbrk`, so every `sudo ghbrk allow` on a stock Linux install failed with `Read-only file system (os error 30)`.

### Decision

Add `/etc/ghbrk` to the unit's existing `ReadWritePaths=/run/ghbrk /var/log/ghbrk`, keeping the default `GHBRK_POLICY=/etc/ghbrk/policy.yaml` unchanged.

### Options Considered

| Option | Verdict |
|--------|---------|
| Whitelist `/etc/ghbrk` in `ReadWritePaths=` | Chosen — minimal, additive change; keeps the conventional path; matches the established narrow-whitelist precedent; the widened directory is owner-restricted (`ghbrk:ghbrk`, `policy.yaml` mode `0600`) and the daemon is its sole privilege-gated writer, so `ProtectSystem=strict` hardening elsewhere is preserved |
| Migrate the default policy path to an alternate location outside `/etc` | Rejected — larger diff spanning install.sh, README, and multiple existing specs for no user-visible benefit; abandons the conventional `/etc` config location |

### Consequences

`sudo ghbrk allow <org>/<repo> <op>` succeeds on a stock Linux install using the conventional `/etc/ghbrk/policy.yaml` path. The deployment feature's regression test derives the policy directory from the `GHBRK_POLICY` value declared in the unit and asserts that directory is present in `ReadWritePaths=`, so the two settings cannot silently drift apart.

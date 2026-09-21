# Decisions: fix-runtimedir-namespace

## ADR: Use `tmpfiles.d` snippet and `ReadWritePaths=` for the socket parent directory

**ID:** tmpfiles-snippet-and-readwritepaths-for-socket-dir
**Plan:** fix-runtimedir-namespace (hotfix)
**Status:** Accepted

### Context

`runtimedirectory-ghbrk-mode-2750-for-socket-dir` chose `RuntimeDirectory=ghbrk` as the canonical systemd mechanism for recreating `/run/ghbrk/` on every service start. Post-deploy inspection of `/proc/<pid>/mountinfo` revealed that `RuntimeDirectory=` combined with `ProtectSystem=strict` creates the directory inside the service's private mount namespace. The socket bound there is invisible to processes running in the host namespace, including the shim. Every shim connection hit `ENOENT` on the socket path, triggering the EACCES silent-fallthrough path and causing the broker to be bypassed entirely.

### Decision

Replace `RuntimeDirectory=ghbrk` and `RuntimeDirectoryMode=2750` with a `tmpfiles.d(5)` snippet (`deploy/linux/ghbrk.tmpfiles`, installed to `/etc/tmpfiles.d/ghbrk.conf`) that creates `/run/ghbrk` on the host's `/run` tmpfs at every boot. Add `ReadWritePaths=/run/ghbrk` to the unit so the daemon can write the socket there under `ProtectSystem=strict`. `install.sh` installs the snippet and calls `systemd-tmpfiles --create` to create the directory immediately without requiring a reboot.

### Options Considered

| Option | Verdict |
|--------|---------|
| `RuntimeDirectory=ghbrk` in the unit (`runtimedirectory-ghbrk-mode-2750-for-socket-dir`) | Superseded — with `ProtectSystem=strict` the directory is created in the service's private mount namespace; the socket is not visible to host-namespace processes |
| `tmpfiles.d` snippet + `ReadWritePaths=/run/ghbrk` | Chosen — directory is created on the host's `/run` tmpfs and is visible to all processes; lifecycle managed by `systemd-tmpfiles` at every boot |
| Socket activation (`.socket` unit) | Rejected — would require protocol changes and adds complexity with no additional security benefit |

### Consequences

`/run/ghbrk/` is created on the host's `/run` tmpfs at every boot with owner `ghbrk:ghbrk-clients` and mode `2750`. The socket is visible to shim processes in the host namespace. `install.sh` now installs a second artefact (`ghbrk.tmpfiles`) alongside the unit file. The `ReadWritePaths=` entry makes the socket directory lifecycle explicit in the unit file itself.

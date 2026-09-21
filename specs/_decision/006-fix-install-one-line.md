# Decisions: fix-install-one-line

## ADR: Set daemon's primary group to `ghbrk-clients` instead of relying on runtime `chown`

**ID:** daemon-primary-group-ghbrk-clients
**Plan:** fix-install-one-line
**Status:** Accepted

### Context

`ghbrk.service` ran the daemon with `Group=ghbrk`. When the broker bound the socket, the socket file inherited the daemon's primary group (`ghbrk`). `apply_socket_group` then attempted `chown(socket, None, ghbrk-clients-gid)`, but a non-root process can only `chown` files to groups it is a member of, and the `ghbrk` user was not in `ghbrk-clients`. The `chown` failed with `EPERM`, logged only at `warn!` level, and the socket ended up `ghbrk:ghbrk` mode `0660`. Any agent user (in `ghbrk-clients` but not `ghbrk`) received `EACCES` on connect, silently triggering the passthrough fallback and bypassing the broker entirely.

### Decision

Change `Group=ghbrk` to `Group=ghbrk-clients` in `deploy/linux/ghbrk.service`. The daemon's primary GID becomes `ghbrk-clients`, so the socket file is created with that group on the first `bind(2)` call, with no `chown` step required for correctness. `apply_socket_group` stays as a defence-in-depth check but logs at `error!` level with the systemd directive named in the message. `install.sh` also runs `usermod -aG ghbrk-clients ghbrk` so the defence-in-depth path can succeed if the unit is ever misconfigured.

### Options Considered

| Option | Verdict |
|--------|---------|
| `Group=ghbrk-clients` as daemon's primary group | Chosen — socket is correctly grouped from the first `bind(2)` byte; no TOCTOU window before chown runs |
| Keep `Group=ghbrk` and add `ghbrk` to `ghbrk-clients` via `usermod` so the runtime `chown` succeeds | Rejected — leaves the socket owned by the daemon's primary group (`ghbrk`) on the first `bind()` and relies on the `chown` completing before any client connects |

### Consequences

The socket is correctly grouped from the first byte on every service start, including the instant between `bind(2)` and any subsequent `chown`. The defence-in-depth `chown` path now exists only to catch misconfigurations; when it fires it logs at `error!` and names the unit directive as the fix so operators can locate and correct the misconfiguration without consulting source code.

## ADR: Use `RuntimeDirectory=ghbrk` mode `2750` for the socket parent directory

**ID:** runtimedirectory-ghbrk-mode-2750-for-socket-dir
**Plan:** fix-install-one-line
**Status:** Superseded by tmpfiles-snippet-and-readwritepaths-for-socket-dir

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

## ADR: `install.sh` enables and restarts the service and wires users into `ghbrk-clients`

**ID:** install-sh-enables-restarts-wires-users-into-group
**Plan:** fix-install-one-line
**Status:** Accepted

### Context

`install.sh` ended with instructions to manually run `systemctl enable` and `systemctl start ghbrk`. The `ghbrk` daemon user and `$SUDO_USER` were not automatically joined to `ghbrk-clients`, requiring further manual operator steps. The stated contract for the installer was one privileged command (`sudo ./deploy/linux/install.sh`), then copy credentials and edit policy. Printing follow-up instructions violated that contract.

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

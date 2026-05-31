# Feature: systemd-unit

Defines the systemd service unit for the ghbrk daemon: how the unit starts the process, which user it runs as, the runtime directory it owns for the Unix socket, and the hardening directives that constrain the daemon's capabilities.

## Background

The unit file lives at `deploy/linux/ghbrk.service`. To let the daemon drop spawned children to the requesting user, the unit grants exactly `CAP_SETUID` and `CAP_SETGID` (via `AmbientCapabilities` and `CapabilityBoundingSet`) and sets `ProtectHome=no` so user-owned children can write repositories under user home directories. `NoNewPrivileges=true` is retained: it does not block `setuid(2)`/`setgid(2)` when the capability is already held, and it still prevents SUID-binary escalation. All other hardening directives are unchanged.

## Scenarios

### Scenario: systemd unit starts the daemon as the ghbrk user

* *GIVEN* the systemd unit `deploy/linux/ghbrk.service` is installed
* *WHEN* an operator runs `systemctl start ghbrk`
* *THEN* the unit MUST start `/usr/local/bin/ghbrk daemon`
* *AND* the unit MUST run as `User=ghbrk`
* *AND* the unit MUST have `Group=ghbrk-clients`

### Scenario: systemd unit has hardening directives

* *GIVEN* the systemd unit file
* *WHEN* an operator inspects it
* *THEN* the unit MUST include at minimum `ProtectSystem=strict`, `NoNewPrivileges=true`, and `PrivateTmp=true`
* *AND* the unit MUST scope its capability set to exactly `CAP_SETUID` and `CAP_SETGID` via `CapabilityBoundingSet`

### Scenario: socket parent directory is on the host filesystem

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *AND* the tmpfiles snippet `deploy/linux/ghbrk.tmpfiles`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST NOT contain `RuntimeDirectory=`
* *AND* the unit MUST include `ReadWritePaths=` with `/run/ghbrk` so the daemon can write the socket under `ProtectSystem=strict`
* *AND* `deploy/linux/ghbrk.tmpfiles` MUST declare `d /run/ghbrk 2750 ghbrk ghbrk-clients` so systemd recreates the directory on every boot

### Scenario: systemd unit grants the privilege-drop capabilities

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST include `AmbientCapabilities=CAP_SETUID CAP_SETGID` so the daemon process retains the capability to change to the requesting user
* *AND* the unit MUST include `CapabilityBoundingSet=CAP_SETUID CAP_SETGID` so no other capability can be acquired

### Scenario: systemd unit keeps NoNewPrivileges alongside the privilege-drop capabilities

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *AND* the unit grants `CAP_SETUID` and `CAP_SETGID` via `AmbientCapabilities`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST retain `NoNewPrivileges=true`
* *AND* `NoNewPrivileges=true` MUST NOT block the daemon's `setuid(2)`/`setgid(2)` syscalls, because the capability is already held and no SUID transition is involved
* *AND* `NoNewPrivileges=true` MUST continue to prevent the daemon from gaining privilege by executing SUID binaries

### Scenario: systemd unit allows the child to write under user home directories

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST set `ProtectHome=no`
* *AND* the unit MUST NOT set `ProtectHome=read-only`
* *AND* the rationale MUST be that child processes spawned as the requesting user need write access to repositories under that user's home directory for `git fetch`/`git pull`

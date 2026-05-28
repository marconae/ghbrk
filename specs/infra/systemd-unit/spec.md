# Feature: systemd-unit

Defines the systemd service unit for the ghbrk daemon: how the unit starts the process, which user it runs as, the runtime directory it owns for the Unix socket, and the hardening directives that constrain the daemon's capabilities.

## Background

The unit file lives at `deploy/linux/ghbrk.service` and is installed by `deploy/linux/install.sh`. It runs the daemon as the `ghbrk` user in the `ghbrk-clients` group.

The socket parent directory `/run/ghbrk/` is created on the host's `/run` tmpfs at every boot by `systemd-tmpfiles` using the snippet at `deploy/linux/ghbrk.tmpfiles` (installed to `/etc/tmpfiles.d/ghbrk.conf`). The unit exposes the directory via `ReadWritePaths=/run/ghbrk` so the daemon can bind its socket there under `ProtectSystem=strict`.

`RuntimeDirectory=` is intentionally absent: combined with `ProtectSystem=strict`, it creates the directory inside the service's private mount namespace, making the socket invisible to host-namespace processes (the shim).

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

### Scenario: socket parent directory is on the host filesystem

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *AND* the tmpfiles snippet `deploy/linux/ghbrk.tmpfiles`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST NOT contain `RuntimeDirectory=`
* *AND* the unit MUST include `ReadWritePaths=` with `/run/ghbrk` so the daemon can write the socket under `ProtectSystem=strict`
* *AND* `deploy/linux/ghbrk.tmpfiles` MUST declare `d /run/ghbrk 2750 ghbrk ghbrk-clients` so systemd recreates the directory on every boot

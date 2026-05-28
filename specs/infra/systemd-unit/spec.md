# Feature: systemd-unit

Defines the systemd service unit for the ghbrk daemon: how the unit starts the process, which user it runs as, the RuntimeDirectory it owns for the Unix socket, and the hardening directives that constrain the daemon's capabilities.

## Background

The unit file lives at `deploy/linux/ghbrk.service` and is installed by `deploy/linux/install.sh`. It runs the daemon as the `ghbrk` user in the `ghbrk-clients` group, declares `RuntimeDirectory=ghbrk` so `/run/ghbrk/` is recreated on every service start (including post-reboot when the `tmpfs` is wiped), and applies systemd hardening directives.

## Scenarios

### Scenario: systemd unit starts the daemon as the ghbrk user

* *GIVEN* the systemd unit `deploy/linux/ghbrk.service` is installed
* *WHEN* an operator runs `systemctl start ghbrk`
* *THEN* the unit MUST start `/usr/local/bin/ghbrk daemon`
* *AND* the unit MUST run as `User=ghbrk`
* *AND* the unit MUST have `Group=ghbrk-clients`
* *AND* the unit MUST declare `RuntimeDirectory=ghbrk`
* *AND* the unit MUST declare `RuntimeDirectoryMode=2750`

### Scenario: systemd unit has hardening directives

* *GIVEN* the systemd unit file
* *WHEN* an operator inspects it
* *THEN* the unit MUST include at minimum `ProtectSystem=strict`, `NoNewPrivileges=true`, and `PrivateTmp=true`

### Scenario: systemd unit declares a RuntimeDirectory for the socket

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST include `RuntimeDirectory=ghbrk`
* *AND* the unit MUST include `RuntimeDirectoryMode=2750`
* *AND* the unit's `ReadWritePaths=` directive MUST NOT include `/var/run/ghbrk` because `RuntimeDirectory=` already grants write access under `ProtectSystem=strict`

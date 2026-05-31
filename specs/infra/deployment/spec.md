# Feature: deployment

Provides the artefacts an operator needs to install and run ghbrk on a Linux host: an installation script, an annotated example policy, and a `cargo deny` configuration that enforces the MIT-only dependency policy. Systemd unit behaviour is specified separately in `infra/systemd-unit`.

## Background

With executor privilege drop in place, setup no longer requires loosening home directory permissions. The README and install script MUST NOT instruct operators to run `chmod o+x ~`, because brokered git operations run as the requesting user and traverse the home directory with that user's own permissions. Targets Linux only in v1.

## Scenarios

### Scenario: install.sh creates ghbrk system user

* *GIVEN* a Linux host without an existing `ghbrk` user
* *WHEN* the operator runs `deploy/linux/install.sh` as root
* *THEN* the script MUST create the system user `ghbrk`
* *AND* the script MUST create the group `ghbrk-clients`

### Scenario: install.sh creates required directories with correct modes

* *GIVEN* a fresh Linux host
* *WHEN* `install.sh` completes successfully
* *THEN* `/etc/ghbrk/` MUST exist with mode `0755` and owner `root:root`
* *AND* `/etc/ghbrk/credentials/` MUST exist with mode `0700` and owner `ghbrk:ghbrk`
* *AND* `/var/log/ghbrk/` MUST exist with mode `0750` and owner `ghbrk:ghbrk-clients`
* *AND* `/run/ghbrk/` MUST be created by `systemd-tmpfiles` at every boot via `/etc/tmpfiles.d/ghbrk.conf` (deployed by `install.sh` from `deploy/linux/ghbrk.tmpfiles`), with owner `ghbrk:ghbrk-clients` and mode `2750`; the unit exposes it via `ReadWritePaths=/run/ghbrk` so the socket is visible to host-namespace processes

### Scenario: install.sh is idempotent on second run

* *GIVEN* `install.sh` was already run successfully on this host
* *WHEN* the operator runs `install.sh` a second time
* *THEN* the script MUST exit zero
* *AND* the script MUST NOT report a fatal error about an existing user, group, or directories
* *AND* the script MUST NOT report a fatal error when re-running `useradd`, `groupadd`, or `systemctl enable`
* *AND* the script MUST NOT report a fatal error when `usermod -aG ghbrk-clients ghbrk` is invoked a second time
* *AND* the script MUST NOT report a fatal error when `usermod -aG ghbrk-clients "$SUDO_USER"` is invoked a second time
* *AND* the script MUST `restart` (rather than `start`) the service so a second run picks up any unit changes without failing on "already running"

### Scenario: Example policy YAML is loadable by the policy engine

* *GIVEN* the file `config/policy.example.yaml`
* *WHEN* the policy engine loads it
* *THEN* loading MUST succeed without errors

### Scenario: cargo deny rejects a GPL dependency

* *GIVEN* the `deny.toml` is configured per the project policy
* *WHEN* `cargo deny check` is run against a Cargo.toml that declares a GPL-3.0 dependency
* *THEN* the command MUST exit with a non-zero status
* *AND* the output MUST mention the disallowed license

### Scenario: cargo deny passes on the real dependency tree

* *GIVEN* the project's actual `Cargo.toml` and `Cargo.lock`
* *WHEN* `cargo deny check` is run
* *THEN* the command MUST exit zero

### Scenario: install.sh adds ghbrk user to ghbrk-clients group

* *GIVEN* a Linux host where the `ghbrk` user and the `ghbrk-clients` group have just been created by `install.sh`
* *WHEN* `install.sh` reaches the group-membership step
* *THEN* the script MUST run `usermod -aG ghbrk-clients ghbrk`
* *AND* the `usermod` invocation MUST use the `-a` (append) flag so the `ghbrk` user's existing supplementary groups are preserved
* *AND* the script MUST echo a confirmation line indicating the `ghbrk` user has been added to `ghbrk-clients`

### Scenario: install.sh enables and starts the daemon

* *GIVEN* a Linux host running systemd where `install.sh` has placed the binary and the unit file
* *WHEN* `install.sh` reaches the service-activation step
* *THEN* the script MUST run `systemctl daemon-reload`
* *AND* the script MUST run `systemctl enable ghbrk`
* *AND* the script MUST run `systemctl restart ghbrk` (rather than `systemctl start`) so re-runs after editing the unit pick up the new directives without erroring on "already running"
* *AND* the script MUST guard the `systemctl` calls behind `command -v systemctl &>/dev/null` so non-systemd hosts do not fail
* *AND* the closing banner MUST describe what the script did (service is enabled and running) rather than instructing the operator to run `systemctl enable`/`start` manually

### Scenario: install.sh adds the installing user to ghbrk-clients

* *GIVEN* `install.sh` is invoked via `sudo` so the environment variable `SUDO_USER` is set to a non-empty username
* *WHEN* the script reaches the group-membership step
* *THEN* the script MUST run `usermod -aG ghbrk-clients "$SUDO_USER"`
* *AND* the script MUST print a notice that the operator SHALL log out and back in (or run `newgrp ghbrk-clients`) for the supplementary group membership to take effect in their existing shell sessions
* *AND* when `$SUDO_USER` is unset or empty (the script was run as actual root rather than via `sudo`), the script MUST instead print a clear manual-add instruction telling the operator which command to run to add their user to the `ghbrk-clients` group
* *AND* the script MUST NOT abort if `usermod -aG ghbrk-clients "$SUDO_USER"` is run a second time against a user already in the group

### Scenario: Setup requires no home directory mode change

* *GIVEN* a developer following the README setup instructions on a Linux host
* *AND* the developer's home directory has the default `0700` mode
* *WHEN* the developer completes credential and policy setup and runs a brokered git operation
* *THEN* the documented setup MUST NOT instruct the operator to run `chmod o+x ~` or otherwise loosen home directory permissions
* *AND* the brokered operation MUST succeed because the daemon drops to the requesting user's UID/GID before spawning the child, so the child traverses the home directory with the user's own permissions
* *AND* the install script MUST NOT add any new step to grant the `ghbrk` system user access to user home directories

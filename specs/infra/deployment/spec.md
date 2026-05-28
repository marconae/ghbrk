# Feature: deployment

Provides the artefacts an operator needs to install and run ghbrk on a Linux host: a systemd unit file, an installation script, an annotated example policy, and a `cargo deny` configuration that enforces the MIT-only dependency policy.

## Background

Targets Linux only in v1. The install script creates the `ghbrk` system user and `ghbrk-clients` group, places the binary, creates `/etc/ghbrk/`, `/etc/ghbrk/credentials/`, `/var/run/ghbrk/`, and `/var/log/ghbrk/` with correct ownership and permissions, and installs the systemd unit. The `cargo deny` config rejects any GPL/AGPL/LGPL/SSPL licensed dependency.

## Scenarios

### Scenario: systemd unit starts the daemon as the ghbrk user

* *GIVEN* the systemd unit `deploy/linux/ghbrk.service` is installed
* *WHEN* an operator runs `systemctl start ghbrk`
* *THEN* the unit MUST start `/usr/local/bin/ghbrk daemon`
* *AND* the unit MUST run as `User=ghbrk`
* *AND* the unit MUST have `Group=ghbrk`

### Scenario: systemd unit has hardening directives

* *GIVEN* the systemd unit file
* *WHEN* an operator inspects it
* *THEN* the unit MUST include at minimum `ProtectSystem=strict`, `NoNewPrivileges=true`, and `PrivateTmp=true`

### Scenario: install.sh creates ghbrk system user

* *GIVEN* a Linux host without an existing `ghbrk` user
* *WHEN* the operator runs `deploy/linux/install.sh` as root
* *THEN* the script MUST create the system user `ghbrk`
* *AND* the script MUST create the group `ghbrk-clients`

### Scenario: install.sh creates required directories with correct modes

* *GIVEN* a fresh Linux host
* *WHEN* `install.sh` completes successfully
* *THEN* `/etc/ghbrk/` MUST exist with mode `0750` and owner `ghbrk:ghbrk`
* *AND* `/etc/ghbrk/credentials/` MUST exist with mode `0700` and owner `ghbrk:ghbrk`
* *AND* `/var/run/ghbrk/` MUST exist with mode `0755` and owner `ghbrk:ghbrk-clients`

### Scenario: install.sh is idempotent on second run

* *GIVEN* `install.sh` was already run successfully
* *WHEN* the operator runs `install.sh` a second time
* *THEN* the script MUST exit zero
* *AND* the script MUST NOT report a fatal error about existing user or directories

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

### Scenario: install.sh creates /usr/local/bin/git and /usr/local/bin/gh symlinks to ghbrk

* *GIVEN* a Linux host where `/usr/local/bin/ghbrk` has been installed
* *WHEN* the operator runs `deploy/linux/install.sh` as root
* *THEN* the script MUST create a symlink at `/usr/local/bin/git` pointing to `/usr/local/bin/ghbrk`
* *AND* the script MUST create a symlink at `/usr/local/bin/gh` pointing to `/usr/local/bin/ghbrk`
* *AND* both symlinks MUST be created such that `/usr/local/bin` (which precedes `/usr/bin` in the default system PATH) routes `git` and `gh` invocations through the shim

### Scenario: install.sh symlink creation is idempotent

* *GIVEN* `install.sh` was already run successfully and the `/usr/local/bin/git` and `/usr/local/bin/gh` symlinks exist
* *WHEN* the operator runs `install.sh` a second time
* *THEN* the script MUST exit zero
* *AND* the script MUST NOT report a fatal error about existing symlinks
* *AND* the symlinks MUST continue to point to `/usr/local/bin/ghbrk`

### Scenario: install.sh refuses to overwrite a non-symlink at /usr/local/bin/git

* *GIVEN* a Linux host where `/usr/local/bin/git` already exists as a regular file (not a symlink)
* *WHEN* the operator runs `install.sh`
* *THEN* the script MUST NOT silently delete or replace the existing regular file
* *AND* the script MUST print a clear warning indicating the conflict
* *AND* the script MUST continue with the remaining install steps

# Feature: deployment

Installs ghbrk on a Linux host: creates the system user and group, places the binary, creates `/etc/ghbrk` directories with correct ownership and permissions, installs and starts the systemd unit, and manages group membership. The install script no longer creates `git`/`gh` symlinks, since the transparent shim is removed.

## Background

The install script creates the `ghbrk` system user and `ghbrk-clients` group, places the binary, creates `/etc/ghbrk` directories with correct ownership and permissions, joins both the `ghbrk` user and the invoking user into `ghbrk-clients`, installs the systemd unit, and enables and restarts the service. With the explicit gateway, the script does NOT create `/usr/local/bin/git` or `/usr/local/bin/gh` symlinks and there is no `install-shims` step; agents call plain `git`/`gh` and invoke `ghbrk git`/`ghbrk gh` explicitly. The `deny.toml` config rejects any GPL/AGPL/LGPL/SSPL licensed dependency.

## Scenarios

<!-- DELTA:REMOVED -->
### Scenario: install.sh creates /usr/local/bin/git and /usr/local/bin/gh symlinks to ghbrk

* *GIVEN* a Linux host where `ghbrk` has been installed
* *WHEN* the operator runs `install.sh` as root
* *THEN* the script MUST create a symlink at `/usr/local/bin/git` pointing to the ghbrk binary
* *AND* the script MUST create a symlink at `/usr/local/bin/gh` pointing to the ghbrk binary
* *AND* both symlinks MUST be created such that `/usr/local/bin` (which precedes `/usr/bin` in the default system PATH) routes `git` and `gh` invocations through the shim
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: install.sh symlink creation is idempotent

* *GIVEN* `install.sh` was already run successfully and the `git` and `gh` symlinks exist
* *WHEN* the operator runs `install.sh` a second time
* *THEN* the script MUST exit zero
* *AND* the script MUST NOT report a fatal error about existing symlinks
* *AND* the symlinks MUST continue to point to the ghbrk binary
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: install.sh refuses to overwrite a non-symlink at /usr/local/bin/git

* *GIVEN* a Linux host where `/usr/local/bin/git` already exists as a regular file (not a symlink)
* *WHEN* the operator runs `install.sh`
* *THEN* the script MUST NOT silently delete or replace the existing regular file
* *AND* the script MUST print a clear warning indicating the conflict
* *AND* the script MUST continue with the remaining install steps
<!-- /DELTA:REMOVED -->

<!-- DELTA:CHANGED -->
### Scenario: install.sh is idempotent on second run

* *GIVEN* `install.sh` was already run successfully on this host
* *WHEN* the operator runs `install.sh` a second time
* *THEN* the script MUST exit zero
* *AND* the script MUST NOT report a fatal error about an existing user, group, or directories
* *AND* the script MUST NOT report a fatal error when re-running `useradd`, `groupadd`, or `systemctl enable`
* *AND* the script MUST `restart` (rather than `start`) the service so a second run picks up any unit changes without failing on "already running"
<!-- /DELTA:CHANGED -->

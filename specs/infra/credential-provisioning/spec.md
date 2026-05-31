# Feature: credential-provisioning

Provides the `provision-user.sh` script that operators run to create per-user credential directory structures under `/etc/ghbrk/credentials/` after `install.sh` has established the system-level prerequisites.

## Background

With executor privilege drop in place, credential files are owned by the `ghbrk` system user and are not readable by the calling user. The `provision-user.sh` script creates the directory and placeholder files with the correct ownership and modes, then prints instructions for the operator to fill in the actual key and token values.

## Scenarios

### Scenario: provision-user.sh creates credential files owned by ghbrk

* *GIVEN* a Linux host where `install.sh` has already created the `ghbrk` user and the `/etc/ghbrk/credentials/` root
* *WHEN* the operator runs `deploy/linux/provision-user.sh <username>` as root
* *THEN* `/etc/ghbrk/credentials/<username>/` MUST be created with mode `0750` and owner `ghbrk:ghbrk`
* *AND* `/etc/ghbrk/credentials/<username>/id_rsa` MUST be created with mode `0600` and owner `ghbrk:ghbrk`
* *AND* `/etc/ghbrk/credentials/<username>/token` MUST be created with mode `0600` and owner `ghbrk:ghbrk`
* *AND* the printed fill-in instructions MUST tell the operator to `chown ghbrk:ghbrk` the `id_rsa` file (NOT `<username>:<username>`)

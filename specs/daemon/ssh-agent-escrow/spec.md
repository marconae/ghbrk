# Feature: ssh-agent-escrow

Manages the per-operation SSH agent lifecycle so that SSH private key bytes never enter the calling user's address space. The daemon loads the key into a short-lived `ssh-agent`, exposes only the socket to the child process, and tears down the agent after the operation completes.

## Background

Credentials live under `/etc/ghbrk/credentials/<user>/` owned by the `ghbrk` user with mode `0600`. The SSH private key file is `id_rsa`. The daemon process must have read access; the calling user's processes must not — the key file is not readable nor reachable by the calling user. The daemon loads `id_rsa` into a per-operation `ssh-agent` running as `ghbrk` and exposes only `SSH_AUTH_SOCK` to the child, so the key bytes never enter the calling user's address space.

## Scenarios

### Scenario: SSH URL selects SSH key injection

* *GIVEN* the resolved URL is `git@github.com:acme/web.git`
* *AND* the file `/etc/ghbrk/credentials/alice/id_rsa` exists with mode `0600` owned by `ghbrk:ghbrk`
* *WHEN* the injector prepares the child environment for user `alice`
* *THEN* the daemon MUST start an `ssh-agent` and load `id_rsa` into it via `ssh-add` while running as the `ghbrk` user
* *AND* the environment MUST set `SSH_AUTH_SOCK` to the agent socket path and MUST NOT set `GIT_SSH_COMMAND` with an `-i <key path>` option
* *AND* the `id_rsa` bytes MUST NOT be passed to the child via env var, argv, or file path
* *AND* the key MUST be loaded into the agent with a lifetime of at most 30 seconds so that after the SSH handshake completes the agent holds no usable key material

### Scenario: SSH URL with missing key returns explicit error

* *GIVEN* the resolved URL is `git@github.com:acme/web.git`
* *AND* `/etc/ghbrk/credentials/alice/id_rsa` does not exist
* *WHEN* the injector prepares the child environment
* *THEN* the injector MUST return an error indicating the SSH key is missing for user `alice`

### Scenario: SSH key is not readable by the calling user

* *GIVEN* `/etc/ghbrk/credentials/alice/id_rsa` exists with mode `0600` owned by `ghbrk:ghbrk`
* *AND* the calling Unix user `alice` is not `ghbrk` and is not in the `ghbrk` group
* *WHEN* a process running as `alice` attempts to open `id_rsa` for reading
* *THEN* the open MUST fail with `EACCES`
* *AND* the daemon (running as `ghbrk`) MUST still be able to read `id_rsa` to load it into the agent

### Scenario: SSH agent socket is removed after operation

* *GIVEN* an SSH git operation for user `alice` started an `ssh-agent` with a socket under a private temp directory
* *WHEN* the git child exits and the injector's agent handle is dropped
* *THEN* the `ssh-agent` process MUST be terminated
* *AND* the agent socket path MUST no longer exist
* *AND* the private temp directory MUST no longer exist

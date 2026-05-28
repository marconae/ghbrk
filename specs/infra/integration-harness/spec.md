# Feature: integration-harness

Provides a Docker-based test fixture that runs a real bare-git SSH server so end-to-end shim → daemon → git operations can be exercised in CI without depending on GitHub.

## Background

The harness lives under `tests/integration/` and is launched by an integration test using `docker compose`. The container exposes an SSH-accessible bare git repo. A test SSH keypair is generated per run; the public key is injected into the container's `authorized_keys`. The daemon is started in a temporary directory with credentials wired to the generated private key. Tests work with `git@<container>:repo.git`-style URLs.

## Scenarios

### Scenario: Harness starts a reachable git SSH server

* *GIVEN* docker is available on the host
* *WHEN* the test harness brings up the compose project
* *THEN* the bare repo `repo.git` MUST be reachable via `ssh://git@<container>/srv/git/repo.git`
* *AND* the test MUST be able to clone the repo using the generated key

### Scenario: Push through shim to harness succeeds when policy allows

* *GIVEN* the daemon is running with a policy allowing push to the harness repo for the test user
* *AND* the test SSH key is registered in the daemon's credentials directory
* *WHEN* a test process invokes the shim with `git push origin main` against the harness URL
* *THEN* the push MUST succeed
* *AND* the bare repo's `refs/heads/main` MUST point at the pushed commit

### Scenario: Push through shim to harness is rejected when policy denies

* *GIVEN* the daemon's policy has no matching allow rule for push to the harness repo
* *WHEN* a test process invokes the shim with `git push origin main`
* *THEN* the shim MUST exit with a non-zero status
* *AND* the bare repo's `refs/heads/main` MUST NOT be updated
* *AND* the audit log MUST contain a deny record

### Scenario: Clone through shim to harness streams progress

* *GIVEN* the daemon is running and policy allows clone
* *WHEN* a test process invokes the shim with `git clone <harness-url> /tmp/clone`
* *THEN* the clone MUST succeed
* *AND* `/tmp/clone/.git` MUST exist after the command

### Scenario: Harness tears down cleanly between tests

* *GIVEN* one harness-based test has completed
* *WHEN* the test harness shuts down the compose project
* *THEN* the SSH container MUST be stopped and removed
* *AND* no orphan listener MUST remain on the previously-used host port

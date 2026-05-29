# Feature: integration-harness

Provides a Docker-based test fixture that runs a real bare-git SSH server and a developer-tools container so end-to-end shim → daemon → git/gh operations can be exercised in CI without depending on GitHub for the SSH path.

## Background

The harness lives under `tests/integration/` and is launched by an integration test using `docker compose`. The container exposes an SSH-accessible bare git repo. A test SSH keypair is generated per run; the public key is injected into the container's `authorized_keys`. The daemon is started in a temporary directory with credentials wired to the generated private key. A separate `devenv` service (built from `Dockerfile.devenv`, `debian:bookworm-slim`, running as `root` with `gh`, `git`, and `openssh-client` installed) provides a reachable container for exercising `gh api` through the broker.

To prove the `gh api` → broker → GitHub path end-to-end without a real GitHub token or network access, the compose project additionally runs a `mock-github` service: a minimal HTTPS server (built from `Dockerfile.mock-github`) that answers `GET /api/v3/user` with a fixed JSON body. Because the `gh` CLI enforces HTTPS even when `GH_HOST` points at a non-`github.com` host, the mock serves real TLS using a pre-generated, self-signed test CA and server certificate committed under `tests/integration/certs/` (CN/SAN `mock-github`). The `devenv` image installs that CA into its system trust store at build time so `gh` trusts the mock. The `gh api` harness tests configure the broker with `GH_HOST=mock-github` and a synthetic token, so they always run whenever Docker is available — they no longer skip gracefully.

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

### Scenario: Mock GitHub API service is reachable over TLS from devenv container

* *GIVEN* the compose project is up with the `mock-github` HTTPS service running
* *AND* the `devenv` container has the test CA installed in its system trust store
* *WHEN* a test process runs `curl https://mock-github/api/v3/user -H "Authorization: bearer test-fake-token-value"` from inside the `devenv` container
* *THEN* the request MUST succeed with HTTP status 200 without any TLS verification override
* *AND* the response body MUST contain `"login": "test-user"`

### Scenario: gh api through broker succeeds — mock GitHub API

* *GIVEN* the compose project is up with the `mock-github` HTTPS service running and the broker configured with `GH_HOST=mock-github`
* *AND* the daemon is running with a policy allowing `gh_api_read` for the test user
* *AND* a synthetic token is present in the daemon's credentials directory with mode `0600`
* *WHEN* a test process invokes the shim with `gh api user`
* *THEN* the command MUST exit with status zero
* *AND* the streamed stdout MUST contain `"login": "test-user"` from the mock API response

### Scenario: gh api through broker — invalid token returns 401

* *GIVEN* the compose project is up with the `mock-github` HTTPS service running and the broker configured with `GH_HOST=mock-github`
* *AND* the daemon is running with a policy allowing `gh_api_read` for the test user
* *AND* no token file exists in the daemon's credentials directory for the test user
* *WHEN* a test process invokes the shim with `gh api user`
* *THEN* the command MUST exit with a non-zero status
* *AND* the streamed stderr MUST indicate the token is missing

### Scenario: Integration dev container is reachable as root

* *GIVEN* the integration `devenv` service defined in the compose project
* *WHEN* the test harness brings up the compose project
* *THEN* the `devenv` container MUST be running as the `root` user
* *AND* the container MUST have `gh`, `git`, and `ssh` available on its PATH

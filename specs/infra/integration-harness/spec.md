# Feature: integration-harness

Provides a Docker-based test fixture that runs a real bare-git SSH server and a developer-tools container so end-to-end gateway → daemon → git/gh operations can be exercised in CI without depending on GitHub for the SSH path.

## Background

With executor privilege drop in place, the harness gains a Linux-only end-to-end test that proves a brokered push succeeds through an unprivileged user's `0700` home directory without any `chmod`. The daemon runs as `root` inside the `devenv` container (where Docker grants `CAP_SETUID`/`CAP_SETGID` to root by default), and the brokered client runs as a dedicated `priv-testuser` (uid 2001) whose home is mode `0700`. This mirrors the real deployment model — a privileged daemon serving unprivileged users — and verifies that home-directory traversal comes from the per-child privilege drop rather than from loosened permissions.

## Scenarios

### Scenario: Harness starts a reachable git SSH server

* *GIVEN* docker is available on the host
* *WHEN* the test harness brings up the compose project
* *THEN* the bare repo `repo.git` MUST be reachable via `ssh://git@<container>/srv/git/repo.git`
* *AND* the test MUST be able to clone the repo using the generated key

### Scenario: Push through the gateway to harness succeeds when policy allows

* *GIVEN* the daemon is running with a policy allowing push to the harness repo for the test user
* *AND* the test SSH key is registered in the daemon's credentials directory
* *WHEN* a test process invokes `ghbrk git push origin main` against the harness URL
* *THEN* the push MUST succeed
* *AND* the bare repo's `refs/heads/main` MUST point at the pushed commit

### Scenario: Push through the gateway to harness is rejected when policy denies

* *GIVEN* the daemon's policy has no matching allow rule for push to the harness repo
* *WHEN* a test process invokes `ghbrk git push origin main`
* *THEN* the gateway client MUST exit with a non-zero status
* *AND* the bare repo's `refs/heads/main` MUST NOT be updated
* *AND* the audit log MUST contain a deny record

### Scenario: Clone through the gateway to harness streams progress

* *GIVEN* the daemon is running and policy allows clone
* *WHEN* a test process invokes `ghbrk git clone <harness-url> /tmp/clone`
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
* *WHEN* a test process invokes `ghbrk gh api user`
* *THEN* the command MUST exit with status zero
* *AND* the streamed stdout MUST contain `"login": "test-user"` from the mock API response

### Scenario: gh api through broker — invalid token returns 401

* *GIVEN* the compose project is up with the `mock-github` HTTPS service running and the broker configured with `GH_HOST=mock-github`
* *AND* the daemon is running with a policy allowing `gh_api_read` for the test user
* *AND* no token file exists in the daemon's credentials directory for the test user
* *WHEN* a test process invokes `ghbrk gh api user`
* *THEN* the command MUST exit with a non-zero status
* *AND* the streamed stderr MUST indicate the token is missing

### Scenario: Integration dev container is reachable as root

* *GIVEN* the integration `devenv` service defined in the compose project
* *WHEN* the test harness brings up the compose project
* *THEN* the `devenv` container MUST be running as the `root` user
* *AND* the container MUST have `gh`, `git`, and `ssh` available on its PATH

### Scenario: Push succeeds through a 0700 home directory without chmod (privilege drop e2e)

* *GIVEN* `priv-testuser` (uid 2001) exists inside the `devenv` container with home `/home/priv-testuser` at mode `0700` containing a git clone of the harness repo
* *AND* the ghbrk daemon runs as `root` inside `devenv` with `CAP_SETUID`/`CAP_SETGID`, a policy allowing `push` for `priv-testuser`, and SSH credentials registered in the harness git server
* *AND* no `chmod o+x /home/priv-testuser` is run at any point in the test
* *WHEN* `priv-testuser` invokes `ghbrk git push origin main` against the daemon socket
* *THEN* the push MUST succeed with exit status zero
* *AND* the harness bare repo's `refs/heads/main` MUST point at the pushed commit
* *AND* `stat -c %a /home/priv-testuser` MUST still report `700`
* *AND* the audit log MUST contain an allow record for the push

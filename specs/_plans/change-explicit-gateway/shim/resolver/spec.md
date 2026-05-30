# Feature: resolver

Maps the request `(tool, args, cwd)` to a normalised `(operation, org, repo, branch?)` tuple. This `shim/`-domain copy is removed; the feature is relocated unchanged to `daemon/resolver` because the resolver always ran inside the broker, never in the shim.

## Background

For `git`, the resolver reads `<cwd>/.git/config` (or walks upward) to find the `remote.origin.url`, parses it, and extracts org and repo; for `gh`, it inspects subcommand args or the cwd's git remote. Only GitHub URLs are recognised. The `shim/` domain is being removed entirely, so this copy is deleted and recreated under `daemon/resolver` with identical scenarios.

## Scenarios

<!-- DELTA:REMOVED -->
### Scenario: Resolve git push to push operation

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *AND* `HEAD` points at branch `feature/x`
* *WHEN* the resolver processes `git push origin feature/x`
* *THEN* the resolver MUST produce `{ op: push, org: acme, repo: web, branch: feature/x }`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve git clone with explicit URL

* *GIVEN* the args are `git clone https://github.com/acme/web.git /tmp/work`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce `{ op: clone, org: acme, repo: web, branch: None }`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve git fetch in existing repo

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `git fetch origin`
* *THEN* the resolver MUST produce `{ op: fetch, org: acme, repo: web, branch: None }`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve gh pr create using cwd repo

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *AND* the current branch is `feature/x`
* *WHEN* the resolver processes `gh pr create --title foo`
* *THEN* the resolver MUST produce `{ op: pr_open, org: acme, repo: web, branch: feature/x }`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve gh pr create with explicit -R flag

* *GIVEN* the args are `gh pr create -R other/proj --title bar`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce `{ op: pr_open, org: other, repo: proj, branch: ... }`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve gh issue close

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `gh issue close 42`
* *THEN* the resolver MUST produce `{ op: issue_close, org: acme, repo: web, branch: None }`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Reject non-GitHub remote URL

* *GIVEN* `cwd` has `remote.origin.url=git@gitlab.com:acme/web.git`
* *WHEN* the resolver runs
* *THEN* the resolver MUST return an error indicating the host is not GitHub
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Reject git command outside any repo when remote is needed

* *GIVEN* `cwd` is `/tmp` and contains no git repo
* *WHEN* the resolver processes `git push`
* *THEN* the resolver MUST return an error indicating no repository was found
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Unknown git subcommand maps to a sentinel and is denied by default

* *GIVEN* the args are `git unknown-cmd`
* *WHEN* the resolver runs
* *THEN* the resolver MUST return an error or an `unknown` operation
* *AND* the daemon MUST treat the request as denied
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve git pull in existing repo

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `git pull origin`
* *THEN* the resolver MUST produce `{ op: pull, org: acme, repo: web, branch: None }`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve git pull rejects non-GitHub remote

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@gitlab.com:acme/web.git`
* *WHEN* the resolver processes `git pull`
* *THEN* the resolver MUST return an error indicating the host is not GitHub
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve git pull outside any repo is rejected

* *GIVEN* `cwd` is `/tmp` and contains no git repo
* *WHEN* the resolver processes `git pull`
* *THEN* the resolver MUST return an error indicating no repository was found
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve gh api to a read operation carrying the path

* *GIVEN* the args are `gh api user`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `user`
* *AND* the resolver MUST NOT require a GitHub remote in `cwd`
* *AND* the resolver MUST leave org and repo unset (matched as wildcard by the policy)
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Resolve gh api with a nested path

* *GIVEN* the args are `gh api repos/acme/web`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `repos/acme/web`
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: gh api with no path is rejected

* *GIVEN* the args are `gh api`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST return an error indicating the API path is missing
<!-- /DELTA:REMOVED -->

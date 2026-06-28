# Feature: resolver

Maps the broker request `(tool, args, cwd)` to a normalised `(operation, org, repo, branch?)` tuple for `git` invocations, so the policy engine has a stable input regardless of how the caller spelled the command. `gh` invocations are covered by the sibling `daemon/resolver-gh` (pull requests, issues, `gh api`) and `daemon/resolver-release` (`gh release` lifecycle) features.

## Background

The resolver runs inside the broker daemon (`src/broker.rs::resolve_request`), not in the calling process. For `git`, the resolver reads `<cwd>/.git/config` (or walks upward) to find the `remote.origin.url`, parses it, and extracts org and repo. Branch resolution for `git push` reads the local refspec or `HEAD`. Read-side git operations (`fetch`, `clone`, `pull`) do not populate a branch — the policy decides at the operation level. Only GitHub URLs are recognised; other forges produce an error. This feature is relocated unchanged from the removed `shim/` domain — the resolver was always a broker-side concern.

This feature covers a plain `.git` directory checkout. See `daemon/repo-discovery` for how a linked worktree, a submodule, a `.git` file, or a `.git` symlink resolves, and `daemon/repo-discovery-boundaries` for what happens when discovery meets an unusable or dangling `.git` entry.

## Scenarios

### Scenario: Resolve git push to push operation

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *AND* `HEAD` points at branch `feature/x`
* *WHEN* the resolver processes `git push origin feature/x`
* *THEN* the resolver MUST produce `{ op: push, org: acme, repo: web, branch: feature/x }`

### Scenario: Resolve git clone with explicit URL

* *GIVEN* the args are `git clone https://github.com/acme/web.git /tmp/work`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce `{ op: clone, org: acme, repo: web, branch: None }`

### Scenario: Resolve git fetch in existing repo

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `git fetch origin`
* *THEN* the resolver MUST produce `{ op: fetch, org: acme, repo: web, branch: None }`

### Scenario: Reject non-GitHub remote URL

* *GIVEN* `cwd` has `remote.origin.url=git@gitlab.com:acme/web.git`
* *WHEN* the resolver runs
* *THEN* the resolver MUST return an error indicating the host is not GitHub

### Scenario: Reject git command outside any repo when remote is needed

* *GIVEN* `cwd` is `/tmp` and contains no git repo
* *WHEN* the resolver processes `git push`
* *THEN* the resolver MUST return an error indicating no repository was found

### Scenario: Unknown git subcommand maps to a sentinel and is denied by default

* *GIVEN* the args are `git unknown-cmd`
* *WHEN* the resolver runs
* *THEN* the resolver MUST return an error or an `unknown` operation
* *AND* the daemon MUST treat the request as denied

### Scenario: Resolve git pull in existing repo

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `git pull origin`
* *THEN* the resolver MUST produce `{ op: pull, org: acme, repo: web, branch: None }`

### Scenario: Resolve git pull rejects non-GitHub remote

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@gitlab.com:acme/web.git`
* *WHEN* the resolver processes `git pull`
* *THEN* the resolver MUST return an error indicating the host is not GitHub

### Scenario: Resolve git pull outside any repo is rejected

* *GIVEN* `cwd` is `/tmp` and contains no git repo
* *WHEN* the resolver processes `git pull`
* *THEN* the resolver MUST return an error indicating no repository was found

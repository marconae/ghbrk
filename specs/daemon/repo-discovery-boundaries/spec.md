# Feature: repo-discovery-boundaries

Defines what repository discovery does when the first `.git` entry it finds is unusable — a dangling pointer, a missing `config`, or an entry that is neither a directory nor a file — and confirms discovery never falls through to an enclosing repository. Split out of `daemon/resolver`, alongside `daemon/repo-discovery`, which covers straightforward worktree and submodule resolution.

## Background

Discovery stops at the first `.git` entry even when that entry turns out to be unusable, matching git. A dangling `gitdir:` pointer, a `commondir` naming a directory with no readable `config`, a `.git` directory with no `config`, and a `.git` entry that, after symlinks are followed, is neither a directory nor a regular file, including a dangling symlink, all report no repository rather than falling through to an enclosing repository, because falling through would silently evaluate the policy against the wrong repo or the wrong branch. All of these cases surface as the existing no-repository error; there is no distinct error for a broken worktree link. A `.git` symlink naming a real git directory or a real `.git` file resolves exactly as its target does — see `daemon/repo-discovery` for that mechanism; only a *dangling* symlink or an unusable target stops discovery here.

## Scenarios

### Scenario: Reject a non-GitHub remote reached through a worktree link

* *GIVEN* a main checkout whose `.git/config` has `remote.origin.url=git@gitlab.com:acme/web.git`
* *AND* `cwd` is a linked worktree of that checkout
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST return an error indicating the host is not GitHub
* *AND* the resolver MUST NOT return the no-repository error

### Scenario: A worktree nested inside another repository does not resolve to the enclosing repository

* *GIVEN* `<outer>` is a checkout whose `.git/config` has `remote.origin.url=git@github.com:acme/outer.git` and whose `HEAD` points at branch `main`
* *AND* `cwd` is `<outer>/inner`, a linked worktree of a different checkout whose `remote.origin.url` is `git@github.com:acme/other.git`
* *AND* the `<outer>/inner` worktree's own `HEAD` points at branch `feature/x`
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST produce `{ op: push, org: acme, repo: other, branch: feature/x }`
* *AND* the resolver MUST stop the upward walk at `<outer>/inner/.git` and MUST NOT read `<outer>/.git/config`

### Scenario: Report no repository when a .git file points at a missing git directory

* *GIVEN* `cwd` contains a `.git` file holding `gitdir: /nonexistent/path`
* *AND* no ancestor of `cwd` is a git repository
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST return the no-repository error naming `cwd`

### Scenario: Report no repository when a worktree commondir points at a directory with no config

* *GIVEN* `cwd` contains a `.git` file naming a private git directory that exists and holds `HEAD`
* *AND* that private directory holds a `commondir` file naming a directory with no readable `config`
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST return the no-repository error naming `cwd`

### Scenario: Discovery stops at the first .git entry even when that entry is unusable

* *GIVEN* `<outer>` is a checkout whose `.git/config` has `remote.origin.url=git@github.com:acme/outer.git`
* *AND* `cwd` is `<outer>/nested`, which contains a `.git` directory holding no `config` file
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST return the no-repository error naming `cwd`
* *AND* the resolver MUST NOT resolve to `acme/outer`

### Scenario: Report no repository when the .git entry is neither a directory nor a regular file

* *GIVEN* `<outer>` is a checkout whose `.git/config` has `remote.origin.url=git@github.com:acme/outer.git`
* *AND* `cwd` is `<outer>/nested`, whose `.git` is a symlink naming a target that does not exist
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST stop the upward walk at `<outer>/nested/.git`, because a `.git` entry exists there
* *AND* the resolver MUST return the no-repository error naming `cwd`
* *AND* the resolver MUST NOT resolve to `acme/outer`

### Scenario: Resolve a checkout whose .git is a symlink naming a real git directory

* *GIVEN* `cwd` contains a `.git` symlink whose target is a real git directory
* *AND* that directory holds `config` with `remote.origin.url=git@github.com:acme/web.git` and a `HEAD` pointing at branch `feature/x`
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST produce `{ op: push, org: acme, repo: web, branch: feature/x }`
* *AND* the resolver MUST classify the `.git` entry after following the symlink, resolving it exactly as its target resolves

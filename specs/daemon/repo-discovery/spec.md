# Feature: repo-discovery

Resolves the git repository containing `cwd` through git's own private/common directory split — following a `.git` file's `gitdir:` pointer and an optional `commondir` file — so a linked worktree or a submodule checkout resolves to the repository git itself resolves, not to an enclosing checkout. Split out of `daemon/resolver`, alongside `daemon/repo-discovery-boundaries`, which covers discovery's stop-at-first-entry and unusable-entry semantics.

## Background

Repository discovery (`src/resolver.rs::LocalRepo::discover`) follows git's own rules. It walks upward from `cwd` and stops at the first ancestor that holds a `.git` entry of any type, and it classifies that entry after symlinks are followed. A `.git` directory is itself the *private* git directory. A `.git` file names the private git directory instead, through a single `gitdir: <path>` line; git writes an absolute path for a linked worktree and a relative path for a submodule, and both forms are accepted, with a relative path resolved against the directory holding the `.git` file. Inside the private directory, however that directory was reached, an optional `commondir` file names the *common* git directory shared with the main checkout, itself absolute or resolved relative to the private directory. Real git honours `commondir` in a plain `.git` directory exactly as it does in a worktree's private directory, so the resolver reads it for both rather than assuming a `.git` directory is its own common directory. When `commondir` is absent, the private directory is also the common directory, which is the plain-checkout and submodule shape. Real git writes both pointer files with a trailing newline, so the resolver reads the `gitdir:` value and the whole `commondir` file content with surrounding whitespace and the trailing newline removed.

The two directories are not interchangeable, so the resolver never collapses them into one path. The common directory owns `config` and therefore the remote URL. The resolver reads `remote.origin.url` from `<common>/config` only: it does not read `<private>/config.worktree`, `remote.<name>.pushurl`, or a second `url =` value under the same remote. The private directory owns `HEAD` and therefore the branch, which differs per worktree. Reading `HEAD` from the common directory would report the main checkout's branch for a push made from a linked worktree, and the policy engine keys pushes on the branch.

The same discovery routine serves both sides of the privilege boundary. `ghbrk git`, `ghbrk gh`, and `ghbrk explain` call it in the invoking user's process to compute the remote-URL and HEAD-branch hints carried on the request (`src/resolver.rs::repo_hints`); the broker calls it as a fallback when a request carries no URL hint. A request that does carry a URL hint skips filesystem discovery for the remote; see `daemon/resolver-gh` for `gh pr create`'s branch fallback when a request carries no branch hint. See `daemon/repo-discovery-boundaries` for what happens when the `.git` entry or its pointers are unusable.

## Scenarios

### Scenario: Resolve git push from a linked worktree

* *GIVEN* a main checkout whose `.git/config` has `remote.origin.url=git@github.com:acme/web.git`
* *AND* `cwd` is a linked worktree whose `.git` is a file containing `gitdir: <main>/.git/worktrees/wt`
* *AND* `<main>/.git/worktrees/wt` holds a `HEAD` pointing at branch `feature/x` and a `commondir` naming `<main>/.git`
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST produce `{ op: push, org: acme, repo: web, branch: feature/x }`
* *AND* the resolver MUST read the branch from the worktree's own `HEAD`, not from the main checkout's `HEAD`

### Scenario: Resolve git fetch from a linked worktree

* *GIVEN* a main checkout whose `.git/config` has `remote.origin.url=https://github.com/acme/web.git`
* *AND* `cwd` is a linked worktree of that checkout
* *WHEN* the resolver processes `git fetch origin` with no URL hint
* *THEN* the resolver MUST produce `{ op: fetch, org: acme, repo: web, branch: None }`

### Scenario: Resolve git pull from a linked worktree

* *GIVEN* a main checkout whose `.git/config` has `remote.origin.url=https://github.com/acme/web.git`
* *AND* `cwd` is a linked worktree of that checkout
* *WHEN* the resolver processes `git pull` with no URL hint
* *THEN* the resolver MUST produce `{ op: pull, org: acme, repo: web, branch: None }`

### Scenario: Resolve a checkout whose .git file names a self-contained git directory

* *GIVEN* `cwd` contains a `.git` file holding the relative pointer `gitdir: ../../.git/modules/vendor/sub`
* *AND* the named directory holds `config` with `remote.origin.url=git@github.com:acme/sub.git` and holds no `commondir` file
* *WHEN* the resolver processes `git fetch` with no URL hint
* *THEN* the resolver MUST produce `{ op: fetch, org: acme, repo: sub, branch: None }`
* *AND* the resolver MUST resolve the relative `gitdir:` pointer against the directory holding the `.git` file
* *AND* the resolver MUST treat the named directory as both the private and the common git directory

### Scenario: A commondir file in a plain .git directory redirects the remote

* *GIVEN* `cwd` contains a `.git` directory holding `config` with `remote.origin.url=git@github.com:acme/private.git` and a `HEAD` pointing at branch `feature/x`
* *AND* that `.git` directory holds a `commondir` file naming a separate directory whose `config` has `remote.origin.url=git@github.com:acme/common.git`
* *WHEN* the resolver processes `git push` with no URL hint
* *THEN* the resolver MUST produce `{ op: push, org: acme, repo: common, branch: feature/x }`
* *AND* the resolver MUST read the remote from the named common directory, as `git` itself does when run in that checkout
* *AND* the resolver MUST read the branch from the `.git` directory's own `HEAD`
* *AND* the resolver MUST accept both an absolute pointer and one resolved relative to the `.git` directory

### Scenario: The client-side hint pass reads remote URL and branch from a linked worktree

* *GIVEN* a main checkout whose `.git/config` has `remote.origin.url=git@github.com:acme/web.git`
* *AND* `cwd` is a linked worktree of that checkout whose own `HEAD` points at branch `feature/x`
* *WHEN* `ghbrk git`, `ghbrk gh`, or `ghbrk explain` computes the request hints in the invoking user's process
* *THEN* the hint pass MUST report remote URL `git@github.com:acme/web.git`
* *AND* the hint pass MUST report head branch `feature/x`
* *AND* all three commands MUST compute the same hints from the same working directory

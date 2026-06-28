# Decisions: fix-resolver-worktree-support

## ADR: Repository discovery is owned by one type, `LocalRepo`, not by a git-dir path handed to callers

**ID:** local-repo-owns-private-common-split
**Plan:** fix-resolver-worktree-support
**Status:** Accepted

### Context

A linked worktree has two git directories: a private directory holding `HEAD` (the branch, which differs per worktree) and a common directory holding `config` (the remote, shared by every worktree). The prior `find_git_dir(&Path) -> Option<PathBuf>` returned one path, so it could not answer both questions correctly at once. Five call sites (`src/cmd/git.rs`, `src/cmd/gh.rs`, `src/cmd/explain.rs`, `resolve_remote_url`, `resolve_gh`) each performed the same `find_git_dir` then read-origin-url-and-head-branch sequence.

### Decision

Replace `find_git_dir` with `LocalRepo::discover(&Path) -> Option<LocalRepo>`, holding a private dir and a common dir, exposing only `origin_url()` (reads `common/config`) and `head_branch()` (reads `private/HEAD`). No caller receives a raw git-dir path.

### Options Considered

| Option | Verdict |
|--------|---------|
| `LocalRepo` type with accessor methods | ✓ Chosen — one type owns the private/common decision; a caller handed two methods cannot swap them the way it can swap two same-typed paths |
| Return `(PathBuf, PathBuf)` from `find_git_dir` | ✗ Rejected — two same-typed paths let a caller read `HEAD` from the common dir with no compiler complaint, reproducing the worktree bug in a new form |
| Keep `find_git_dir`; add a parallel `find_git_common_dir` | ✗ Rejected — walks the tree twice and still leaves every one of the five call sites knowing which directory holds `config` and which holds `HEAD` |

### Consequences

Every call site gets the branch and the remote from the directory that actually owns each, by construction. Adding a sixth call site cannot reintroduce the private/common confusion without a type error.

## ADR: Repository discovery reads the `.git` file directly; it never shells out to `git rev-parse`

**ID:** resolver-reads-git-files-not-subprocess
**Plan:** fix-resolver-worktree-support
**Status:** Accepted

### Context

`git rev-parse --git-dir --git-common-dir` is the documented way to get a worktree's private and common directories. The broker resolves a request before the policy decision, running as the privileged `ghbrk` user, against a repository an untrusted caller controls.

### Decision

`LocalRepo::discover` parses the `.git` file's `gitdir:` line and the optional `commondir` file with plain filesystem reads. It does not invoke a `git` subprocess.

### Options Considered

| Option | Verdict |
|--------|---------|
| Plain filesystem reads of `.git`, `gitdir:`, and `commondir` | ✓ Chosen — two small file reads, no subprocess, no new attack surface at the trust boundary |
| Shell out to `git rev-parse --git-dir --git-common-dir` | ✗ Rejected — hands an untrusted repository influence over resolution through `PATH`, repository-local `core.*` settings, and `include.path`, at the exact boundary ghbrk exists to defend; the daemon host may also carry no `git` binary at all |

### Consequences

Resolution has no dependency on a `git` binary being present or trustworthy on the daemon host. The residual trust exposure — a `.git` file can name an arbitrary path that the broker then reads as `config`/`HEAD` before the policy decision — is not a new class of exposure: `Request.cwd` was already unvalidated client input, and the broker already read `<ancestor-of-cwd>/.git/config` from it.

## ADR: Discovery stops at the first `.git` entry, whether or not that entry is usable

**ID:** discovery-stops-at-first-git-entry
**Plan:** fix-resolver-worktree-support
**Status:** Accepted

### Context

The prior walk skipped a `.git` entry it could not use (e.g., a `.git` directory with no `config`) and continued upward to an enclosing repository. Verified against git 2.47.3: a linked worktree nested inside another repository, or a submodule nested in a superproject, mis-resolved to the outer repository's org, repo, or branch under that skip rule — a policy-correctness defect, since the broker gates per-repo and per-branch.

### Decision

The upward walk stops at the first ancestor for which `symlink_metadata(<dir>/.git)` succeeds. The entry is then classified by `metadata`, which follows symlinks: a directory is used as-is, a regular file is read as a `gitdir:` pointer, and any other type — or a failing `metadata` call, or a resolved entry that fails to reach a common dir with a readable `config` — yields `None`. The walk never continues past the stop point to an enclosing repository. A `.git` symlink naming a real git directory or a real `.git` file resolves exactly as its target does; only a dangling symlink or an unusable target stops discovery.

### Options Considered

| Option | Verdict |
|--------|---------|
| Stop at the first `.git` entry unconditionally; classify after following symlinks | ✓ Chosen — matches git's own discovery; one rule instead of two; no existing test depended on the skip behavior |
| Stop only on a `.git` file; keep skipping a `config`-less `.git` directory | ✗ Rejected — two stop rules is one rule a future reader must memorise for no benefit, and the skip case carries the identical wrong-repo hazard |
| Keep falling through on any unusable entry | ✗ Rejected — silently binds the policy decision to the wrong repository or branch, which is worse than reporting no repository |

### Consequences

A directory holding an unusable `.git` entry — a hand-built directory missing `config`, a partial clone, or a dangling `.git` symlink — now reports no repository instead of silently resolving to an enclosing checkout. `git init` always writes a `config`, so real checkouts are unaffected; only hand-built or corrupted `.git` entries are.

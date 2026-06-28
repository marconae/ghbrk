# Feature: resolver-gh

Maps `gh pr`, `gh issue`, and `gh api` invocations to a normalised `(operation, org, repo, branch?)` tuple, so the policy engine has a stable input regardless of how the caller spelled the command. Split from `daemon/resolver`, which covers `git` invocation resolution; `daemon/resolver-release` covers the `gh release` lifecycle surface.

## Background

The resolver runs inside the broker daemon (`src/broker.rs::resolve_request`), not in the calling process. For `gh`, the resolver inspects subcommand args (e.g. `gh pr create -R acme/web` or the current cwd's git remote). Classification (`src/resolver.rs::classify_gh`) keys only on the first two non-flag positional tokens (`group` and `action`) and deliberately ignores all other flags and values. The `gh api <path>` operation is user-scoped: it carries the raw API path and does not require a GitHub remote, so org and repo are left unset (matched as wildcard by the policy). Only GitHub URLs are recognised for repo-scoped operations; other forges produce an error.

`gh pr create`'s branch comes from `HEAD` of the repository `daemon/repo-discovery` resolves for `cwd` — a linked worktree's own `HEAD`, not an enclosing checkout's — and that lookup still runs even when the request carries a URL hint for the remote, since a URL hint only substitutes for the remote-discovery step, not the branch one.

Skipping flags means skipping the values that belong to them. A token is positional only when no preceding token claimed it, so the resolver classifies each `-`-prefixed token by arity before it decides what the next token is. `gh` accepts flags before positionals, so `gh api --input body.json repos/acme/web` and `gh api --jq .name repos/acme/web` are legal spellings whose API path is `repos/acme/web`; reading the first non-flag token without arity would take `body.json` or `.name` as the path and record the wrong operation in the audit log and the wrong input to the policy engine.

The arity table applies only when the invocation's first non-flag token is `api`. That scope is load-bearing. `gh_positional_args` is the one function every `gh` invocation passes through on its way to `classify_gh`, and the same short spellings mean different things elsewhere: `-f` is the boolean `--fill` of `gh pr create`, and `-t` is the value-taking `--title` of `gh release create`. Applying the table to every invocation would change positional extraction for `gh pr`, `gh issue`, and `gh release` as a side effect of fixing `gh api`. The group and action tokens stay the first two non-flag tokens under the pre-existing no-arity rule, so classification of every non-`api` invocation is byte-for-byte unchanged.

The table lists the value-taking flags of `gh api`, the one subcommand whose positional carries semantic weight beyond the two classification tokens: `-X`/`--method`, `-F`/`--field`, `-f`/`--raw-field`, `-H`/`--header`, `-q`/`--jq`, `-t`/`--template`, `--input`, `--cache`, and `--hostname`. Boolean flags such as `--paginate`, `--slurp`, `--silent`, `--verbose`, and `-i`/`--include` are deliberately absent: listing one would make the resolver swallow the API path that follows it. A `=`-joined spelling (`--jq=.name`) carries its own value and consumes no following token.

A short flag with an attached value carries its own value too. `gh` accepts `-XPOST` and `-q.name` alongside `-X POST` and `-q .name`, and `src/resolver.rs::gh_api_method` already recognises the attached spelling, so both reach the resolver today. A table entry matches a token only on exact equality with the whole token, never as a prefix of it. Prefix matching would make `-XPOST` match the `-X` entry and skip the endpoint that follows, and would make `--jq=.name` match the `--jq` entry and do the same — the defect the table exists to remove.

## Scenarios

### Scenario: Resolve gh pr create using cwd repo

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *AND* the current branch is `feature/x`
* *WHEN* the resolver processes `gh pr create --title foo`
* *THEN* the resolver MUST produce `{ op: pr_open, org: acme, repo: web, branch: feature/x }`

### Scenario: Resolve gh pr create with explicit -R flag

* *GIVEN* the args are `gh pr create -R other/proj --title bar`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce `{ op: pr_open, org: other, repo: proj, branch: ... }`

### Scenario: Resolve gh issue close

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `gh issue close 42`
* *THEN* the resolver MUST produce `{ op: issue_close, org: acme, repo: web, branch: None }`

### Scenario: Resolve gh api to a read operation carrying the path

* *GIVEN* the args are `gh api user`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `user`
* *AND* the resolver MUST NOT require a GitHub remote in `cwd`
* *AND* the resolver MUST leave org and repo unset (matched as wildcard by the policy)

### Scenario: Resolve gh api with a nested path

* *GIVEN* the args are `gh api repos/acme/web`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `repos/acme/web`

### Scenario: gh api with no path is rejected

* *GIVEN* the args are `gh api`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST return an error indicating the API path is missing

### Scenario: Resolve gh pr create from a linked worktree using the worktree's own HEAD

* *GIVEN* a main checkout whose `.git/config` has `remote.origin.url=git@github.com:acme/web.git` and whose `HEAD` points at branch `main`
* *AND* `cwd` is a linked worktree of that checkout whose own `HEAD` points at branch `feature/x`
* *WHEN* the resolver processes `gh pr create --title foo` with no URL hint and no branch hint
* *THEN* the resolver MUST produce `{ op: pr_open, org: acme, repo: web, branch: feature/x }`
* *AND* the resolver MUST NOT report the main checkout's branch `main`

### Scenario: Resolve gh pr create from a linked worktree when the request carries a URL hint

* *GIVEN* a main checkout whose `.git/config` has `remote.origin.url=git@github.com:acme/web.git` and whose `HEAD` points at branch `main`
* *AND* `cwd` is a linked worktree of that checkout whose own `HEAD` points at branch `feature/x`
* *WHEN* the resolver processes `gh pr create --title foo` with URL hint `git@github.com:acme/web.git` and no branch hint
* *THEN* the resolver MUST produce `{ op: pr_open, org: acme, repo: web, branch: feature/x }`
* *AND* the resolver MUST discover the repository to read the worktree's own `HEAD`, even though the URL hint made discovery of the remote unnecessary

### Scenario: gh api path is not taken from the value of a preceding file flag

* *GIVEN* the args are `gh api --input /tmp/body.json repos/acme/web`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `repos/acme/web`
* *AND* the resolver MUST NOT carry the API path `/tmp/body.json`

### Scenario: gh api path is not taken from the value of a preceding jq flag

* *GIVEN* the args are `gh api --jq .name repos/acme/web`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `repos/acme/web`
* *AND* the resolver MUST NOT carry the API path `.name`

### Scenario: gh api path is not taken from the value of a preceding field flag

* *GIVEN* the args are `gh api -F name=value repos/acme/web`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `repos/acme/web`

### Scenario: gh api boolean flag does not consume the API path

* *GIVEN* the args are `gh api --paginate repos/acme/web/issues`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `repos/acme/web/issues`
* *AND* the resolver MUST NOT return an error indicating the API path is missing

### Scenario: gh api short flag with an attached value consumes no following token

* *GIVEN* the args are `gh api -XPOST repos/acme/web`
* *AND* `-XPOST` carries its own value, so no table entry matches it by exact token equality
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation carrying the API path `repos/acme/web`
* *AND* the resolver MUST NOT skip the token following `-XPOST`

### Scenario: gh api equals-joined flag value consumes no following token

* *GIVEN* the args are `gh api --jq=.name repos/acme/web`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce an operation `gh_api_read` carrying the API path `repos/acme/web`

### Scenario: gh api with a value-taking flag and no path is rejected

* *GIVEN* the args are `gh api --input /tmp/body.json`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST return an error indicating the API path is missing

### Scenario: Classification is unaffected by a file-valued flag after the action token

* *GIVEN* the args are `gh pr comment --body-file /tmp/body.md 42`
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce the operation `pr_comment`
* *AND* the resolver MUST NOT treat `/tmp/body.md` as the group or action token

### Scenario: The arity table does not apply to a non-api invocation

* *GIVEN* the args are `gh pr create -f --title 'Add stdin forwarding'`
* *AND* `-f` is the boolean `--fill` of `gh pr create`, while `-f` in the `gh api` arity table takes a value
* *WHEN* the resolver processes the request
* *THEN* the resolver MUST produce the operation `pr_open`
* *AND* the resolver MUST NOT apply the `gh api` arity table, because the first non-flag token is `pr` rather than `api`

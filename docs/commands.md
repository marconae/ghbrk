[ghbrk](../README.md) / [Docs](./index.md) / Commands

---

# Commands

## `ghbrk git <remote-subcommand> [args]`

Relays a remote git operation to the broker. Only remote-capable subcommands (`push`, `fetch`, `pull`, `clone`) are accepted; local-only subcommands return a guidance error on stderr without contacting the broker.

```bash
ghbrk git push origin main
ghbrk git fetch
ghbrk git pull --rebase
ghbrk git clone git@github.com:acme/platform
```

## `ghbrk gh <subcommand> [args]`

Relays any `gh` invocation to the broker. The broker injects `GH_TOKEN` from stored credentials and, for mutating operations, evaluates the configured policy.

```bash
ghbrk gh pr create --title "My PR" --body "..."
ghbrk gh pr merge 42 --squash
ghbrk gh issue comment 7 --body "Fixed in abc123"
```

## `ghbrk doctor`

Checks daemon reachability, credential presence and file-permission validity, and policy-file parsability. Prints one status line per check; exits zero only when all checks pass.

```bash
ghbrk doctor
```

## `ghbrk explain <cmd> [args]`

Dry run: sends the command to the broker, which resolves the operation and evaluates policy without executing anything. Reports the would-be decision and which credential would be injected.

```bash
ghbrk explain git push origin main    # shows what the broker would do
ghbrk explain git status              # reports: local operation, out of scope
```

## `ghbrk policy <org>/<repo>`

Lists which operations the calling user is allowed and forbidden to perform on the specified repository, based on the current policy file.

```bash
ghbrk policy acme/web
```

## `ghbrk allow <org>/<repo> <operations|role>`

Grants one or more operations (or a named role) on a repository for the calling user. Appends a rule to the policy file and restarts the daemon.

```bash
ghbrk allow acme/platform push fetch pull
ghbrk allow acme/platform pr_open pr_comment
```

Built-in roles: `read-only`, `write`, `maintain`, `admin` — see [Policy Reference](./policy.md#built-in-roles).

## `ghbrk daemon`

Starts the broker server. Normally managed by systemd; you do not need to invoke this manually.

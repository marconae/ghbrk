[ghbrk](../README.md) / [Docs](./index.md) / Policy Reference

---

# Policy Reference

## Policy file

`/etc/ghbrk/policy.yaml` controls which callers may perform which operations. Rules are evaluated top-to-bottom; the **first match wins**. If no rule matches, the request is **denied** (default-deny).

```yaml
rules:
  # alice can push to feature branches in acme/platform
  - user: alice
    org: acme
    repo: platform
    operations: [push]
    branches: ["feature/*"]
    effect: allow

  # any agent may open and comment on PRs across the acme org
  - user: "*"
    org: acme
    repo: "*"
    operations: [pr_open, pr_comment]
    effect: allow

  # nobody may push to main — place before a broader allow to take priority
  - user: "*"
    org: acme
    repo: "*"
    operations: [push]
    branches: [main]
    effect: deny
```

See `config/policy.example.yaml` for a fully annotated example.

## Rule fields

| Field | Required | Description |
|-------|----------|-------------|
| `user` | yes | Unix username or `"*"` (any caller) |
| `org` | yes | GitHub organisation or user, or `"*"` |
| `repo` | yes | Repository name (without org prefix), or `"*"` |
| `operations` | yes | Non-empty list — see [Operations reference](#operations-reference) |
| `branches` | no | Glob list; defaults to `["*"]`; only evaluated for `push` |
| `effect` | yes | `allow` or `deny` |

Reload the policy by restarting the daemon:

```bash
sudo systemctl restart ghbrk
```

---

## Operations reference

Use these names in the `operations` list of your policy rules.

| Operation | Triggered by | Branch-aware |
|-----------|-------------|:------------:|
| `push` | `ghbrk git push` | yes |
| `fetch` | `ghbrk git fetch` | no |
| `pull` | `ghbrk git pull` | no |
| `clone` | `ghbrk git clone` | no |
| `pr_open` | `ghbrk gh pr create` | no |
| `pr_comment` | `ghbrk gh pr comment` | no |
| `pr_close` | `ghbrk gh pr close` | no |
| `pr_merge` | `ghbrk gh pr merge` | no |
| `pr_review` | `ghbrk gh pr review` | no |
| `issue_open` | `ghbrk gh issue create` | no |
| `issue_comment` | `ghbrk gh issue comment` | no |
| `issue_close` | `ghbrk gh issue close` | no |
| `release_create` | `ghbrk gh release create` | no |
| `release_delete` | `ghbrk gh release delete` | no |
| `release_edit` | `ghbrk gh release edit` | no |
| `release_upload` | `ghbrk gh release upload` | no |
| `release_delete_asset` | `ghbrk gh release delete-asset` | no |
| `release_list` | `ghbrk gh release list` | no |
| `release_view` | `ghbrk gh release view` | no |
| `release_download` | `ghbrk gh release download` | no |
| `gh_api_read` | `ghbrk gh api <path>` (GET) | no |

Only `push` evaluates the `branches` field. For all other operations the `branches` field in a rule is ignored.

---

## Command routing

### git

A routing decision is made before contacting the broker. Git subcommands that do not require credential injection are rejected immediately — the broker is never contacted.

| Subcommand | Routed to |
|------------|-----------|
| `push` | broker |
| `fetch` | broker |
| `pull` | broker |
| `clone` (GitHub remote) | broker |
| `status`, `add`, `commit`, `log`, `diff`, `checkout`, and everything else | guidance error (use plain `git`) |

### gh

Every `ghbrk gh` invocation is relayed to the broker so that `GH_TOKEN` is always injected.

| Group + action | Broker path |
|----------------|-------------|
| `pr create`, `pr comment`, `pr merge`, `pr close`, `pr review` | policy check → inject → exec |
| `issue create`, `issue comment`, `issue close` | policy check → inject → exec |
| `release create` | policy check → inject → exec |
| `release delete`, `release edit`, `release upload`, `release delete-asset`, `release list`, `release view`, `release download` | policy check → inject → exec |
| `api <path>` (GET only; `-X POST/PATCH/DELETE` rejected) | policy check → inject → exec |
| `auth status`, `repo view`, `pr list`, `pr status`, and everything else | inject → exec (no policy check) |

Non-policy-gated invocations bypass the resolver and policy engine but still receive `GH_TOKEN`; they are logged with `decision=passthrough`.

---

## Repo resolution

- **`ghbrk git` commands**: the client reads the origin remote URL from `.git/config` in the invoking user's working directory.
- **`ghbrk gh` commands**: uses the `-R`/`--repo` flag if present; otherwise falls back to the git repo context of the working directory.
- **Supported URL formats**: `git@github.com:org/repo`, `ssh://git@github.com/org/repo`, `https://github.com/org/repo` (with or without `.git` suffix).
- Non-GitHub hosts (GitLab, Bitbucket, etc.) are rejected.

---

## Environment variables

| Variable | Default | Scope | Description |
|----------|---------|-------|-------------|
| `GHBRK_SOCKET` | `/var/run/ghbrk/broker.sock` | daemon + client | Unix socket path |
| `GHBRK_POLICY` | `/etc/ghbrk/policy.yaml` | daemon only | Policy file path |
| `GHBRK_AUDIT_LOG` | `/var/log/ghbrk/audit.log` | daemon only | Audit log path |
| `RUST_LOG` | `info` | daemon only | Log verbosity (`debug`, `trace` for troubleshooting) |

To override the socket in both the daemon and client, set `GHBRK_SOCKET` consistently in both environments.

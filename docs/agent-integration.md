[ghbrk](../README.md) / [Docs](./index.md) / Agent Integration

---

# Agent Integration

## How agents use ghbrk

Agents use plain `git`/`gh` for local and read-only operations, and `ghbrk git`/`ghbrk gh` for remote and authenticated operations. No `PATH` manipulation or symlink setup is required.

```bash
# Local operations — plain git, no broker
git status
git add -p
git commit -m "fix: correct off-by-one"
git log --oneline -10
git diff HEAD~1

# Remote operations — explicit broker call
ghbrk git push origin feature/my-branch
ghbrk git fetch
ghbrk git pull --rebase
ghbrk gh pr create --title "My PR" --fill
ghbrk gh pr merge 42 --squash
```

## Group membership

The agent's Unix user must be a member of the `ghbrk-clients` group to reach the broker socket:

```bash
sudo usermod -aG ghbrk-clients alice
```

Log out and back in for the group change to take effect. If the broker socket is not at the default path, set `GHBRK_SOCKET` in the agent's environment.

## Guidance errors

If an agent mistakenly calls `ghbrk git status` (a local subcommand), `ghbrk` exits non-zero immediately — before contacting the broker — and prints:

```
error: 'git status' is a local operation; run 'git status' directly.
       ghbrk only brokers remote operations: push, fetch, pull, clone.
```

This makes the boundary self-documenting and easy to debug.

## Dry-run before running

Use `ghbrk explain` to preview what the broker would do without executing anything:

```bash
ghbrk explain git push origin main
ghbrk explain gh pr create --title "test"
```

This is useful for validating policy rules during setup or when debugging unexpected denials.

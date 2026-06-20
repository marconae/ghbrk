# ghbrk — git/gh credential broker

- ghbrk is a privilege-separated daemon: it holds SSH keys and GitHub tokens so agents never see credentials
- Every remote git/gh operation must be prefixed with `ghbrk`; local operations use plain `git`
- **Remote — use `ghbrk` prefix**:
  - `ghbrk git push origin <branch>`
  - `ghbrk git fetch` / `ghbrk git pull` / `ghbrk git clone <url>`
  - `ghbrk gh pr create` / `ghbrk gh pr merge` / `ghbrk gh pr comment`
  - `ghbrk gh issue create` / `ghbrk gh issue comment`
  - `ghbrk gh release create`
  - `ghbrk gh api <endpoint>`
- **Local — plain git, no prefix**: `git status`, `git add`, `git commit`, `git log`,
  `git diff`, `git checkout`, `git merge`, `git rebase` (local-only rebases)
- **Never call** `git push`, `git fetch`, `git pull`, `git clone`, or any `gh` subcommand directly
- Dry-run: `ghbrk explain git push origin main` — preview policy decision without executing
- Denied? The operator controls `/etc/ghbrk/policy.yaml`; run `ghbrk doctor` to diagnose

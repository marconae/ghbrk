<div align="center">

<img src="assets/logo.svg" alt="ghbrk Logo" width="200">

**A credential broker for AI coding agents on Linux**

[![CI](https://github.com/marconae/ghbrk/actions/workflows/ci.yml/badge.svg)](https://github.com/marconae/ghbrk/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org/)
[![spec|driven](https://img.shields.io/badge/spec-driven-blue)](specs/)
[![agentic|engineering](https://img.shields.io/badge/agentic-engineering-purple)](https://deliberate.codes)

[Getting Started](#getting-started) • [Why](#why-i-built-it) • [How It Works](#how-it-works) • [Documentation](./docs/)

</div>

---

## How It Works

Your SSH key and GitHub token live in `/etc/ghbrk/credentials/$USER`, a directory owned by the `ghbrk` system user, mode `0600`; your agent's own Unix account cannot read it. `ghbrk` doesn't intercept your git commands; you call it explicitly for the operations that need those credentials.

```bash
$ git commit -m "fix: correct retry backoff"
[main e4f5a6b] fix: correct retry backoff

# A plain push fails: the agent's Unix user holds no SSH key.
$ git push origin main
git@github.com: Permission denied (publickey).
fatal: Could not read from remote repository.

# Ask ghbrk what it would do — nothing leaves the machine.
$ ghbrk explain git push origin main
tool:      git push
operation: push
repo:      acme/platform
branch:    main
policy:    allow
inject:    SSH credential

# Broker the push
$ ghbrk git push origin main
...
To github.com:acme/platform.git
   1a2b3c4..e4f5a6b  main -> main
```

---

## Getting Started

```bash
curl -fsSL https://raw.githubusercontent.com/marconae/ghbrk/main/install.sh | sudo bash
```

> [!NOTE]
> **Agent wiring included.** The installer places `ghbrk.md` in `~/.claude/` and prepends `@ghbrk.md` to `~/.claude/CLAUDE.md` (Claude Code), and appends it to `~/.codex/AGENTS.md` (Codex). Agents learn which operations require the `ghbrk` prefix automatically. Pass `--no-claude` or `--no-codex` to skip wiring.

> [!IMPORTANT]
> Requires Linux with `systemd` and an `x86_64` CPU. See [Installation](./docs/install.md) for credential setup and policy configuration.

---

## Why I Built It

I run autonomous AI coding agents. Agents can expose your GitHub credentials in ways you might not notice.

For example, when an agent:
- reads `~/.ssh/config` or `~/.ssh/id_rsa` to figure out how to push — your private key ends up in the context window
- runs `echo $GITHUB_TOKEN` to debug a failing `gh` call — your token lands in the session transcript

So I built `ghbrk` to prevent agents from getting access to your GitHub credentials.

The daemon holds your SSH key and GitHub token. Agents never see them. Every remote git and gh operation is checked against a policy you control, and every decision is logged.

---

## Who Should Use It?

If you are an agentic engineer running autonomous agents — coding assistants, CI bots, automated reviewers — and you give those agents access to GitHub, then `ghbrk` was built for you.

---

## Architecture

```
  Agent
    │
    │  ghbrk git push / ghbrk gh pr create
    │  (explicit — no transparent interception)
    ▼
  ghbrk daemon  (holds your SSH key and token)
    │
    ├─ checks policy ──── allow → runs git / gh with credentials injected
    │                              streams output back to the agent
    │
    └──────────────────── deny  → returns error, logs the decision
```

1. **Agents call `ghbrk` explicitly** for remote operations. Local commands (`git status`, `git commit`) run as usual, without going through the broker.
2. **The daemon checks policy** — owned by root, not readable by the agent. The agent cannot see or modify what it is allowed to do.
3. **Credentials are injected at execution time.** The agent process never sees the SSH key or token.
4. **Every decision is logged** to an append-only audit log.

The policy is a YAML file you write and only root can change. Only the repos, operations, and branches you explicitly allow will go through.

```yaml
rules:
  - user: alice
    org: acme
    repo: platform
    operations: [push]
    branches: ["feature/*"]
    effect: allow
```

Everything else is denied by default.

---

## Documentation

| Guide | Description |
|-------|-------------|
| [Installation](./docs/install.md) | Install from binary, provision credentials, write a policy |
| [Commands](./docs/commands.md) | `ghbrk git`, `ghbrk gh`, `doctor`, `explain`, `policy`, `allow` |
| [Policy Reference](./docs/policy.md) | Rules, operations, branch matching, environment variables |
| [Agent Integration](./docs/agent-integration.md) | How to wire up an agent to use `ghbrk` |
| [Audit Log](./docs/audit-log.md) | Log format and example entries |

---

<div align="center">

Built with Rust 🦀 and made with ❤️ by [marconae](https://deliberate.codes).

</div>

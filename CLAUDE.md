# CLAUDE.md — ghbrk development rules

## Git commits

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>
```

Common types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`.
Scope is optional but encouraged (e.g. `daemon`, `shim`, `policy`, `deploy`).

## Dependencies

`ghbrk` is MIT-licensed. All dependencies must use a permissive license:
MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, or ISC.

GPL, AGPL, LGPL, and SSPL dependencies are **forbidden**.
`cargo deny check` must pass before merging any PR.

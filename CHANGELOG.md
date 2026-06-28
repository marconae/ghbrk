# Changelog

## [0.2.0]

### Added
- Dependency license compliance checks cover MIT, Apache-2.0, and Unicode-3.0.

## [0.1.0]

### Added
- Privilege-separated Unix daemon (`ghbrk daemon`) holding your SSH key and GitHub token — policy-gated, credential-injected `git`/`gh` operations streamed over a Unix socket, agents never see the credentials.
- Role-based YAML policy (`read-only`/`write`/`maintain`/`admin`, org/repo/branch scoped); `ghbrk explain` previews decisions, `ghbrk allow` manages allow-lists, `ghbrk doctor` runs health checks.
- Per-operation SSH agent escrow and privilege-dropped execution.
- Append-only audit log of every policy decision.
- `install.sh`: systemd service install, with optional Claude Code/Codex agent wiring.

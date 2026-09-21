# Changelog

## [0.3.2]

### Fixed
- `credentials.rs` unit tests: two tests mutated the process-global `HOME` environment variable without restoring it, leaking the change into later tests in the same run and causing an intermittent flaky failure. All three affected tests now save and restore the original value.

### Changed
- Comments across the daemon and CLI rewritten for clarity (plain, active-voice sentences); no behavior change.
- Consolidated three internal constants (`REQUIRED_MODE`, `PERMISSION_MASK`, `CLIENT_GROUP_NAME`) that were each defined twice, to one source per constant.
- Docker integration harness: centralized the `mock-github` readiness wait behind one helper instead of repeating it at each of the six call sites that need it.

## [0.3.1]

### Fixed
- Docker integration harness: `mock-github` readiness is now polled before use instead of assumed right after the `devenv` container responds, removing an intermittent TLS-connect race on cold CI runners.
- Release pipeline only publishes once lint and the full test suite (including the Docker harness) pass, instead of racing an independent CI run.

## [0.3.0]

### Added
- Release pipeline generates a third-party license notice file as part of every tagged build.

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

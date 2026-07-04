# Changelog

## [Unreleased]

## [1.1.3]

### Fixed
- `/etc/ghbrk` whitelisted in the systemd unit's `ReadWritePaths` — `sudo ghbrk allow` no longer fails with `Read-only file system (os error 30)` on stock Linux installs.

## [1.1.2]

### Added
- Talos Linux support in the deploy scripts.
- `ghbrk.md` agent instructions, wiring Claude and Codex on install.

### Fixed
- Actionable hint on read-only policy errors; dropped `PrivateTmp` from the systemd unit.
- Integration tests mount the local `ghbrk.md` instead of downloading it from a release tag.

## [1.1.0]

### Added
- `ghbrk allow`: manage per-repo allow-lists, with role-based policy evaluation (owner/member/guest).
- Project logo added to the README and repo assets.

### Changed
- **Policy file format** (breaking): now requires a `roles` section.
- Replaced the `ring` crate with `aws-lc-rs`; tightened the dependency license allow-list.

### Fixed
- `doctor`: corrected credential directory mode and policy error message; silenced the policy check for unprivileged users.

## [1.0.3]

### Fixed
- Release archive renamed from `ghbrk-<tag>-x86_64-unknown-linux-gnu.tar.gz` to `ghbrk-<tag>-x86_64-linux.tar.gz`.

## [1.0.2]

### Added
- Wire-framing integration tests covering partial-body delivery and oversized-frame rejection over real Unix socket pairs — scenarios the unit tests cannot exercise.

### Fixed
- Increased `wait_for_ssh` timeout (30 s → 120 s) and `wait_for_devenv` timeout (60 s → 120 s) in the integration harness to prevent intermittent failures on cold-cache or loaded cloud VMs.
- `cargo-about` v0.9 compatibility: removed obsolete `filter-noassertion` key and updated `clarify` block format in `about.toml`.
- `cargo-about` install: added `--features=cli` so the binary is available after install.

## [1.0.1]

### Added
- `ghbrk --version` / `ghbrk -V` prints the program version and exits zero.
- GitHub Actions CI workflow: lint (`fmt` + `clippy` + `cargo-deny`) → integration tests (`--test-threads=1`).
- GitHub Actions release workflow: `cargo-deny` gate → build `x86_64-linux-musl` binary → strip → `cargo-about` third-party license file → tar.gz → GitHub Release.

## [1.0.0]

### Security
- **SSH agent escrow**: `id_rsa` is now `0600 ghbrk:ghbrk` — readable only by the daemon. For SSH operations the daemon spawns a per-operation `ssh-agent`, loads the key via `ssh-add` with a 30-second TTL, and passes only `SSH_AUTH_SOCK` to the git child. Key bytes never enter the calling user's address space.
- **Agent proxy socket**: an in-process proxy (`proxy.sock`) forwards connections to the real `agent.sock` inside the daemon process, working around OpenSSH 10+'s peer-uid check that rejects connections from a privilege-dropped child.
- **Executor privilege drop**: git/gh children run as the requesting user's uid/gid (via `setgroups` → `setresgid` → `setresuid` in a `pre_exec` closure). Eliminates the manual `chmod o+x ~` setup step.

### Fixed
- `chdir` now runs after `setresuid` in the forked child so 0700-mode working directories owned by the peer user are accessible.
- Cross-user `git push`: injected `safe.directory=*` env var for broker-spawned git processes; `ProtectHome=read-only` in the systemd unit.
- `credentials/` directory mode set to `0700 ghbrk:ghbrk`; `provision-user.sh` creates per-user subdirs at `0750`.

## [0.5.0]

### Changed
- **Explicit privilege gateway** (breaking): replaced the transparent `git`/`gh` PATH-interception shim with explicit `ghbrk git <remote-op>` / `ghbrk gh <subcommand>` invocations. Agents use plain `git`/`gh` for local/read-only work and `ghbrk git`/`ghbrk gh` only for operations that leave the machine.
- `ghbrk check` superseded by `ghbrk doctor`; new `ghbrk explain <cmd>` and `ghbrk policy <org>/<repo>` subcommands added.

### Removed
- `git`/`gh` symlink creation from `install.sh`.
- `/etc/ghbrk/config.yaml` (`real_git`/`real_gh` shim config).
- `src/shim.rs`, `src/passthrough.rs`, `src/config.rs`, `src/cmd/check.rs`, `src/cmd/shim.rs`.

## [0.4.x]

### Added
- `ghbrk check`: verifies SSH key, token, and GitHub API reachability by delegating credential inspection to the broker (fixes EACCES for normal-user callers).
- `gh api <path>` routed through the broker as a policy-gated, credential-injected `gh_api_read` operation. Non-GET methods are rejected at the resolver.
- Docker-based integration harness with a mock GitHub HTTPS API.

### Fixed
- Resolver pre-resolves git context in the shim (running as the invoking user) and forwards URL/branch hints to the broker, bypassing the broker's inability to traverse 0700 home directories.
- All `gh` invocations (including passthrough commands) now receive `GH_TOKEN` injection and `HOME` forwarding from the daemon.
- Mutex poison cascade and container name conflict in the integration harness.

## [0.3.x]

### Added
- `git pull` brokered as a distinct `Operation::Pull`, independently configurable from `fetch`.
- `install.sh` creates `/usr/local/bin/git` and `/usr/local/bin/gh` symlinks idempotently.
- Silent EACCES passthrough: when the broker socket is permission-denied, the shim execs the real binary silently (ENOENT/ECONNREFUSED remain fail-closed).

### Fixed
- Socket file now inherits the correct group (`ghbrk-clients`) by setting `Group=ghbrk-clients` in the unit, removing the runtime `chown` that failed due to group membership.
- `install.sh` wires `ghbrk` and `$SUDO_USER` into `ghbrk-clients` automatically and enables/restarts the service.
- Socket directory now created via `tmpfiles.d` (host-namespace `/run/ghbrk`) instead of `RuntimeDirectory=`, which was invisible to host-namespace shim processes.
- `GHBRK_SOCKET` aligned to `/run/ghbrk/broker.sock`.

## [0.2.0]

### Added
- Local-command passthrough: `git status`, `git add`, etc. exec the real binary without broker contact.
- Optional `/etc/ghbrk/config.yaml` to configure real `git`/`gh` binary paths (`real_git`, `real_gh`).

## [0.1.0]

### Added
- Initial release: privilege-separated Unix daemon (`ghbrk daemon`) holding SSH key and GitHub token on behalf of AI agents.
- Length-prefixed JSON wire protocol over a Unix stream socket with real-time stdout/stderr streaming.
- First-match-wins YAML policy engine with org/repo/branch/operation granularity.
- Brokered operations: `git push`, `git fetch`, `git clone`; `gh pr`, `gh issue`, `gh release` action groups.
- `SO_PEERCRED` identity verification; append-only audit log.
- `systemd` service unit with hardening directives; `install.sh` deployment script.
- Docker-based end-to-end integration harness with a mock SSH git server.

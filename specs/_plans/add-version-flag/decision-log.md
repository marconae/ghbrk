# Decision Log: add-version-flag

Date: 2026-05-31

## Interview

No clarifying interview was conducted. The feature is unambiguous: clap derive supports it directly via `#[command(version)]` on the `Cli` struct, and the desired behaviour (`ghbrk --version` prints `ghbrk <version>` and exits 0) is fully specified by the user intent.

## Design Decisions

### [1] Use clap's built-in `version` support rather than a hand-rolled flag

- **Decision:** Add `version` to the `#[command(...)]` attribute on the `Cli` struct so clap auto-generates `--version`/`-V`, reading `CARGO_PKG_VERSION` at compile time.
- **Alternatives:** Manually declare a `-V/--version` arg and print a version string; rejected because it duplicates clap behaviour, risks drift from `CARGO_PKG_VERSION`, and adds maintenance burden.
- **Rationale:** Idiomatic, zero extra dependencies, single source of truth for the version, and consistent with the existing clap-only dispatch model documented in infra/cli-dispatch.
- **Promotes to ADR:** no

### [2] Defer the version bump to a post-implementation task

- **Decision:** Bump `Cargo.toml` from `1.0.0` to `1.0.1` as the final task, after the feature and tests pass.
- **Alternatives:** Bump the version up front; rejected so the manual-test "before bump" expectation (`ghbrk 1.0.0`) and the post-bump expectation (`ghbrk 1.0.1`) both stay verifiable in order.
- **Rationale:** Adding a CLI flag is a backward-compatible, patch-level change under semver.
- **Promotes to ADR:** no

## Review Findings

<!-- Populated by speq-implement after code review. -->

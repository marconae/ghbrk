# Plan: add-version-flag

## Summary

Add a `--version` / `-V` flag to the `ghbrk` binary so that `ghbrk --version` prints the program name and crate version (e.g. `ghbrk 1.0.0`) and exits zero, using clap's built-in version support which reads `CARGO_PKG_VERSION` at compile time.

## Design

Small fix — no ADR required. The change is a single clap derive attribute. clap auto-generates the `--version`/`-V` handler, prints `<name> <CARGO_PKG_VERSION>` to stdout, and exits with status 0. Adding `version` to `#[command(...)]` on the `Cli` struct is the idiomatic clap-derive approach; no manual argument parsing or version string construction is introduced.

## Features

| Feature | Status | Spec |
|---------|--------|------|
| infra/cli-dispatch | CHANGED | `infra/cli-dispatch/spec.md` |

## Implementation Tasks

1. Add `version` to the `#[command(...)]` attribute on the `Cli` struct in `src/main.rs` (i.e. `#[command(name = "ghbrk", version, about = "Privilege-separated git/gh broker")]`).
2. Add an integration test to `tests/cli_dispatch.rs` (e.g. `version_flag_prints_version_and_exits_zero`) that runs `ghbrk --version`, asserts the process exits with status 0, and asserts stdout contains `ghbrk`.
3. After implementation and tests pass, bump the package version in `Cargo.toml` from `1.0.0` to `1.0.1` (patch-level change).

## Parallelization

None — tasks are sequential. Task 2 depends on Task 1; Task 3 follows verification.

## Verification

### Scenario Coverage

| Scenario | Test Type | Test Location | Test Name |
|----------|-----------|---------------|-----------|
| ghbrk --version prints version and exits zero | Integration | `tests/cli_dispatch.rs` | `version_flag_prints_version_and_exits_zero` |

### Manual Testing

| Feature | Command | Expected Output |
|---------|---------|-----------------|
| infra/cli-dispatch | `cargo run --release -- --version` | Prints `ghbrk 1.0.1` (`1.0.0` before the version bump) to stdout and exits 0 |
| infra/cli-dispatch | `cargo run --release -- -V` | Same as `--version`: prints `ghbrk <version>` and exits 0 |

### Checklist

| Step | Command | Expected |
|------|---------|----------|
| Build | `cargo build --release` | Exit 0 |
| Test | `cargo test` | 0 failures |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | 0 errors/warnings |
| Format | `cargo fmt --check` | No changes |
| Dependency audit | `cargo deny check` | Pass (no new dependencies) |

# Tasks: add-version-flag

## Phase 2: Implementation

- [x] 2.1 Add `version` to `#[command(...)]` on `Cli` struct in `src/main.rs`
- [x] 2.2 Add integration test `version_flag_prints_version_and_exits_zero` in `tests/cli_dispatch.rs`

## Phase 3: Verification

- [x] 3.1 Build (`cargo build --release`)
- [x] 3.2 Test suite (`cargo test`)
- [x] 3.3 Lint (`cargo clippy --all-targets --all-features -- -D warnings`)
- [x] 3.4 Format check (`cargo fmt --check`)
- [x] 3.5 Dependency audit (`cargo deny check`)
- [x] 3.6 Manual: `ghbrk --version` and `ghbrk -V` print version and exit 0

## Phase 4: Version bump

- [x] 4.1 Bump `Cargo.toml` version 1.0.0 → 1.0.1 and rebuild

# Tasks: fix-harness-failures

## Phase 2: Implementation (Group A — single file, sequential)
- [x] A.1 Replace all 9 `GLOBAL_LOCK.lock().unwrap()` with `unwrap_or_else(|e| e.into_inner())` in `tests/integration/harness.rs`
- [x] A.2 Add `docker rm -f` for each named container in `start_compose()` in `tests/integration/harness.rs`

## Phase 3: Verification
- [ ] V.1 `cargo build --tests` exits 0
- [ ] V.2 `cargo test --test harness -- --test-threads=1` — 9 passed, 0 failed
- [ ] V.3 `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] V.4 `cargo fmt --check` exits 0

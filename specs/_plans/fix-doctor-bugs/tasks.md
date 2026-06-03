# Tasks: fix-doctor-bugs

## Phase 2: Implementation

- [x] 2.1 Fix `install-credentials.sh`: change `-m 0750` to `-m 0700` on line 57
- [x] 2.2 Fix `check_policy` in `src/cmd/doctor.rs`: distinguish `PermissionDenied` from `NotFound`; add unit test for the `PermissionDenied` case

## Phase 2b: Review Fixes

- [x] 2.3 Strengthen `policy_permission_denied_returns_false` test: refactor `check_policy` to extract a pure `policy_status(path) -> (bool, String)` helper; test calls the helper directly and asserts both `!ok` AND message contains `"Policy: ERROR"` and does not contain `"MISSING"`

## Phase 3: Verification

- [x] 3.1 `cargo build --release` → exit 0
- [x] 3.2 `cargo test -p ghbrk` → 0 failures
- [x] 3.3 `cargo clippy -- -D warnings` → 0 errors

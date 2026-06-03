# Tasks: fix-policy-permission-verdict

## Phase 2: Implementation

- [x] 2.1 Change `policy_status` return type from `(bool, String)` to `(PermissionVerdict, String)`; map `PermissionDenied` → `Warning`; update `check_policy` and `run()` accordingly; update all tests

## Phase 2b: Review Fixes

- [x] 2.2 Remove `print_policy_permissions`; replace its 3 call-sites in `check_policy_permissions` with `print_permission_verdict("Policy permissions", &verdict)` (DRY / dead code)
- [x] 2.3 Give each `Error` arm in `policy_status` a distinct detail string: `"missing"` for `NotFound`, the I/O error text for other I/O errors, `"invalid"` for parse errors
- [x] 2.4 Assert `msg.contains("Policy: MISSING")` in `policy_missing_file_reports_missing` test
- [x] 2.5 Fix `Policy: WARNING` message format to `format!("Policy: WARNING ({}): not readable by current user; daemon validates on startup", path.display())` (drop redundant OS error from printed message)

## Phase 2c: UX Fix

- [x] 2.6 When `PermissionDenied` on the policy file, `check_policy` must print nothing and return `PermissionVerdict::Ok` — the `Policy permissions:` check already covers this case and there is nothing the user can act on

## Phase 3: Verification

- [x] 3.1 `cargo build --release` → exit 0
- [x] 3.2 `cargo test -p ghbrk` → 0 failures
- [x] 3.3 `cargo clippy -- -D warnings` → 0 errors

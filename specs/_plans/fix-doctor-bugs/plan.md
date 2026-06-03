# Plan: fix-doctor-bugs

## Problem

`ghbrk doctor` exits non-zero on a correctly installed system due to two bugs
found during post-release verification.

## Bug 1 — credential dir created with wrong mode

**File:** `deploy/linux/install-credentials.sh`, line 57
**Symptom:** `Credential dir permissions: ERROR mode 0o750 … (expected 0o700)`
**Root cause:** `install -d -m 0750` — the group execute (traversal) bit is set.
  The doctor's `CREDENTIAL_DIR_EXPECTED_MODE` constant is `0o700`.
**Fix:** Change `-m 0750` to `-m 0700`.

## Bug 2 — `check_policy` conflates PermissionDenied with NotFound

**File:** `src/cmd/doctor.rs`, function `check_policy` (around line 504)
**Symptom:** `Policy: MISSING (/etc/ghbrk/policy.yaml: Permission denied (os error 13))`
  The file exists but is not readable by unprivileged users; the doctor says MISSING.
**Root cause:** The catch-all `Err(err)` arm (after the `NotFound` guard) also
  prints `"Policy: MISSING"`. `PermissionDenied` and other unexpected errors
  should be `"Policy: ERROR"`.
**Fix:** Add a second guard for `NotFound` only; the catch-all becomes `Policy: ERROR`.
  Also add a unit test covering the PermissionDenied path.

## Affected specs

- `cli/doctor` — "Policy file parses cleanly reports OK" (must pass after fix)
- `cli/doctor-permissions` — credential dir mode check

## Parallelization

Both bugs are in separate files and can be fixed in a single agent pass.

## Verification

### Checklist
- `cargo build --release` → exit 0
- `cargo test -p ghbrk` → 0 failures
- `cargo clippy -- -D warnings` → 0 errors

### Scenario Coverage
- Bug 1: credential dir created at `0700`, doctor reports no error
- Bug 2: `check_policy` returns `false` and prints `Policy: ERROR` on `PermissionDenied`

### Manual Testing
- Run `ghbrk doctor` after rebuilding; verify exit 0 and all OK lines

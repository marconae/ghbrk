# Plan: fix-policy-permission-verdict

## Problem

`ghbrk doctor` exits non-zero on a healthy system because `check_policy`
returns `false` (→ `PermissionVerdict::Error`) when the policy file is not
readable by the invoking user.

The policy file is intentionally `0600 ghbrk:ghbrk`; unprivileged users
will always get `PermissionDenied`. The daemon validates the policy at startup.
The `Policy permissions: OK` line already confirms the mode and owner are
correct. Treating `PermissionDenied` as an error is wrong — it should be a
`Warning`, which does not affect the exit code.

## Root cause

`policy_status` in `src/cmd/doctor.rs` returns `(bool, String)`. It returns
`false` for all non-NotFound errors, including `PermissionDenied`. The `bool`
is fed into `verdict_from_success` which maps `false` → `PermissionVerdict::Error`.

There is no way to express a Warning through the current `(bool, String)`
return type.

## Fix

Change `policy_status` to return `(PermissionVerdict, String)` so it can
signal a Warning:

- `NotFound` → `(PermissionVerdict::Error, "Policy: MISSING (…)")`
- `PermissionDenied` → `(PermissionVerdict::Warning, "Policy: WARNING (not readable by current user; daemon validates on startup)")`
- Other `Err` → `(PermissionVerdict::Error, "Policy: ERROR (…)")`
- Parse error → `(PermissionVerdict::Error, "Policy: INVALID (…)")`
- Success → `(PermissionVerdict::Ok, "Policy: OK")`

Update `check_policy` to print the message and return the verdict directly
(drop the `verdict_from_success` call for this check in `run()`).

Update printing: use the `print_permission_verdict` helper (already exists)
or inline `println!("{msg}")`.

## Affected files

- `src/cmd/doctor.rs` — `policy_status`, `check_policy`, `run()`, tests

## Affected specs

- `cli/doctor` — "All checks passing exits zero" scenario

## Parallelization

Single file; one agent pass.

## Verification

### Checklist
- `cargo build --release` → exit 0
- `cargo test -p ghbrk` → 0 failures
- `cargo clippy -- -D warnings` → 0 errors

### Scenario Coverage
- `PermissionDenied` → verdict is `Warning`, message contains `"Policy: WARNING"`, exit 0
- `NotFound` → verdict is `Error`, message contains `"Policy: MISSING"`, exit 1
- Other error → verdict is `Error`, message contains `"Policy: ERROR"`, exit 1
- Valid file → verdict is `Ok`, message is `"Policy: OK"`, exit 0

### Manual Testing
- Run `ghbrk doctor` as unprivileged user → exit 0; `Policy: WARNING` line visible

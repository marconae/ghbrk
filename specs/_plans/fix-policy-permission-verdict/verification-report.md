# Verification Report: fix-policy-permission-verdict

**Verdict: PASS**

## Summary

`ghbrk doctor` now exits 0 on a correctly installed system. The policy parse
check is silently skipped when the file is not readable by the invoking user
(expected: `/etc/ghbrk/policy.yaml` is `0600 ghbrk:ghbrk`). The
`Policy permissions: OK` line remains as the sole policy-related output,
which is sufficient — it confirms the file exists and is correctly locked down.

## Changes

| Task | Change |
|------|--------|
| 2.1 | `policy_status` returns `(PermissionVerdict, String)` instead of `(bool, String)` |
| 2.2 | Removed duplicate `print_policy_permissions`; inlined via `print_permission_verdict` |
| 2.3 | Distinct `Error` detail strings per arm: `"missing"`, `"invalid"`, I/O error text |
| 2.4 | `policy_missing_file_reports_missing` test asserts message contains `"Policy: MISSING"` |
| 2.5 | (superseded by 2.6 — Warning arm removed entirely) |
| 2.6 | `PermissionDenied` → `(Ok, "")` — silent, no output line printed |

## Manual verification

```
$ ghbrk doctor
Daemon: OK
Credential dir permissions: OK
SSH key permissions: OK
Token permissions: OK
Credentials: OK
Policy permissions: OK
Config dir permissions: OK
Socket permissions: OK
exit: 0
```

No `Policy:` parse line shown to unprivileged users. All checks OK. Exit 0.

## Automated checks

| Check | Result |
|-------|--------|
| `cargo build --release` | ✅ exit 0 |
| `cargo test -p ghbrk` | ✅ 0 failures |
| `cargo clippy -- -D warnings` | ✅ 0 errors |

## Scenario coverage

| Scenario | Status |
|----------|--------|
| `PermissionDenied` → silent Ok, exit 0 | ✅ `policy_permission_denied_is_silent` |
| `NotFound` → `Policy: MISSING`, exit 1 | ✅ `policy_missing_file_reports_missing` |
| Parse error → `Policy: INVALID`, exit 1 | ✅ `policy_invalid_yaml_reports_invalid` |
| Valid file → `Policy: OK`, exit 0 | ✅ `policy_valid_file_reports_ok` |

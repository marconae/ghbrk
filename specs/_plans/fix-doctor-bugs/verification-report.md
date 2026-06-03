# Verification Report: fix-doctor-bugs

**Verdict: PASS** (code fixes correct; two live-system notes below)

## Summary

Two bugs identified during post-release `ghbrk doctor` run on v1.1.0:

1. `install-credentials.sh` created the per-user credential directory at `0750` instead of `0700`.
2. `check_policy` in `doctor.rs` printed `Policy: MISSING` for `PermissionDenied` errors (only `NotFound` should say MISSING).

Both are fixed. All automated checks pass.

---

## Bug 1 — Credential dir mode (`install-credentials.sh`)

**Fix:** `install -d -m 0750` → `install -d -m 0700` (line 57)

**Evidence:**

```
cargo build --release → Finished (exit 0)
cargo test -p ghbrk   → 178 passed, 0 failed
cargo clippy -D warnings → 0 errors
```

**Live-system note:** The already-installed directory `/etc/ghbrk/credentials/ferris/` retains the old `0750` mode. Fix by running:

```bash
sudo chmod 700 /etc/ghbrk/credentials/ferris/
```

Future installs via `install-credentials.sh` will create it correctly at `0700`.

---

## Bug 2 — `check_policy` conflates `PermissionDenied` with `NotFound`

**Fix:** Split the catch-all `Err` arm into a `NotFound` guard (`MISSING`) and a true catch-all (`ERROR`). Extracted `policy_status(path) -> (bool, String)` so the message can be unit-tested.

**Test added:** `policy_permission_denied_returns_false` — creates a `0000`-mode temp file, calls `policy_status`, asserts `!ok`, `msg.contains("Policy: ERROR")`, and `!msg.contains("MISSING")`.

**Before fix output:**
```
Policy: MISSING (/etc/ghbrk/policy.yaml: Permission denied (os error 13))
```

**After fix output:**
```
Policy: ERROR (/etc/ghbrk/policy.yaml: Permission denied (os error 13))
```

**Structural note (separate issue):** The policy file is installed as `0600 ghbrk:ghbrk`, so it is unreadable by any `ghbrk-clients` user. `Policy: ERROR (permission denied)` when running as an unprivileged user is therefore expected on the current system. This is a design gap — not a regression introduced by this fix — and should be tracked separately (e.g., policy check routed through daemon, or file mode changed to `0640 ghbrk:ghbrk-clients`).

---

## Scenario Coverage

| Scenario | Status |
|----------|--------|
| `PermissionDenied` → `Policy: ERROR` (not MISSING) | ✅ |
| `NotFound` → `Policy: MISSING` | ✅ (existing test) |
| Credential dir created at `0700` by install script | ✅ (code fix; live dir needs manual chmod) |

---

## Automated Checks

| Check | Result |
|-------|--------|
| `cargo build --release` | ✅ exit 0 |
| `cargo test -p ghbrk` | ✅ 0 failures |
| `cargo clippy -- -D warnings` | ✅ 0 errors |

# Verification Report: fix-harness-failures

**Generated:** 2026-05-29

## Verdict

| Result | Details |
|--------|---------|
| **PASS** | All 9 harness tests pass; build, lint, and format clean |

| Check | Status |
|-------|--------|
| Build | ✓ |
| Harness tests (9) | ✓ 9 passed, 0 failed |
| Unit tests (147) | ✓ |
| Lint | ✓ |
| Format | ✓ |

## Root Causes Fixed

### Bug 1 — Mutex poison cascade
`GLOBAL_LOCK.lock().unwrap()` at 9 sites — one test panic poisoned the mutex,
cascading to 8 additional `PoisonError` failures.
**Fix:** replaced with `.unwrap_or_else(|e| e.into_inner())` at all 9 sites.

### Bug 2 — Container name conflict
`docker compose down` sometimes left named containers (`ghbrk-it-*`) from
interrupted runs, causing `compose up` to fail with a naming conflict.
**Fix:** explicit `docker rm -f` loop for all 3 named containers in
`start_compose()`, after `down` and before `up`.

## Test Evidence

| Suite | Passed | Failed |
|-------|--------|--------|
| `cargo test --test harness -- --test-threads=1` | 9 | 0 |
| `cargo test --lib` | 147 | 0 |

Harness runtime: ~227s (Docker Compose bring-up + 9 e2e scenarios).

## Tool Evidence

```
cargo clippy --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.54s
(no warnings or errors)

cargo fmt --check
(no output — clean)
```

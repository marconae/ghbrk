# Plan: fix-harness-failures

## Problem

`cargo test --test harness` reports 9 failures. All 9 pass when run with
`--test-threads=1` on a clean Docker state.  The failures stem from two bugs
in `tests/integration/harness.rs`.

## Root Cause

### Bug 1 — Mutex poison cascade (primary cause of 8/9 failures)

Every test acquires `GLOBAL_LOCK` via `GLOBAL_LOCK.lock().unwrap()`.  When the
**first** test panics (for any reason — including Bug 2), Rust marks the
`Mutex` as *poisoned*.  All subsequent `.lock().unwrap()` calls panic with
`PoisonError`, producing 8 additional test failures that have nothing to do
with the actual failure.

Fix: replace every `.lock().unwrap()` on `GLOBAL_LOCK` with
`.lock().unwrap_or_else(|e| e.into_inner())`.  A `Mutex<()>` holds no data
that can be corrupted, so recovering the guard from a poisoned mutex is always
safe.

### Bug 2 — Container name conflict in `start_compose()` (root of first failure)

`start_compose()` runs `docker compose down -v --remove-orphans` as a
best-effort cleanup, then `docker compose up -d --build`.  When a previous
harness run was interrupted (e.g. the process was killed, or a background
`cargo test` invocation left containers running), the named containers
(`ghbrk-it-git-server`, `ghbrk-it-mock-github`, `ghbrk-it-devenv`) may
persist in Docker's state even though `compose down` ran.  This causes
`compose up` to fail with:

```
Error response from daemon: Conflict. The container name "/ghbrk-it-git-server"
is already in use by container "...".
```

The `compose down` is silently ignored (`let _ = ...`), and if it does not
remove every container (e.g. because the container was started under a
different Docker Compose project name, or Docker's internal state is stale),
the subsequent `compose up` fails.

Fix: after `compose down`, explicitly `docker rm -f` each named container
before calling `compose up`, so any survivor is force-removed.

## Changes

All changes are confined to `tests/integration/harness.rs`.

### 1. Fix poison cascade — all GLOBAL_LOCK sites

Replace every:
```rust
let _lock = GLOBAL_LOCK.lock().unwrap();
```
with:
```rust
let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
```

There are 9 call sites (lines 546, 565, 605, 654, 716, 1039, 1071, 1108, 1136).

### 2. Fix container cleanup in `start_compose()`

After the existing `docker compose down` call and before `docker compose up`,
force-remove each named container:

```rust
for name in &[CONTAINER_NAME, DEVENV_CONTAINER, "ghbrk-it-mock-github"] {
    let _ = Command::new("docker")
        .args(["rm", "-f", name])
        .output();
}
```

Note: `CONTAINER_NAME = "ghbrk-it-git-server"`, `DEVENV_CONTAINER = "ghbrk-it-devenv"`.
The mock-github container is unnamed in the constants — use the literal string.

## Verification

### Checklist
- [ ] `cargo build --tests` exits 0
- [ ] `cargo test --test harness -- --test-threads=1` exits 0 with 9 passed
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0

### Scenario Coverage
- Running `cargo test --test harness` (no `--test-threads=1`) must not cascade
  9 failures from a single panic — individual test failures must remain isolated

### Manual Testing
```bash
cargo test --test harness -- --test-threads=1
```

## Parallelization

### Group A (single agent — both changes in one file)
- [ ] A.1 Fix all 9 `GLOBAL_LOCK.lock().unwrap()` call sites
- [ ] A.2 Add explicit `docker rm -f` cleanup to `start_compose()`
- [ ] A.3 Verify: `cargo test --test harness -- --test-threads=1` passes

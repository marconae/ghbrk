# Tasks: change-explicit-gateway

## Group A (protocol/dispatch foundation)
- [x] 1.1 Remove argv[0] symlink dispatch from `src/main.rs`; make dispatch clap-only [expert]
- [x] 1.2 Add `Doctor`, `Explain { args }`, `Policy { repo }` clap subcommands; remove `Check`
- [x] 1.3 Add `Tool::Explain` and `Tool::Policy` discriminants to `src/protocol.rs`

## Group B (removals)
- [x] 2.1 Delete `src/shim.rs`, `src/passthrough.rs`, `src/cmd/shim.rs`, `src/config.rs`; rewire broker-relay transport in `cmd/git.rs`/`cmd/gh.rs` [expert]
- [x] 2.2 Remove `git`/`gh` symlink creation from `deploy/linux/install.sh`

## Group C (gateway + commands)
- [x] 3.1 Rewrite `src/cmd/git.rs`: reject local-only subcommands with guidance error; relay only remote ops [expert]
- [x] 3.2 Simplify `src/cmd/gh.rs` to relay all gh invocations to broker (minus deleted deps)
- [x] 3.3 Implement `src/cmd/doctor.rs`: daemon-reachability, credential check, policy-parse check
- [x] 3.4 Implement `src/cmd/policy.rs`: send `Tool::Policy { repo }`; print allowed/forbidden ops

## Group D (broker query handlers)
- [x] 4.1 Implement `src/cmd/explain.rs`: send `Tool::Explain`; broker resolves + evaluates without executing [expert]
- [x] 4.2 Implement broker handling for `Tool::Explain`: resolve, evaluate policy, report, MUST NOT execute [expert]
- [x] 4.3 Implement broker handling for `Tool::Policy`: evaluate every op vocab for caller/repo, stream grouped result [expert]
- [x] 4.4 Add broker defence-in-depth: deny any local-only git subcommand that bypasses gateway filter

## Group E (tests)
- [x] 5.1 Remove obsolete tests; migrate resolver tests to daemon resolver path
- [x] 5.2 Write integration tests for new scenarios (cli-dispatch, doctor, explain, policy-query, broker deny, wire-protocol)

## Group F (docs + release)
- [x] 6.1 Update `specs/mission.md` prose references to transparent shim
- [x] 6.2 Update `README.md`: replace transparent shim docs with explicit gateway model
- [x] 6.3 Bump crate version in `Cargo.toml` from 0.4.2 to reflect breaking architecture change

## Verification
- [ ] V.1 Build: `cargo build --release` → exit 0
- [ ] V.2 Test: `cargo test` → 0 failures
- [ ] V.3 Lint: `cargo clippy --all-targets --all-features -- -D warnings` → 0 errors
- [ ] V.4 Format: `cargo fmt --check` → no changes
- [ ] V.5 License: `cargo deny check` → exit 0
- [ ] V.6 Integration tests via Docker
- [ ] V.7 Manual scenario checks

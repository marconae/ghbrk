# Tasks: add-release-lifecycle-ops

## Phase 2: Implementation (Group A)
- [x] 2.1 policy.rs — operation vocabulary: add ReleaseDelete, ReleaseEdit, ReleaseUpload, ReleaseDeleteAsset, ReleaseList, ReleaseView, ReleaseDownload to Operation enum; wire tag()/from_tag()

## Phase 2: Implementation (Group B)
- [x] 2.2 policy.rs — built-in role restructure: read_only + 3 read release ops, maintain = write + 5 mutating release ops, admin = maintain.clone() [expert]
- [x] 2.3 resolver.rs — classify_gh arms for delete, delete-asset, download, edit, list, upload, view
- [x] 2.4 broker.rs — gh_is_broker_op arms for the same 7 verbs + doc-comment update

## Phase 2: Implementation (Group C)
- [x] 2.5 docs/policy.md — operations reference table + gh command-routing table
- [x] 2.6 config/policy.example.yaml — vocabulary comment + maintain role line + admin/read-only description updates

## Phase 2: Implementation (Group D)
- [x] 2.7 Tests — resolver: per-verb classify_gh unit tests + tests/resolver.rs integration test (delivered as part of 2.3)
- [x] 2.8 Tests — policy: unit tests for new ops/roles, update builtin_roles_available_without_declaration (delivered as part of 2.2)
- [x] 2.9 Tests — broker + explain: tests/broker_server.rs and tests/explain.rs integration tests

## Phase 4: Code Review
- [x] 4.1 Review all changed files (code-reviewer)

## Phase 4: Fix Review Findings
- [x] 4.2 broker.rs — add "maintain" to BUILTIN_ROLE_NAMES + fix doc comment; regression test for `ghbrk allow ... maintain` [expert]
- [x] 4.3 tests/broker_server.rs — fix PATH mutation race in install_stub_gh; dedupe current_test_user vs inline id -un logic

## Phase 5: Verification
- [x] 5.1 cargo build --release
- [x] 5.2 cargo test
- [x] 5.3 cargo clippy --all-targets --all-features -- -D warnings
- [x] 5.4 cargo fmt --check
- [x] 5.5 cargo deny check
- [x] 5.6 Scenario coverage audit
- [x] 5.7 Manual testing (explain, policy, broker gh release delete)

## Phase 6: Verification Report
- [x] 6.1 Generate verification-report.md

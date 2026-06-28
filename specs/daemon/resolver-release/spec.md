# Feature: resolver-release

Maps every `gh release <verb>` subcommand to its own release operation, so the policy engine can gate the full release lifecycle surface at GitHub-style, per-verb granularity. Split from `daemon/resolver`; see `daemon/resolver-gh` for `gh pr`/`gh issue`/`gh api` resolution.

## Background

The resolver runs inside the broker daemon (`src/broker.rs::resolve_request`), not in the calling process. Classification (`src/resolver.rs::classify_gh`) keys only on the first two non-flag positional tokens (`group` and `action`) and deliberately ignores all flags, values, tags, and trailing asset paths. The `gh release` group is fully classified: each `release <verb>` subcommand maps to its own release operation (`release_create`, `release_delete`, `release_edit`, `release_upload`, `release_delete_asset`, `release_list`, `release_view`, `release_download`). The `delete-asset` verb is a single hyphenated token, not two positional words. Only GitHub URLs are recognised; other forges produce an error.

## Scenarios

### Scenario: Resolve gh release delete to release_delete

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *WHEN* the resolver processes `gh release delete v1.2.0 --yes`
* *THEN* the resolver MUST produce `{ op: release_delete, org: acme, repo: web, branch: None }`
* *AND* the resolver MUST ignore the tag argument and any flags during classification

### Scenario: Resolve gh release edit to release_edit

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `gh release edit v1.2.0 --title "New title" --target main`
* *THEN* the resolver MUST produce `{ op: release_edit, org: acme, repo: web, branch: None }`
* *AND* the resolver MUST NOT treat the `--target` value as a policy branch

### Scenario: Resolve gh release upload to release_upload

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *WHEN* the resolver processes `gh release upload v1.2.0 ./dist/app.tar.gz`
* *THEN* the resolver MUST produce `{ op: release_upload, org: acme, repo: web, branch: None }`
* *AND* the resolver MUST ignore the trailing asset path during classification

### Scenario: Resolve gh release delete-asset to release_delete_asset

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *WHEN* the resolver processes `gh release delete-asset v1.2.0 app.tar.gz --yes`
* *THEN* the resolver MUST produce `{ op: release_delete_asset, org: acme, repo: web, branch: None }`
* *AND* the resolver MUST treat `delete-asset` as a single subcommand token, not a `delete` verb with an `asset` operand

### Scenario: Resolve gh release list to release_list

* *GIVEN* `cwd` is a git repo with `remote.origin.url=https://github.com/acme/web.git`
* *WHEN* the resolver processes `gh release list`
* *THEN* the resolver MUST produce `{ op: release_list, org: acme, repo: web, branch: None }`

### Scenario: Resolve gh release view to release_view

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *WHEN* the resolver processes `gh release view v1.2.0`
* *THEN* the resolver MUST produce `{ op: release_view, org: acme, repo: web, branch: None }`

### Scenario: Resolve gh release download to release_download

* *GIVEN* `cwd` is a git repo with `remote.origin.url=git@github.com:acme/web.git`
* *WHEN* the resolver processes `gh release download v1.2.0 --pattern "*.tar.gz"`
* *THEN* the resolver MUST produce `{ op: release_download, org: acme, repo: web, branch: None }`

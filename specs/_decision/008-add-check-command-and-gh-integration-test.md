# Decisions: add-check-command-and-gh-integration-test

## ADR: Route `gh api` through the broker instead of passthrough

**ID:** route-gh-api-through-broker
**Plan:** add-check-command-and-gh-integration-test
**Status:** Accepted

### Context

`gh api <path>` was treated as passthrough, allowing any agent to call arbitrary GitHub read endpoints using whatever token was in its own environment, bypassing the broker entirely and defeating the privilege-separation goal for the most common scripted GitHub access pattern. Operators had no mechanism to policy-gate or audit `gh api` calls.

### Decision

`gh api <path>` is classified as broker-mediated. A new `Operation::GhApiRead { path }` flows through the existing shim → resolver → policy → executor pipeline, policy-gated by a `gh_api_read` rule and credential-injected with `GH_TOKEN`.

### Options Considered

| Option | Verdict |
|--------|---------|
| Route `gh api` through the broker | Chosen — closes the largest bypass of broker mediation; makes API reads policy-gated and audited with the existing pipeline |
| Leave `gh api` as passthrough | Rejected — lets agents call arbitrary GitHub read endpoints with their own ambient token, bypassing broker mediation and audit entirely |

### Consequences

`gh api` calls are now policy-gated and recorded in the audit log. Agents that previously relied on ambient `GH_TOKEN` passthrough must have a `gh_api_read` rule in the policy. Write methods (`gh api -X POST/PATCH/DELETE`) and GraphQL remain default-denied; they require distinct operations introduced in a future plan.

## ADR: Evaluate `gh_api_read` on the user only (wildcard org/repo)

**ID:** gh-api-read-user-scoped-wildcard
**Plan:** add-check-command-and-gh-integration-test
**Status:** Accepted

### Context

The `gh api` path space is not uniformly repo-scoped. Paths like `/user`, `/rate_limit`, and `/orgs/acme` carry no repo component. Deriving org and repo from the path would be unreliable for the general case and inconsistent across path patterns.

### Decision

The resolver emits unset/wildcard org and repo for `gh_api_read`. Policy authorises it via a user-scoped rule with `org: "*"` and `repo: "*"`. Branch is ignored (`has_branch() == false`). Default-deny still applies when no rule grants the operation.

### Options Considered

| Option | Verdict |
|--------|---------|
| User-scoped wildcard org/repo | Chosen — correct and consistent across all API paths; keeps the model simple; default-deny still protects against unauthorised access |
| Parse org/repo out of the API path (for example `repos/acme/web`) | Rejected — API paths are not uniformly repo-scoped; parsing would be unreliable and inconsistent for paths like `/user` or `/rate_limit` |

### Consequences

Operators write a single user-scoped rule to authorise all `gh api` read calls for a user. Per-repo granularity for API reads is not available; it is deferred to a future plan if needed. The user-scoping model is consistent with how branch-less operations like `clone` and `fetch` are already handled.

## ADR: Mock GitHub API over TLS so `gh api` integration tests always run

**ID:** mock-github-api-over-tls-for-integration-tests
**Plan:** add-check-command-and-gh-integration-test
**Status:** Accepted

### Context

The `gh api` harness tests previously skipped gracefully when `GH_TOKEN` was absent, providing no proof of the broker path in a default environment (local dev, CI without a configured secret). The `gh` CLI enforces HTTPS even when `GH_HOST` points at a non-`github.com` host and does not skip certificate verification, so a plain HTTP mock is refused.

### Decision

A `mock-github` HTTPS service is added to the Docker compose stack, returning a fixed JSON body for `GET /api/v3/user`. The mock serves real TLS via a pre-generated, self-signed test CA and server certificate (CN/SAN `mock-github`) committed under `tests/integration/certs/`. The `devenv` image installs the CA into its system trust store at build time. Harness tests point the broker at `GH_HOST=mock-github` with a synthetic token and assert stdout contains `"login": "test-user"`. A curl-based TLS smoke test from `devenv` proves trust independently of the broker, and a missing-token case asserts non-zero exit. The tests no longer skip gracefully, they always run when Docker is available.

### Options Considered

| Option | Verdict |
|--------|---------|
| HTTPS mock with committed self-signed test CA | Chosen — exercises the real `gh api` → broker → credential-injection path; always provides proof; no real token; no network dependency |
| Graceful skip when `GH_TOKEN` absent | Rejected — provides no proof in a default/CI-without-secret environment (the original approach) |
| Plain HTTP mock | Rejected — `gh` enforces HTTPS even for non-github.com `GH_HOST` and refuses non-HTTPS connections |
| Transparent TLS interception (mitmproxy-style) | Rejected — heavier dependency and more moving parts than a fixed-response mock requires |

### Consequences

`gh api` tests always run and prove the broker path whenever Docker is available, with no real token and no network dependency. Self-signed certs scoped to the Docker test network carry no security risk. The committed certs (approximately 10-year validity) must be regenerated before they expire; the `openssl` commands are documented under `tests/integration/certs/` for rotation.

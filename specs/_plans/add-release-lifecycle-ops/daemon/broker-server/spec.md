# Feature: broker-server

In addition to the peer username, the broker now resolves the peer's full identity from `SO_PEERCRED` plus the password database: `uid`, primary `gid`, supplementary GIDs, and home directory. This identity is attached to the `ChildSpec` for every executing tool (brokered git and gh passthrough) so the executor can drop the child to the requesting user. Supplementary group lookup failure is non-fatal — the broker logs a warning and proceeds with the primary GID. All prior binding, peer-credential, policy, and concurrency behaviour is unchanged.

## Background

Before executing a `gh` invocation, the broker decides whether it is a broker-mediated operation (subject to resolve + policy via `src/broker.rs::gh_is_broker_op`) or an ungoverned passthrough. Passthrough invocations still receive `GH_TOKEN` injection but bypass resolve and policy. Every `gh release` lifecycle subcommand — `create`, `delete`, `edit`, `upload`, `delete-asset`, `list`, `view`, `download` — is a broker-mediated operation and MUST be policy-gated; `gh_is_broker_op` mirrors `classify_gh`'s release arms so real execution and policy evaluation agree.

## Scenarios

<!-- DELTA:NEW -->
### Scenario: Broker policy-gates gh release delete instead of passing it through

* *GIVEN* the broker is running and no policy rule grants the calling user `release_delete` on `acme/web`
* *AND* `cwd` is a clone of `acme/web`
* *WHEN* the user runs `ghbrk gh release delete v1.2.0 --yes`
* *THEN* the broker MUST route the invocation through resolve and policy as a broker-mediated operation, not passthrough
* *AND* the broker MUST deny the `release_delete` request on `acme/web` by default
* *AND* the broker MUST NOT execute `gh release delete`

### Scenario: Broker policy-gates the mutating gh release subcommands

* *GIVEN* the broker is running
* *WHEN* the broker receives a `gh release edit`, `gh release upload`, or `gh release delete-asset` invocation
* *THEN* `gh_is_broker_op` MUST report each as a broker-mediated operation
* *AND* the broker MUST route each through resolve and policy before any execution
* *AND* the broker MUST NOT fall through to ungoverned passthrough for any of them

### Scenario: Broker executes a policy-allowed gh release delete

* *GIVEN* a policy rule grants the calling user the `maintain` role on `acme/web`
* *AND* `cwd` is a clone of `acme/web`
* *WHEN* the user runs `ghbrk gh release delete v1.2.0 --yes`
* *THEN* the broker MUST evaluate the policy and obtain an `allow` decision for `release_delete`
* *AND* the broker MUST inject `GH_TOKEN` and execute the wrapped `gh release delete`
<!-- /DELTA:NEW -->

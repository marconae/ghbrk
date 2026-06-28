# Feature: policy-admin

Enforces broker-side privilege and validation for the `ghbrk allow` mutation request: only an effective-UID-0 caller may append a rule to the policy file, and operands are validated against the loaded vocabulary and roles before any write. Split from `daemon/broker-server`; see `cli/allow-command` for the client-facing subcommand behaviour.

## Background

The broker is the sole writer of `/etc/ghbrk/policy.yaml`. On an `allow` request it MUST verify the caller's `SO_PEERCRED` effective UID is `0` before mutating anything; an unprivileged caller is denied with the policy file left untouched and a deny entry written to the audit log. Operands (operations or a role name) are validated against the loaded policy vocabulary and roles before the file is touched — an unknown operand is rejected without mutation. On success the broker appends a validated allow rule, reloads the policy handle so subsequent connections see the new rule, writes an audit entry, and streams a confirmation followed by an `Exit { code: 0 }` frame.

## Scenarios

### Scenario: Broker enforces privilege for the allow request

* *GIVEN* the broker receives a `Request { tool: allow, ... }`
* *AND* the connecting peer's `SO_PEERCRED` reports an effective UID other than 0
* *WHEN* the broker processes the request
* *THEN* the broker MUST send a `Denied` frame indicating elevated privileges are required
* *AND* the broker MUST NOT write to the policy file
* *AND* the broker MUST write a deny entry to the audit log

### Scenario: Broker appends a rule and reloads on a privileged allow request

* *GIVEN* the broker receives a `Request { tool: allow, args: ["acme/web", "write"] }`
* *AND* the connecting peer's `SO_PEERCRED` reports effective UID 0
* *WHEN* the broker processes the request
* *THEN* the broker MUST append a validated allow rule to the policy file
* *AND* the broker MUST reload the policy handle so subsequent connections see the new rule
* *AND* the broker MUST write an allow entry to the audit log and stream a confirmation followed by an `Exit { code: 0 }` frame

### Scenario: Allow request validates operands before mutating the policy file

* *GIVEN* the broker receives a privileged `Request { tool: allow, args: ["acme/web", "frobnicate"] }`
* *WHEN* the broker validates the operands against the loaded policy vocabulary and roles
* *THEN* the broker MUST reject the request with a `Denied` frame mentioning `frobnicate`
* *AND* the broker MUST leave the policy file byte-for-byte unchanged

# Feature: audit-log

Writes one structured record per allow or deny decision so operators can later reconstruct who attempted which operation against which repo and what the broker decided.

## Background

The audit log is an append-only file at `/var/log/ghbrk/audit.log` (configurable via daemon flag). One record per line in JSON form. Records MUST include a wall-clock timestamp in RFC 3339, the caller username, the resolved tool, operation, org, repo, branch (if any), the decision (`allow` or `deny`), and on deny a `reason` string. Audit writes MUST NOT block request processing if disk is slow — best-effort buffered append is acceptable, but a flush on shutdown is required.

## Scenarios

### Scenario: Allow decision produces an allow record

* *GIVEN* the policy engine returned `allow` for `(alice, acme/web, push, main)`
* *WHEN* the daemon completes the decision phase
* *THEN* the audit log MUST contain a JSON line with `decision: "allow"`, `user: "alice"`, `op: "push"`, `org: "acme"`, `repo: "web"`, `branch: "main"`

### Scenario: Deny decision includes reason

* *GIVEN* the policy engine returned `deny` with reason `"no matching rule"`
* *WHEN* the daemon completes the decision phase
* *THEN* the audit log MUST contain a JSON line with `decision: "deny"`
* *AND* the same line MUST contain `reason: "no matching rule"`

### Scenario: Audit record carries timestamp

* *GIVEN* the daemon makes a decision at wall-clock time T
* *WHEN* the audit record is written
* *THEN* the record MUST include a `ts` field with an RFC 3339 timestamp within 1 second of T

### Scenario: Token value never appears in audit log

* *GIVEN* the request involved an HTTPS push using a stored token
* *WHEN* the audit record is written
* *THEN* the record MUST NOT contain the token contents

### Scenario: Audit log survives daemon restart

* *GIVEN* the daemon wrote 5 records, then was restarted
* *WHEN* the daemon starts again and writes a 6th record
* *THEN* the audit log file MUST contain all 6 records in order
* *AND* the daemon MUST NOT truncate the existing file

### Scenario: Audit log flushes on SIGTERM

* *GIVEN* the daemon has buffered audit records pending
* *WHEN* the daemon receives SIGTERM
* *THEN* the daemon MUST flush all buffered records to disk before exit

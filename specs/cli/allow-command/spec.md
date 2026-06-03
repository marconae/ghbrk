# Feature: allow-command

Provides a `ghbrk allow <org>/<repo> <operations-or-role> [--user <name>]` subcommand that lets a privileged caller grant repository permissions by appending an allow rule to `/etc/ghbrk/policy.yaml`, so administrators can adjust policy without hand-editing the file the broker owns.

## Background

`ghbrk allow` is a privileged mutation, not a query. The caller invokes it as `ghbrk allow <org>/<repo> <op> [<op>...]` to grant a list of operations, or `ghbrk allow <org>/<repo> <role>` to grant a named role (built-in or user-defined). The grant targets the calling Unix user by default; an optional `--user <name>` flag targets a different user. The CLI forwards the request to the broker over the existing Unix socket using a dedicated wire-protocol message. The broker is the sole writer of the policy file: it MUST verify that the caller is privileged (effective UID 0, i.e. invoked under `sudo`/root) via `SO_PEERCRED` before mutating anything. On success the broker appends a well-formed allow rule, hot-reloads the policy so the change takes effect immediately, writes an audit record, and confirms back to the caller. An unprivileged caller is denied with no change to the policy file. Operations and role names are validated against the loaded policy before the rule is written; an unknown operation or role is rejected without mutating the file.

## Scenarios

### Scenario: Privileged caller grants an operation list to self

* *GIVEN* the broker is running and the calling process has effective UID 0
* *AND* the policy file is writable by the broker
* *WHEN* the user runs `ghbrk allow acme/web push pr_open`
* *THEN* the broker MUST append an allow rule scoped to the caller's username for `acme/web` with operations `[push, pr_open]`
* *AND* the broker MUST hot-reload the policy so subsequent evaluations honour the new rule
* *AND* the command MUST print a confirmation naming the user, repo, and granted operations, then exit with status zero

### Scenario: Privileged caller grants a named role

* *GIVEN* the broker is running and the calling process has effective UID 0
* *WHEN* the user runs `ghbrk allow acme/web write`
* *THEN* the broker MUST append an allow rule whose `operations` field stores the literal role name `write`
* *AND* the broker MUST NOT expand the role into individual operations in the written rule
* *AND* the command MUST exit with status zero

### Scenario: Privileged caller grants to another user via --user

* *GIVEN* the broker is running and the calling process has effective UID 0
* *WHEN* the user runs `ghbrk allow acme/web write --user marconae`
* *THEN* the broker MUST append an allow rule scoped to user `marconae`
* *AND* the appended rule's `user` field MUST be `marconae` rather than the caller's username
* *AND* the command MUST exit with status zero

### Scenario: Unprivileged caller is denied

* *GIVEN* the broker is running and the calling process has effective UID other than 0
* *WHEN* the user runs `ghbrk allow acme/web push`
* *THEN* the broker MUST send a `Denied` frame indicating elevated privileges are required
* *AND* the broker MUST NOT modify the policy file
* *AND* the command MUST exit with a non-zero status

### Scenario: Grant referencing an unknown operation is rejected

* *GIVEN* the broker is running and the calling process has effective UID 0
* *WHEN* the user runs `ghbrk allow acme/web frobnicate`
* *THEN* the broker MUST send a `Denied` frame mentioning `frobnicate`
* *AND* the broker MUST NOT modify the policy file

### Scenario: Grant referencing an unknown role is rejected

* *GIVEN* the broker is running and the calling process has effective UID 0
* *AND* no role named `superuser` is defined
* *WHEN* the user runs `ghbrk allow acme/web superuser`
* *THEN* the broker MUST send a `Denied` frame mentioning `superuser`
* *AND* the broker MUST NOT modify the policy file

### Scenario: Malformed repo specifier is rejected client-side

* *GIVEN* the binary is installed
* *WHEN* the user runs `ghbrk allow not-a-valid-spec push`
* *THEN* the command MUST print an error describing the expected `<org>/<repo>` format to stderr
* *AND* the command MUST NOT contact the broker
* *AND* the command MUST exit with a non-zero status

### Scenario: Daemon unreachable is reported

* *GIVEN* no broker is listening on the configured socket path
* *WHEN* the user runs `ghbrk allow acme/web push`
* *THEN* the command MUST print an error indicating the broker is unavailable to stderr
* *AND* the command MUST exit with a non-zero status

### Scenario: Appended rule round-trips through the policy file

* *GIVEN* the broker is running and the calling process has effective UID 0
* *WHEN* the user runs `ghbrk allow acme/web write`
* *AND* the policy file is re-read from disk afterwards
* *THEN* the reloaded policy MUST parse without error
* *AND* the reloaded policy MUST contain the newly appended allow rule

# Feature: wire-protocol

Defines the request and response message formats and length-prefixed JSON framing used between the gateway client and the broker server, so both sides agree on parsing rules and message types without ambiguity.

## Background

Frames are length-prefixed: a 4-byte big-endian length followed by that many bytes of UTF-8 JSON, one JSON object per frame, the same in both directions; readers MUST treat declared lengths greater than 16 MiB as a protocol error. Two new caller-tool discriminants are added for the explicit gateway's diagnostic subcommands: `explain` and `policy`. Both are non-executing query requests: the broker resolves and/or evaluates policy and streams the result back as `StdoutChunk` frames terminated by an `Exit` frame, reusing the existing framing and server-frame variants. No new server-frame variant is required. The `check` discriminant is retained and reused by `ghbrk doctor`.

## Scenarios

<!-- DELTA:NEW -->
### Scenario: Explain request frame round-trips with the explain discriminant

* *GIVEN* a `Request { tool: "explain", args: ["git", "push", "origin", "main"], cwd: "/work/repo" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original
* *AND* the decoded `tool` MUST be the `explain` discriminant
<!-- /DELTA:NEW -->

<!-- DELTA:NEW -->
### Scenario: Policy-query request frame round-trips with the policy discriminant

* *GIVEN* a `Request { tool: "policy", args: ["acme/web"], cwd: "/work/repo" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original
* *AND* the decoded `tool` MUST be the `policy` discriminant
<!-- /DELTA:NEW -->

# Feature: wire-protocol

Defines the request and response message formats and length-prefixed JSON framing used between the gateway client and the broker server, so both sides agree on parsing rules and message types without ambiguity.

## Background

Frames are length-prefixed: a 4-byte big-endian unsigned integer giving payload length in bytes, followed by exactly that many bytes of UTF-8 JSON. Each frame contains exactly one JSON object. The same framing is used in both directions on the same Unix stream socket. There is no maximum hard limit on frame size beyond available memory, but readers MUST treat declared lengths greater than 16 MiB as a protocol error.

Three caller-tool discriminants support the explicit gateway's diagnostic and administrative subcommands: `explain`, `policy`, and `allow`. `explain` and `policy` are non-executing query requests: the broker resolves and/or evaluates policy and streams the result back as `StdoutChunk` frames terminated by an `Exit` frame, reusing the existing framing and server-frame variants. `allow` is a privileged mutation request: success is signalled with `StdoutChunk` then `Exit`, and privilege/validation failures with `Denied`. No new server-frame variant is required for any of these. The `check` discriminant is retained and reused by `ghbrk doctor`.

## Scenarios

### Scenario: Request frame round-trips through encoder and decoder

* *GIVEN* a `Request { tool: "git", args: ["push", "origin", "main"], cwd: "/work/repo" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original

### Scenario: Stdout chunk frame carries streaming output

* *GIVEN* the broker has captured 12 bytes of stdout from the spawned child
* *WHEN* the broker emits a `StdoutChunk { data: <12 bytes> }` frame
* *THEN* the gateway client MUST decode it as a `StdoutChunk` variant
* *AND* the gateway client MUST receive exactly the same 12 bytes

### Scenario: Stderr chunk frame is distinguishable from stdout

* *GIVEN* the broker emits a `StderrChunk { data: <bytes> }` frame
* *WHEN* the gateway client decodes it
* *THEN* the gateway client MUST route the bytes to its own stderr
* *AND* the gateway client MUST NOT route the bytes to its own stdout

### Scenario: Exit frame terminates the response stream

* *GIVEN* the spawned child has exited with status 7
* *WHEN* the broker emits an `Exit { code: 7 }` frame
* *THEN* the gateway client MUST treat the response as complete
* *AND* the gateway client MUST exit its own process with code 7

### Scenario: Denial frame carries structured error

* *GIVEN* the policy engine denied the request with reason `"branch main is protected"`
* *WHEN* the broker emits a `Denied { reason: "branch main is protected" }` frame
* *THEN* the gateway client MUST print the reason to stderr
* *AND* the gateway client MUST exit with a non-zero status

### Scenario: Frame with declared length exceeding 16 MiB is rejected

* *GIVEN* a frame header declaring length `0x01000001` (16 MiB + 1 byte)
* *WHEN* the decoder reads the length prefix
* *THEN* the decoder MUST return a protocol error
* *AND* the decoder MUST NOT attempt to read that many bytes

### Scenario: Truncated frame body returns parse error

* *GIVEN* a frame header declaring 100 bytes of payload
* *AND* only 40 bytes of payload follow before EOF
* *WHEN* the decoder attempts to read the frame
* *THEN* the decoder MUST return a parse error indicating truncation

### Scenario: Check request frame round-trips with the check tool discriminant

* *GIVEN* a `Request { tool: check, args: [], cwd: "/home/alice" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original
* *AND* the encoded `tool` discriminant field MUST be the string `"check"`

### Scenario: Explain request frame round-trips with the explain discriminant

* *GIVEN* a `Request { tool: "explain", args: ["git", "push", "origin", "main"], cwd: "/work/repo" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original
* *AND* the decoded `tool` MUST be the `explain` discriminant

### Scenario: Policy-query request frame round-trips with the policy discriminant

* *GIVEN* a `Request { tool: "policy", args: ["acme/web"], cwd: "/work/repo" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original
* *AND* the decoded `tool` MUST be the `policy` discriminant

### Scenario: Allow request frame round-trips with the allow discriminant

* *GIVEN* a `Request { tool: "allow", args: ["acme/web", "write", "--user", "marconae"], cwd: "/work/repo" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original
* *AND* the encoded `tool` discriminant field MUST be the string `"allow"`

### Scenario: Allow request reuses the existing server-frame variants

* *GIVEN* the broker has processed an `allow` request
* *WHEN* the broker reports the outcome
* *THEN* the broker MUST signal success using `StdoutChunk` followed by an `Exit` frame
* *AND* the broker MUST signal a privilege or validation failure using a `Denied` frame
* *AND* the broker MUST NOT introduce a new `ServerFrame` variant for the allow request

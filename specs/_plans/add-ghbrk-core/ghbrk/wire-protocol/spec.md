# Feature: wire-protocol

Defines the request and response message formats and length-prefixed JSON framing used between the shim client and the broker server, so both sides agree on parsing rules and message types without ambiguity.

## Background

Frames are length-prefixed: a 4-byte big-endian unsigned integer giving payload length in bytes, followed by exactly that many bytes of UTF-8 JSON. Each frame contains exactly one JSON object. The same framing is used in both directions on the same Unix stream socket. There is no maximum hard limit on frame size beyond available memory, but readers MUST treat declared lengths greater than 16 MiB as a protocol error.

## Scenarios

### Scenario: Request frame round-trips through encoder and decoder

* *GIVEN* a `Request { tool: "git", args: ["push", "origin", "main"], cwd: "/work/repo" }`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original

### Scenario: Stdout chunk frame carries streaming output

* *GIVEN* the broker has captured 12 bytes of stdout from the spawned child
* *WHEN* the broker emits a `StdoutChunk { data: <12 bytes> }` frame
* *THEN* the shim MUST decode it as a `StdoutChunk` variant
* *AND* the shim MUST receive exactly the same 12 bytes

### Scenario: Stderr chunk frame is distinguishable from stdout

* *GIVEN* the broker emits a `StderrChunk { data: <bytes> }` frame
* *WHEN* the shim decodes it
* *THEN* the shim MUST route the bytes to its own stderr
* *AND* the shim MUST NOT route the bytes to its own stdout

### Scenario: Exit frame terminates the response stream

* *GIVEN* the spawned child has exited with status 7
* *WHEN* the broker emits an `Exit { code: 7 }` frame
* *THEN* the shim MUST treat the response as complete
* *AND* the shim MUST exit its own process with code 7

### Scenario: Denial frame carries structured error

* *GIVEN* the policy engine denied the request with reason `"branch main is protected"`
* *WHEN* the broker emits a `Denied { reason: "branch main is protected" }` frame
* *THEN* the shim MUST print the reason to stderr
* *AND* the shim MUST exit with a non-zero status

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

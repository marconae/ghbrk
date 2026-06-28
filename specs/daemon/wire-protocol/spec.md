# Feature: wire-protocol

Defines the request and response message formats and length-prefixed JSON framing used between the gateway client and the broker server, so both sides agree on parsing rules and message types without ambiguity.

## Background

Frames are length-prefixed: a 4-byte big-endian unsigned integer giving payload length in bytes, followed by exactly that many bytes of UTF-8 JSON. Each frame contains exactly one JSON object. The same framing is used in both directions on the same Unix stream socket. There is no maximum hard limit on frame size beyond available memory, but readers MUST treat declared lengths greater than 16 MiB as a protocol error.

Three caller-tool discriminants support the explicit gateway's diagnostic and administrative subcommands: `explain`, `policy`, and `allow`. `explain` and `policy` are non-executing query requests: the broker resolves and/or evaluates policy and streams the result back as `StdoutChunk` frames terminated by an `Exit` frame, reusing the existing framing and server-frame variants. `allow` is a privileged mutation request: success is signalled with `StdoutChunk` then `Exit`, and privilege/validation failures with `Denied`. No new server-frame variant is required for any of these. The `check` discriminant is retained and reused by `ghbrk doctor`.

The client-to-broker direction carries two frame kinds, distinguished by position rather than by a shared tag. The first frame on a connection is always a bare `Request` object, exactly as before. Every frame after it is a `ClientFrame`: a `kind`-tagged enum with the variants `StdinChunk { data }` and `StdinEof`, which carry the caller's own standard input to the child process the broker spawns. Splitting the two shapes by position rather than wrapping `Request` in the tagged enum is what leaves the first frame's shape alone, so a client built before stdin forwarding existed still speaks the current protocol.

The `Request` object grows fields over time, so its compatibility rests on two decoder rules rather than on a fixed byte sequence. A decoder MUST ignore any field of the first frame it does not know, which is what lets a client that emits fields a released daemon never heard of still be understood by it. A decoder MUST supply the documented default for any field the sender omitted, which is what lets a request from a client released before a field existed still decode. Both rules are load-bearing in a rolling deployment, where daemon and clients update at different times, and both apply to every field this change adds.

The `Request` declares whether any `ClientFrame` follows it. The declaration is a boolean `client_frames` field that defaults to `false` when absent, which is how every request from a client released before this change decodes. Position alone cannot carry that signal: on a socket that stays open, a client that will never send a client frame is indistinguishable from a client whose first chunk has not arrived yet, and released clients write the `Request` and then hold their write half open for the whole response, so the broker never observes end-of-file there. Waiting for a first frame would hang such a pair outright; waiting with a timeout would trade the hang for a race. When `client_frames` is `false` the broker MUST NOT wait for a client frame: it closes the spawned child's standard input at spawn time, so the child observes end-of-file at once — the behaviour every released client already gets. When `client_frames` is `true` the client MUST send at least one `ClientFrame`, MUST terminate the sequence with `StdinEof`, and MUST shut down its write half after that frame, so the broker observes end-of-file even if a frame is lost to a write failure.

The `check` request carries one further optional field: a typed `caller_tmp` object holding the device and inode of the caller's own `/tmp`, for the `cli/doctor` shared-filesystem check. It defaults to absent. It carries two integers the broker compares against a value it derives itself; it names no path, so the broker opens nothing on the caller's word. A typed pair keeps it out of `args`, which otherwise carries a child process's argv.

Stdin frames flow while the broker is streaming `StdoutChunk` and `StderrChunk` frames back on the same socket, so the connection is full-duplex from the moment the child is spawned. Neither side may serialise the two directions. A child that writes output while waiting for input deadlocks against a peer that reads only after it has finished writing, and a client streaming standard input at a peer that never reads it blocks on a full socket buffer instead of collecting its `Exit` frame — which is exactly what a new client meets on a daemon released before this change.

The 16 MiB ceiling applies to `ClientFrame` frames exactly as it does to every other frame. A sender reads at most 8 KiB of the caller's standard input per `StdinChunk`, the same bound the executor uses for its output reads. `data` is a byte vector, which `serde_json` renders as an array of decimal numbers at two to four wire bytes per input byte, so a chunk sized near the ceiling would fail at encode time rather than surface as a protocol error a receiver could report. At 8 KiB the encoded frame stays below 32 KiB. Total stdin volume is unbounded while per-frame and resident memory stay flat.

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

### Scenario: Stdin chunk frame round-trips through encoder and decoder

* *GIVEN* the gateway client has read 12 bytes from its own standard input
* *WHEN* the client encodes a `ClientFrame::StdinChunk { data: <12 bytes> }` frame and the broker decodes it
* *THEN* the broker MUST decode it as the `StdinChunk` variant
* *AND* the broker MUST recover exactly the same 12 bytes in the same order
* *AND* the encoded `kind` discriminant field MUST be the string `"stdin_chunk"`

### Scenario: Stdin EOF frame round-trips and carries no payload

* *GIVEN* the gateway client has read its standard input to end-of-file
* *WHEN* the client encodes a `ClientFrame::StdinEof` frame and the broker decodes it
* *THEN* the broker MUST decode it as the `StdinEof` variant
* *AND* the encoded `kind` discriminant field MUST be the string `"stdin_eof"`

### Scenario: Request frame encoding is unchanged by the addition of client frames

* *GIVEN* the JSON text `{"tool":"gh","args":["pr","create"],"cwd":"/work/repo"}`
* *WHEN* the broker decodes it as the first frame of a connection
* *THEN* the broker MUST decode it as a `Request` with tool `gh`
* *AND* the broker MUST NOT require a `kind` discriminant on the first frame

### Scenario: Connection carrying no client frames after the request reads as empty stdin

* *GIVEN* a client has written a `Request` frame with `client_frames` set to `true` and then closed its write half
* *WHEN* the broker attempts to read the next client frame
* *THEN* the broker MUST treat the end-of-file as an empty standard input
* *AND* the broker MUST NOT emit a `Denied` frame for the missing client frames
* *AND* the broker MUST still emit an `Exit` frame for the spawned child

### Scenario: Request without the client-frames declaration never waits for a client frame

* *GIVEN* a client released before stdin forwarding has written a `Request` frame carrying no `client_frames` field
* *AND* that client leaves its write half open for the whole response, so the broker never observes end-of-file on it
* *WHEN* the broker spawns a child that reads its standard input to end-of-file
* *THEN* the broker MUST decode the absent `client_frames` field as `false` and MUST NOT wait for a `ClientFrame`
* *AND* the child MUST observe end-of-file on its standard input rather than blocking
* *AND* the broker MUST emit an `Exit` frame carrying the child's own exit code

### Scenario: Client streaming standard input to a peer that never reads it completes on the Exit frame

* *GIVEN* a daemon released before stdin forwarding, which reads the `Request` frame and then only writes
* *AND* a client with 1 MiB of piped standard input and `client_frames` set to `true`
* *WHEN* the client's `StdinChunk` writes fill the socket buffer that peer never drains
* *THEN* the client MUST continue reading server frames while its own write is blocked
* *AND* the client MUST exit on the `Exit` frame with the code that frame carries
* *AND* the client MUST NOT deadlock waiting to finish sending standard input

### Scenario: One stdin chunk carries at most one bounded read

* *GIVEN* a client with 1 MiB of piped standard input
* *WHEN* the client encodes the stream as `StdinChunk` frames
* *THEN* each frame's `data` MUST carry at most 8192 bytes
* *AND* each encoded frame body MUST stay below the 16 MiB ceiling with the JSON byte-array expansion applied

### Scenario: Check request round-trips with the typed caller /tmp identity

* *GIVEN* a `Request { tool: check, args: [], cwd: "/home/alice" }` carrying a `caller_tmp` object with the device and inode of the caller's `/tmp`
* *WHEN* the request is encoded to bytes and decoded back
* *THEN* the decoded value MUST equal the original
* *AND* a `check` request encoded without a `caller_tmp` object MUST decode with that field absent

### Scenario: Request decoder accepts a first frame carrying fields it does not know

* *GIVEN* the JSON text `{"tool":"gh","args":["pr","create"],"cwd":"/work/repo","client_frames":true,"caller_tmp":null,"field_from_a_later_release":7}`
* *WHEN* a decoder that knows none of the last three fields reads it as the first frame
* *THEN* the decoder MUST decode it as a `Request` with tool `gh`
* *AND* the decoder MUST NOT reject the frame for the fields it does not know
* *AND* the decoder MUST leave every field the sender omitted at its documented default

### Scenario: Stdin chunk with declared length exceeding 16 MiB is rejected

* *GIVEN* a client frame header declaring length `0x01000001` (16 MiB + 1 byte)
* *WHEN* the broker reads the length prefix
* *THEN* the broker MUST return a protocol error
* *AND* the broker MUST NOT attempt to read that many bytes

### Scenario: Stdin frames and output frames share one connection concurrently

* *GIVEN* the broker has spawned a child for an allowed request
* *WHEN* the client writes `StdinChunk` frames while the broker writes `StdoutChunk` frames
* *THEN* the broker MUST continue reading client frames while it writes server frames
* *AND* the gateway client MUST continue reading server frames while it writes client frames
* *AND* neither side SHALL wait for its own direction to finish before servicing the other

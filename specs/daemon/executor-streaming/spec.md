# Feature: executor-streaming

Spawns the real `git` or `gh` binary inside the daemon, applying the injected credentials and resolved cwd, then streams stdout and stderr back to the gateway client in real time over the wire protocol and finally emits the child's exit code.

## Background

Privilege drop applied at spawn time is specified in the `executor-privilege-drop` feature.

The child's standard input is a pipe fed from the caller's own standard input, not `/dev/null`. The executor owns both directions of the child-to-wire relay: it reads `ClientFrame::StdinChunk` frames from the connection and writes their bytes to the child's stdin, while it reads the child's stdout and stderr and writes `StdoutChunk` and `StderrChunk` frames back. Owning both directions in one module keeps the deadlock-avoidance rule in a single place, because the two directions constrain each other: a child that blocks writing stdout while the executor blocks writing stdin would stall both.

The executor therefore drives the input and output relays concurrently and finishes on the output side. The input relay ends at the first of `StdinEof`, end-of-file on the connection, or a write error on the child's stdin; the executor closes the child's stdin at that point so a child that waits for end-of-file can proceed. A write error on the child's stdin — the child exited or closed stdin before consuming everything the caller sent — is a normal outcome, not a failure: `gh` reads a body once and exits, and the caller may have more bytes queued. The executor discards the remainder and reports the child's own exit code. The `Exit` frame stays the final frame on the wire in every case.

The input source is a required argument rather than an option, so the executor makes the decision instead of asking its caller to. A caller with nothing to send passes a source that is already at end-of-file, which closes the child's stdin at once and reproduces the previous `/dev/null` behaviour exactly.

Which source the broker supplies is decided by the request's `client_frames` declaration, defined in `daemon/wire-protocol`. A request that declares client frames gets the connection's read half. A request that does not — every request from a client released before stdin forwarding — gets a source already at end-of-file, never the read half. Such a client holds its write half open for the whole response, so the executor would otherwise wait on a frame that never arrives while the child waits on a pipe that never closes.

## Scenarios

### Scenario: Child stdout is streamed to gateway client as StdoutChunk frames

* *GIVEN* the daemon spawned `git status` whose stdout produces 3 separate writes
* *WHEN* the child writes each chunk
* *THEN* the daemon MUST emit at least one `StdoutChunk` frame per write
* *AND* each frame MUST be sent before the child completes if the child has not yet exited

### Scenario: Child stderr is streamed as StderrChunk frames

* *GIVEN* the daemon spawned `git push` whose stderr emits progress lines
* *WHEN* the child writes a stderr line
* *THEN* the daemon MUST emit a `StderrChunk` frame containing those bytes

### Scenario: Child exit code is propagated in Exit frame

* *GIVEN* the child `git push` exits with status 1
* *WHEN* the executor observes the exit
* *THEN* the daemon MUST emit `Exit { code: 1 }` as the final frame

### Scenario: Child cwd matches request cwd

* *GIVEN* the request `cwd` is `/home/alice/projects/foo`
* *AND* the `ChildSpec` carries the peer user's `uid` and `gid`
* *WHEN* the executor spawns the child with privilege drop applied
* *THEN* the child's working directory MUST be `/home/alice/projects/foo`
* *AND* the child MUST be able to traverse and write within the directory using the peer user's natural filesystem permissions

### Scenario: Stdout and stderr are interleaved in arrival order, not merged

* *GIVEN* a child writes stdout, then stderr, then stdout in that order
* *WHEN* the executor forwards frames
* *THEN* the gateway client MUST receive a `StdoutChunk`, then a `StderrChunk`, then a `StdoutChunk`
* *AND* stdout and stderr bytes MUST NOT be combined into a single chunk

### Scenario: Killed child reports non-zero exit

* *GIVEN* a spawned child is terminated by SIGKILL
* *WHEN* the executor observes the exit
* *THEN* the daemon MUST emit an `Exit` frame with a non-zero code

### Scenario: Failure to spawn child reports denial-style error

* *GIVEN* the requested binary `git` is not on the daemon's `PATH`
* *WHEN* the executor attempts to spawn
* *THEN* the daemon MUST emit a `Denied { reason: ... }` frame mentioning the spawn failure
* *AND* the daemon MUST NOT crash

### Scenario: Large output stream does not exhaust memory

* *GIVEN* the child produces 100 MiB of stdout in many chunks
* *WHEN* the executor streams the output
* *THEN* the daemon's resident memory MUST NOT grow unboundedly with total output size
* *AND* the gateway client MUST receive the bytes in order

### Scenario: Child stdin receives the bytes carried by StdinChunk frames

* *GIVEN* the daemon spawned `cat` for an allowed request
* *AND* the client sends `StdinChunk { data: "hello body\n" }` followed by `StdinEof`
* *WHEN* the executor relays the connection
* *THEN* the child MUST read exactly `hello body\n` from its standard input
* *AND* the daemon MUST emit that text back as `StdoutChunk` frames
* *AND* the daemon MUST emit `Exit { code: 0 }` as the final frame

### Scenario: Multiple stdin chunks reach the child in order as one stream

* *GIVEN* the client sends `StdinChunk { data: "line one\n" }`, then `StdinChunk { data: "line two\n" }`, then `StdinEof`
* *WHEN* the executor relays them to the spawned `cat`
* *THEN* the child MUST read `line one\nline two\n` in that order
* *AND* the executor MUST NOT insert a boundary marker between the two chunks

### Scenario: StdinEof closes the child's standard input

* *GIVEN* the daemon spawned `cat`, which runs until its standard input reaches end-of-file
* *AND* the client sends `StdinChunk { data: "x" }` followed by `StdinEof`
* *WHEN* the executor processes the `StdinEof` frame
* *THEN* the executor MUST close the child's standard input
* *AND* the child MUST exit
* *AND* the daemon MUST emit an `Exit` frame

### Scenario: Connection end-of-file closes the child's standard input

* *GIVEN* the daemon spawned `cat`
* *AND* the client writes its `Request` frame and then closes its write half without sending `StdinEof`
* *WHEN* the executor reads end-of-file where a client frame would be
* *THEN* the executor MUST close the child's standard input
* *AND* the executor MUST NOT emit a `Denied` frame
* *AND* the daemon MUST emit an `Exit` frame

### Scenario: Executor given an already-exhausted stdin source closes the child's standard input immediately

* *GIVEN* the executor is invoked with a client-frame source that is already at end-of-file
* *WHEN* the executor spawns a child that reads its standard input to end-of-file
* *THEN* the child MUST observe an immediate end-of-file on standard input
* *AND* the daemon MUST emit an `Exit` frame
* *AND* the executor MUST NOT require the caller to choose between a present and an absent input source

### Scenario: Request without the client-frames declaration gets an exhausted input source

* *GIVEN* the broker has decoded a `Request` whose `client_frames` field is absent
* *AND* the client that sent it leaves its write half open for the whole response
* *WHEN* the broker invokes the executor for the spawned child
* *THEN* the broker MUST pass an input source already at end-of-file rather than the connection's read half
* *AND* the executor MUST close the child's standard input at spawn time
* *AND* the daemon MUST emit an `Exit` frame carrying the child's own exit code, rather than leaving a child that reads standard input blocked

### Scenario: Child that exits before consuming all stdin does not fail the invocation

* *GIVEN* the daemon spawned a child that exits without reading its standard input
* *AND* the client sends 1 MiB of `StdinChunk` frames
* *WHEN* the executor's write to the child's standard input fails with a broken pipe
* *THEN* the executor MUST treat the write failure as the end of the input relay
* *AND* the daemon MUST emit the child's own `Exit` code
* *AND* the daemon MUST NOT emit a `Denied` frame or terminate abnormally

### Scenario: Output streams while stdin is still arriving

* *GIVEN* the daemon spawned a child that echoes each input line to stdout before its standard input reaches end-of-file
* *WHEN* the client sends a `StdinChunk` and then waits for output before sending the next one
* *THEN* the daemon MUST emit the `StdoutChunk` for the first line before the client sends the second chunk
* *AND* the executor MUST NOT defer reading client frames until the child's output has completed

### Scenario: Large stdin stream does not exhaust memory

* *GIVEN* the client sends 100 MiB of standard input as many `StdinChunk` frames
* *WHEN* the executor relays them to the child
* *THEN* the daemon's resident memory MUST NOT grow unboundedly with total input size
* *AND* the child MUST receive the bytes in order

### Scenario: Exit frame remains the final frame after the input relay ends

* *GIVEN* the executor has closed the child's standard input and the child has exited
* *WHEN* the executor finishes the relay
* *THEN* the daemon MUST emit the `Exit` frame after every `StdoutChunk` and `StderrChunk` frame
* *AND* the daemon MUST NOT emit any frame after the `Exit` frame

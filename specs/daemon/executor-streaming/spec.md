# Feature: executor-streaming

Spawns the real `git` or `gh` binary inside the daemon, applying the injected credentials and resolved cwd, then streams stdout and stderr back to the gateway client in real time over the wire protocol and finally emits the child's exit code.

## Background

The daemon spawns the child with stdout and stderr captured (pipe), stdin closed, and the working directory set to the resolved cwd from the request. Output bytes are forwarded as soon as they arrive — no buffering until child exit. The implementation uses Tokio async readers for stdout and stderr concurrently.

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
* *WHEN* the executor spawns the child
* *THEN* the child's working directory MUST be `/home/alice/projects/foo`

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

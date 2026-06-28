# Feature: cli-dispatch

Routes the single `ghbrk` binary to its subcommands via clap, so one installed artefact serves the broker server, the gateway relays, and the diagnostic and administrative commands.

## Background

The binary is built from a single Rust crate and dispatches via clap only — argv[0] is no longer inspected for `git`/`gh` basenames, and there are no symlinks. The clap subcommand set is `daemon`, `doctor`, `explain`, `policy`, `allow`, `git`, and `gh`; the former `check` subcommand is absorbed into `doctor` (see cli/doctor). `ghbrk git` and `ghbrk gh` relay only operations that leave the machine (remote/authenticated) to the broker; local-only git subcommands return a guidance error before any socket connection instead of being relayed. clap also provides the standard `--version`/`-V` flag, which prints the program name and the crate version read from `CARGO_PKG_VERSION` at compile time and exits zero.

The `git` and `gh` gateways forward the caller's own standard input to the broker, so a brokered command that reads a file from stdin behaves as the unbrokered command does. `gh pr create --body-file -` and every other `-`-as-stdin form work through ghbrk without the caller rewriting the invocation. The gateway does not parse the forwarded argv to decide whether stdin is wanted; parsing would need a per-subcommand table of which flags take a file, and that table would be wrong the day `gh` adds a flag. The gateway keys on the caller's stdin instead: when standard input is not a terminal it streams it, and when standard input is a terminal it sends end-of-file without reading, so an interactive `ghbrk gh pr list` never captures the user's keystrokes.

Only `git` and `gh` forward stdin. `doctor`, `explain`, `policy`, and `allow` never spawn a child, so they send no stdin frames at all, and only `git` and `gh` requests set the `client_frames` field that tells the broker frames follow. The gateway shuts down its write half after the `StdinEof` frame, so the broker sees end-of-file on that direction even when a frame was lost to a write failure. The gateway streams standard input and reads the broker's response frames concurrently: reading output only after standard input has been fully sent would deadlock against a child that writes output while waiting for more input, and would also block against a daemon released before this change, which never reads the stdin direction at all. The gateway stops forwarding as soon as the `Exit` frame arrives, so a command that ignores standard input does not hold the process open against an endless pipe, and a write failure on the stdin direction never changes the exit code the broker reported.

Forwarding a pipe that never closes keeps the child waiting for input, where the same invocation previously saw an immediate end-of-file. That is the same contract `ssh host cmd` has, and the shell already carries its opt-out: `ghbrk gh … < /dev/null` gives the child end-of-file at once, so no flag or environment variable is added for it.

## Scenarios

### Scenario: Binary invoked as ghbrk daemon

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk daemon`
* *THEN* the process MUST enter daemon mode and start the broker server

### Scenario: Binary invoked as ghbrk git push routes to broker

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk git push origin main`
* *THEN* the process MUST relay the `git` invocation to the broker
* *AND* the process MUST forward the args `["push", "origin", "main"]` to the broker

### Scenario: Binary invoked as ghbrk gh routes to broker

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk gh pr list`
* *THEN* the process MUST relay the `gh` invocation to the broker
* *AND* the process MUST forward the args `["pr", "list"]` to the broker

### Scenario: ghbrk git with a local-only subcommand returns a guidance error

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk git status`
* *THEN* the process MUST NOT relay the invocation to the broker
* *AND* the process MUST print a guidance message to stderr instructing the user to run `git status` directly because ghbrk only brokers remote operations
* *AND* the process MUST exit with a non-zero status

### Scenario: ghbrk git with no subcommand returns a guidance error

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk git` with no further arguments
* *THEN* the process MUST NOT relay the invocation to the broker
* *AND* the process MUST print a guidance message naming the brokered remote operations to stderr
* *AND* the process MUST exit with a non-zero status

### Scenario: ghbrk doctor dispatches to the doctor command

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk doctor`
* *THEN* the process MUST dispatch to the environment-diagnostics command
* *AND* the process MUST NOT relay any invocation to the broker for routing classification

### Scenario: ghbrk explain dispatches to the explain command

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk explain git push origin main`
* *THEN* the process MUST dispatch to the explain command with the trailing command tokens `["git", "push", "origin", "main"]`

### Scenario: ghbrk policy dispatches to the policy-query command

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk policy acme/web`
* *THEN* the process MUST dispatch to the policy-query command with the repo argument `acme/web`

### Scenario: Unknown subcommand exits with usage error

* *GIVEN* the binary is invoked as `ghbrk frobnicate`
* *WHEN* clap parses the argv
* *THEN* the process MUST exit with a non-zero status
* *AND* the process MUST print clap-generated usage text to stderr

### Scenario: Help flag shows subcommand list

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk --help`
* *THEN* the process MUST print a help message listing the `daemon`, `doctor`, `explain`, `policy`, `allow`, `git`, and `gh` subcommands
* *AND* the help message MUST NOT list a `check` subcommand
* *AND* the process MUST exit with status zero

### Scenario: ghbrk --version prints version and exits zero

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk --version`
* *THEN* the process MUST print a version line to stdout
* *AND* the version line MUST contain the program name `ghbrk`
* *AND* the version line MUST contain the crate version read from `CARGO_PKG_VERSION` at compile time
* *AND* the process MUST exit with status zero

### Scenario: ghbrk allow dispatches to the allow command

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk allow acme/web push pr_open`
* *THEN* the process MUST dispatch to the allow command with the repo argument `acme/web` and operands `["push", "pr_open"]`

### Scenario: ghbrk allow accepts the --user flag

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk allow acme/web write --user marconae`
* *THEN* the process MUST dispatch to the allow command with the target user `marconae`

### Scenario: ghbrk gh forwards piped standard input to the broker

* *GIVEN* the caller's standard input is a pipe carrying `body text\n`
* *WHEN* the user runs `ghbrk gh pr create --body-file -`
* *THEN* the gateway MUST send the `Request` frame first
* *AND* the gateway MUST send the bytes `body text\n` as one or more `StdinChunk` frames
* *AND* the gateway MUST send a `StdinEof` frame after the pipe reaches end-of-file

### Scenario: ghbrk git forwards piped standard input to the broker

* *GIVEN* the caller's standard input is a pipe carrying the refspec line `refs/heads/main:refs/remotes/origin/main\n`
* *WHEN* the user runs `ghbrk git fetch origin --stdin`
* *THEN* the gateway MUST send the bytes `refs/heads/main:refs/remotes/origin/main\n` as one or more `StdinChunk` frames
* *AND* the gateway MUST send a `StdinEof` frame after the pipe reaches end-of-file

### Scenario: ghbrk gh with a terminal on standard input sends end-of-file without reading

* *GIVEN* the caller's standard input is a terminal
* *WHEN* the user runs `ghbrk gh pr list`
* *THEN* the gateway MUST send a `StdinEof` frame without reading from the terminal
* *AND* the gateway MUST NOT send any `StdinChunk` frame
* *AND* the gateway MUST NOT consume any byte the user types

### Scenario: Standard input that never reaches end-of-file keeps the child waiting

* *GIVEN* the caller's standard input is a pipe an unrelated parent process holds open and never writes to
* *WHEN* the user runs `ghbrk gh pr create --body-file -`
* *THEN* the gateway MUST NOT send a `StdinEof` frame while that pipe stays open
* *AND* the gateway MUST keep relaying until the pipe reaches end-of-file or the `Exit` frame arrives
* *AND* the child MUST therefore wait for input, which is a deliberate change from the immediate end-of-file it saw before stdin forwarding existed
* *AND* redirecting standard input with `< /dev/null` MUST give the child an immediate end-of-file

### Scenario: Diagnostic and administrative subcommands forward no standard input

* *GIVEN* the caller's standard input is a pipe carrying 1 KiB of data
* *WHEN* the user runs `ghbrk explain git push origin main`
* *THEN* the gateway MUST NOT send any `StdinChunk` frame
* *AND* the gateway MUST NOT send a `StdinEof` frame
* *AND* the gateway MUST NOT read the pipe

### Scenario: Gateway stops forwarding standard input once the Exit frame arrives

* *GIVEN* the caller's standard input is an endless pipe
* *AND* the broker has emitted `Exit { code: 0 }`
* *WHEN* the gateway reads the `Exit` frame
* *THEN* the gateway MUST stop reading the caller's standard input
* *AND* the gateway MUST exit with code 0 without waiting for the pipe to close

### Scenario: Broker that never reads standard input does not change the exit code

* *GIVEN* the caller's standard input is a pipe carrying 1 MiB of data
* *AND* the broker closes the connection after emitting `Exit { code: 3 }`
* *WHEN* the gateway's write of a `StdinChunk` frame fails with a broken pipe
* *THEN* the gateway MUST exit with code 3
* *AND* the gateway MUST NOT print a protocol error to stderr
* *AND* the gateway MUST NOT panic

### Scenario: Gateway reads response frames while it is still sending standard input

* *GIVEN* the caller's standard input is a pipe larger than the socket buffer
* *AND* the broker emits `StdoutChunk` frames before the gateway has sent every `StdinChunk`
* *WHEN* the gateway relays the connection
* *THEN* the gateway MUST write the received stdout bytes to its own standard output before it finishes sending standard input
* *AND* the gateway MUST NOT block sending standard input until the response stream has completed

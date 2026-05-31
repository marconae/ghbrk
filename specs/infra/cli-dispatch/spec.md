# Feature: cli-dispatch

Routes the single `ghbrk` binary to its subcommands via clap, so one installed artefact serves the broker server, the gateway relays (`ghbrk git`/`ghbrk gh`), and the diagnostic commands (`doctor`, `explain`, `policy`). There is no transparent interception.

## Background

The binary is built from a single Rust crate and dispatches via clap only — argv[0] is no longer inspected for `git`/`gh` basenames, and there are no symlinks. The clap subcommand set is `daemon`, `doctor`, `explain`, `policy`, `git`, and `gh`; the former `check` subcommand is absorbed into `doctor` (see infra/doctor). `ghbrk git` and `ghbrk gh` relay only operations that leave the machine (remote/authenticated) to the broker; local-only git subcommands return a guidance error before any socket connection instead of being relayed. clap also provides the standard `--version`/`-V` flag, which prints the program name and the crate version read from `CARGO_PKG_VERSION` at compile time and exits zero.

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
* *THEN* the process MUST print a help message listing the `daemon`, `doctor`, `explain`, `policy`, `git`, and `gh` subcommands
* *AND* the help message MUST NOT list a `check` subcommand
* *AND* the process MUST exit with status zero

### Scenario: ghbrk --version prints version and exits zero

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk --version`
* *THEN* the process MUST print a version line to stdout
* *AND* the version line MUST contain the program name `ghbrk`
* *AND* the version line MUST contain the crate version read from `CARGO_PKG_VERSION` at compile time
* *AND* the process MUST exit with status zero

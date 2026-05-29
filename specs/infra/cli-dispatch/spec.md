# Feature: cli-dispatch

Routes the single `ghbrk` binary to daemon mode, the health-check command, or shim mode based on subcommand and argv[0], so that one installed artefact serves the broker server, the `ghbrk check` diagnostics, and the transparent `git`/`gh` shims.

## Background

The binary is built from a single Rust crate. argv[0] inspection happens before clap parsing. Recognised shim names are exactly `git` and `gh`; any other argv[0] basename falls through to clap subcommand dispatch. The clap subcommand set is `daemon`, `check`, `git`, and `gh`.

## Scenarios

### Scenario: Binary invoked as ghbrk daemon

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk daemon`
* *THEN* the process MUST enter daemon mode and start the broker server
* *AND* the process MUST NOT enter shim mode

### Scenario: Binary invoked as ghbrk git

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk git status`
* *THEN* the process MUST enter shim mode for the `git` tool
* *AND* the shim MUST forward the args `["status"]` to the broker

### Scenario: Binary invoked as ghbrk gh

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk gh pr list`
* *THEN* the process MUST enter shim mode for the `gh` tool
* *AND* the shim MUST forward the args `["pr", "list"]` to the broker

### Scenario: Symlink named git activates shim mode

* *GIVEN* a symlink named `git` points at the `ghbrk` binary
* *AND* the symlink is invoked as `git push origin main`
* *WHEN* argv[0] basename is read
* *THEN* the process MUST enter shim mode for the `git` tool
* *AND* the shim MUST forward the args `["push", "origin", "main"]` to the broker

### Scenario: Symlink named gh activates shim mode

* *GIVEN* a symlink named `gh` points at the `ghbrk` binary
* *AND* the symlink is invoked as `gh pr create --title foo`
* *WHEN* argv[0] basename is read
* *THEN* the process MUST enter shim mode for the `gh` tool
* *AND* the shim MUST forward the args `["pr", "create", "--title", "foo"]` to the broker

### Scenario: Unknown subcommand exits with usage error

* *GIVEN* the binary is invoked as `ghbrk frobnicate`
* *WHEN* clap parses the argv
* *THEN* the process MUST exit with a non-zero status
* *AND* the process MUST print clap-generated usage text to stderr

### Scenario: Help flag shows subcommand list

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk --help`
* *THEN* the process MUST print a help message listing the `daemon`, `check`, `git`, and `gh` subcommands
* *AND* the process MUST exit with status zero

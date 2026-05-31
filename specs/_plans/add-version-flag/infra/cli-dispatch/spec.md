# Feature: cli-dispatch

Routes the single `ghbrk` binary to its subcommands via clap, so one installed artefact serves the broker server, the gateway relays (`ghbrk git`/`ghbrk gh`), and the diagnostic commands (`doctor`, `explain`, `policy`). There is no transparent interception.

## Background

The binary is built from a single Rust crate and dispatches via clap only — argv[0] is no longer inspected for `git`/`gh` basenames, and there are no symlinks. The clap subcommand set is `daemon`, `doctor`, `explain`, `policy`, `git`, and `gh`; the former `check` subcommand is absorbed into `doctor` (see infra/doctor). `ghbrk git` and `ghbrk gh` relay only operations that leave the machine (remote/authenticated) to the broker; local-only git subcommands return a guidance error before any socket connection instead of being relayed. clap also provides the standard `--version`/`-V` flag, which prints the program name and the crate version read from `CARGO_PKG_VERSION` at compile time and exits zero.

## Scenarios

<!-- DELTA:NEW -->
### Scenario: ghbrk --version prints version and exits zero

* *GIVEN* the binary is installed at `/usr/local/bin/ghbrk`
* *WHEN* the user runs `ghbrk --version`
* *THEN* the process MUST print a version line to stdout
* *AND* the version line MUST contain the program name `ghbrk`
* *AND* the version line MUST contain the crate version read from `CARGO_PKG_VERSION` at compile time
* *AND* the process MUST exit with status zero
<!-- /DELTA:NEW -->

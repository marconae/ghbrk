# Feature: shim-config

Provides the shim with an optional, file-based configuration that overrides where the real `git` and `gh` binaries live. This entire feature is removed: with no transparent passthrough shim there is no real-binary path to configure.

## Background

The shim read its configuration from `/etc/ghbrk/config.yaml` at startup. With the transparent shim removed, the configuration file and its loader are deleted; agents call plain `git`/`gh` directly and invoke `ghbrk git`/`ghbrk gh` explicitly for brokered operations.

## Scenarios

<!-- DELTA:REMOVED -->
### Scenario: Missing config file falls back to compiled-in defaults

* *GIVEN* no file exists at the shim configuration path
* *WHEN* the shim loads its configuration
* *THEN* the shim MUST use `/usr/bin/git` as the real git path
* *AND* the shim MUST use `/usr/bin/gh` as the real gh path
* *AND* the shim MUST NOT treat the absent file as an error
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Config file overrides both real binary paths

* *GIVEN* a config file sets `real_git: /usr/local/bin/git` and `real_gh: /usr/local/bin/gh`
* *WHEN* the shim loads its configuration
* *THEN* the shim MUST use `/usr/local/bin/git` as the real git path
* *AND* the shim MUST use `/usr/local/bin/gh` as the real gh path
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Config file with one field uses the default for the other

* *GIVEN* a config file that sets only `real_git: /opt/git/bin/git`
* *WHEN* the shim loads its configuration
* *THEN* the shim MUST use `/opt/git/bin/git` as the real git path
* *AND* the shim MUST use the default `/usr/bin/gh` as the real gh path
<!-- /DELTA:REMOVED -->

<!-- DELTA:REMOVED -->
### Scenario: Malformed config file reports a clear error

* *GIVEN* a config file containing content that is not valid YAML for the config schema
* *WHEN* the shim loads its configuration
* *THEN* the shim MUST exit non-zero
* *AND* the shim MUST print an error naming the configuration path to stderr
* *AND* the shim MUST NOT silently fall back to defaults on a parse failure
<!-- /DELTA:REMOVED -->

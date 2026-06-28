# Feature: systemd-unit

Defines the systemd service unit for the ghbrk daemon: how the unit starts the process, which user it runs as, the runtime directory it owns for the Unix socket, and the hardening directives that constrain the daemon's capabilities.

## Background

The unit file lives at `deploy/linux/ghbrk.service`. To let the daemon drop spawned children to the requesting user, the unit grants exactly `CAP_SETUID` and `CAP_SETGID` (via `AmbientCapabilities` and `CapabilityBoundingSet`) and sets `ProtectHome=no` so user-owned children can write repositories under user home directories. `NoNewPrivileges=true` is retained: it does not block `setuid(2)`/`setgid(2)` when the capability is already held, and it still prevents SUID-binary escalation. `ProtectSystem=strict` makes `/usr`, `/boot`, and `/etc` read-only inside the daemon's private mount namespace, so every path the daemon must write to has to be re-granted via `ReadWritePaths=`; the default policy path is `Environment=GHBRK_POLICY=/etc/ghbrk/policy.yaml`, and the broker is the sole writer of that file, performing an atomic temp-file-plus-rename inside its parent directory, so `ReadWritePaths=` additionally includes `/etc/ghbrk` alongside `/run/ghbrk` and `/var/log/ghbrk`. All other hardening directives are unchanged.

`PrivateTmp=` is absent from the unit, and that absence is a requirement rather than an omission. The daemon spawns `git` and `gh` as children of the requesting user, and those children open the file paths the caller wrote in the command line: `gh release create v1.0.0 /tmp/app.tar.gz` uploads an asset from `/tmp`, and `gh pr comment --body-file /tmp/body.md` reads a body from `/tmp`. `PrivateTmp=true` gives the daemon a mount namespace whose `/tmp` is a fresh empty directory, so the child sees no such file and `gh` reports `no such file or directory` for a path the caller can list. Nothing in that error names the namespace, which makes the misconfiguration expensive to diagnose; `cli/doctor` therefore compares the daemon's view of `/tmp` against the caller's and names this directive in its remediation.

The requirement binds every unit this repository can place on a host, and two installers place one. `deploy/linux/install.sh` copies `deploy/linux/ghbrk.service` to `/etc/systemd/system/ghbrk.service` and reloads systemd. The repository-root `install.sh` — the installer `README.md` and `docs/install.md` tell users to pipe into `sudo bash` — runs from a downloaded binary with no repository checkout, so it writes the unit from an embedded heredoc instead of copying anything. Both outputs MUST omit `PrivateTmp=`, and the heredoc MUST emit the same `[Service]` directives as `deploy/linux/ghbrk.service`, so the two installers cannot drift into producing different hardening. A host whose installed unit predates this requirement keeps its private `/tmp` until the operator removes the directive from `/etc/systemd/system/ghbrk.service` and restarts the service; reinstalling from a release that still carries the directive reproduces it.

## Scenarios

### Scenario: systemd unit has hardening directives

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *WHEN* an operator inspects it
* *THEN* the unit MUST include at minimum `ProtectSystem=strict` and `NoNewPrivileges=true`
* *AND* the unit MUST scope its capability set to exactly `CAP_SETUID` and `CAP_SETGID` via `CapabilityBoundingSet`
* *AND* the unit MUST NOT include `PrivateTmp=` in any form, because a private `/tmp` namespace hides caller-named file arguments from the spawned child

### Scenario: socket parent directory is on the host filesystem

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *AND* the tmpfiles snippet `deploy/linux/ghbrk.tmpfiles`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST NOT contain `RuntimeDirectory=`
* *AND* the unit MUST include `ReadWritePaths=` with `/run/ghbrk` so the daemon can write the socket under `ProtectSystem=strict`
* *AND* `deploy/linux/ghbrk.tmpfiles` MUST declare `d /run/ghbrk 2750 ghbrk ghbrk-clients` so systemd recreates the directory on every boot

### Scenario: systemd unit starts the daemon as the ghbrk user

* *GIVEN* the systemd unit `deploy/linux/ghbrk.service` is installed
* *WHEN* an operator runs `systemctl start ghbrk`
* *THEN* the unit MUST start `/usr/local/bin/ghbrk daemon`
* *AND* the unit MUST run as `User=ghbrk`
* *AND* the unit MUST have `Group=ghbrk-clients`

### Scenario: systemd unit grants the privilege-drop capabilities

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST include `AmbientCapabilities=CAP_SETUID CAP_SETGID` so the daemon process retains the capability to change to the requesting user
* *AND* the unit MUST include `CapabilityBoundingSet=CAP_SETUID CAP_SETGID` so no other capability can be acquired

### Scenario: systemd unit keeps NoNewPrivileges alongside the privilege-drop capabilities

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *AND* the unit grants `CAP_SETUID` and `CAP_SETGID` via `AmbientCapabilities`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST retain `NoNewPrivileges=true`
* *AND* `NoNewPrivileges=true` MUST NOT block the daemon's `setuid(2)`/`setgid(2)` syscalls, because the capability is already held and no SUID transition is involved
* *AND* `NoNewPrivileges=true` MUST continue to prevent the daemon from gaining privilege by executing SUID binaries

### Scenario: systemd unit allows the child to write under user home directories

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST set `ProtectHome=no`
* *AND* the unit MUST NOT set `ProtectHome=read-only`
* *AND* the rationale MUST be that child processes spawned as the requesting user need write access to repositories under that user's home directory for `git fetch`/`git pull`

### Scenario: systemd unit grants write access to the policy directory

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *AND* the daemon is the sole writer of the policy file at the default `GHBRK_POLICY` path `/etc/ghbrk/policy.yaml`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST include `/etc/ghbrk` in its `ReadWritePaths=` directive so the broker can atomically rewrite `policy.yaml` under `ProtectSystem=strict`
* *AND* the unit MUST retain `/run/ghbrk` and `/var/log/ghbrk` in `ReadWritePaths=`, because the `/etc/ghbrk` entry is additive and narrowly scoped to the ghbrk-owned config directory, consistent with the existing narrow-whitelist precedent rather than a blanket `/etc` grant
* *AND* the unit MUST NOT relax `ProtectSystem=strict` or any other hardening directive to achieve the write, because widening one owner-restricted subdirectory (`ghbrk:ghbrk`, `policy.yaml` mode `0600`) is sufficient and does not weaken `/etc` hardening elsewhere

### Scenario: unit file carries the rationale for the absent PrivateTmp directive

* *GIVEN* the systemd unit file `deploy/linux/ghbrk.service`
* *WHEN* an operator inspects the `[Service]` section
* *THEN* the unit MUST carry a comment explaining that `PrivateTmp=` is omitted so spawned children can read caller-named files under the shared `/tmp`
* *AND* the comment MUST name at least one affected operation, so a later editor does not re-add the directive as a hardening improvement

### Scenario: Root installer emits a unit with no PrivateTmp directive

* *GIVEN* the repository-root `install.sh`, which writes `/etc/systemd/system/ghbrk.service` from an embedded heredoc instead of copying `deploy/linux/ghbrk.service`
* *WHEN* an operator runs it as the documented `curl … | sudo bash` one-liner
* *THEN* the emitted unit MUST NOT include `PrivateTmp=` in any form
* *AND* `install.sh` MUST NOT contain the text `PrivateTmp=` anywhere in the file
* *AND* the emitted unit MUST carry the same comment explaining the omission that `deploy/linux/ghbrk.service` carries

### Scenario: Both installers emit the same service directives

* *GIVEN* the repository-root `install.sh` heredoc and the unit file `deploy/linux/ghbrk.service`
* *WHEN* the `Key=Value` directive lines of each `[Service]` section are compared, dropping blank lines and whole-line comments and stripping any trailing `#` comment from each remaining line
* *THEN* the two directive sets MUST be equal
* *AND* each installer MUST NOT emit a directive line carrying a trailing `#` comment, because systemd reads one as part of the value rather than as a comment
* *AND* the emitted unit MUST therefore include `/etc/ghbrk` in `ReadWritePaths=`, matching the policy-directory grant the repository unit already carries

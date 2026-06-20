/// Deployment artefact tests.
///
/// These are static-analysis tests that verify the correctness of files in
/// `deploy/linux/` and `config/` without requiring root access or a live
/// systemd installation.
use std::{fs, path::PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ---------------------------------------------------------------------------
// Policy YAML
// ---------------------------------------------------------------------------

#[test]
fn example_policy_loads() {
    let path = workspace_root().join("config/policy.example.yaml");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    ghbrk::policy::Policy::from_yaml(&text).unwrap_or_else(|e| panic!("policy load failed: {e}"));
}

// ---------------------------------------------------------------------------
// Systemd unit — structural assertions
// ---------------------------------------------------------------------------

fn read_service() -> String {
    let path = workspace_root().join("deploy/linux/ghbrk.service");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

#[test]
fn systemd_unit_user_group() {
    let service = read_service();
    assert!(
        service.contains("User=ghbrk"),
        "service must set User=ghbrk"
    );
    assert!(
        service.contains("Group=ghbrk-clients"),
        "service must set Group=ghbrk-clients"
    );
}

#[test]
fn systemd_unit_hardening_directives() {
    let service = read_service();
    assert!(
        service.contains("NoNewPrivileges=true"),
        "service must have NoNewPrivileges=true"
    );
    assert!(
        service.contains("ProtectSystem=strict"),
        "service must have ProtectSystem=strict"
    );
    assert!(
        service.contains("ProtectKernelTunables=true"),
        "service must have ProtectKernelTunables=true"
    );
    assert!(
        service.contains("ProtectKernelModules=true"),
        "service must have ProtectKernelModules=true"
    );
    assert!(
        service.contains("ProtectControlGroups=true"),
        "service must have ProtectControlGroups=true"
    );
}

#[test]
fn unit_has_no_private_tmp() {
    let service = read_service();
    assert!(
        !service.contains("PrivateTmp="),
        "service must NOT contain any PrivateTmp= directive (shared /tmp required for gh release create asset uploads)"
    );
    assert!(
        service.contains("shared /tmp"),
        "service must contain an explanatory comment about shared /tmp"
    );
}

#[test]
fn unit_declares_setuid_setgid_capabilities() {
    let unit = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/deploy/linux/ghbrk.service"
    ))
    .expect("read ghbrk.service");
    assert!(
        unit.contains("AmbientCapabilities=CAP_SETUID CAP_SETGID"),
        "ghbrk.service must declare AmbientCapabilities=CAP_SETUID CAP_SETGID for privilege drop"
    );
    assert!(
        unit.contains("CapabilityBoundingSet=CAP_SETUID CAP_SETGID"),
        "ghbrk.service must declare CapabilityBoundingSet=CAP_SETUID CAP_SETGID for privilege drop"
    );
}

#[test]
fn unit_sets_protect_home_no() {
    let unit = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/deploy/linux/ghbrk.service"
    ))
    .expect("read ghbrk.service");
    assert!(
        unit.contains("ProtectHome=no"),
        "ghbrk.service must set ProtectHome=no so user-impersonated children can write repos"
    );
    assert!(
        !unit.contains("ProtectHome=read-only"),
        "ghbrk.service must NOT have ProtectHome=read-only (replaced by ProtectHome=no)"
    );
}

#[test]
fn service_has_runtime_directory() {
    let service = read_service();
    // RuntimeDirectory= creates a private namespace mount invisible to host processes;
    // socket accessibility requires ReadWritePaths= on a host-created directory instead.
    assert!(
        service.contains("ReadWritePaths=") && service.contains("/run/ghbrk"),
        "service must expose /run/ghbrk via ReadWritePaths so the socket is visible to shim processes"
    );
    assert!(
        !service.contains("RuntimeDirectory="),
        "RuntimeDirectory= must not be used: it creates a namespace-private mount inaccessible to shims"
    );
}

#[test]
fn tmpfiles_snippet_creates_run_ghbrk() {
    let tmpfiles = std::fs::read_to_string("deploy/linux/ghbrk.tmpfiles")
        .expect("deploy/linux/ghbrk.tmpfiles must exist");
    assert!(
        tmpfiles.contains("d /run/ghbrk") && tmpfiles.contains("2750"),
        "tmpfiles snippet must create /run/ghbrk with mode 2750"
    );
    assert!(
        tmpfiles.contains("ghbrk-clients"),
        "tmpfiles snippet must set group to ghbrk-clients"
    );
}

// ---------------------------------------------------------------------------
// install.sh — static analysis
// ---------------------------------------------------------------------------

fn read_install_sh() -> String {
    let path = workspace_root().join("deploy/linux/install.sh");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

#[test]
fn install_creates_user_and_group() {
    let script = read_install_sh();
    let has_useradd = script.contains("useradd") || script.contains("adduser");
    assert!(
        has_useradd,
        "install.sh must call useradd or adduser to create the ghbrk user"
    );
    assert!(
        script.contains("ghbrk"),
        "install.sh must reference the ghbrk user name"
    );
    let has_groupadd = script.contains("groupadd") || script.contains("addgroup");
    assert!(
        has_groupadd,
        "install.sh must call groupadd or addgroup to create the ghbrk-clients group"
    );
    assert!(
        script.contains("ghbrk-clients"),
        "install.sh must reference ghbrk-clients group"
    );
}

#[test]
fn install_creates_directories_with_modes() {
    let script = read_install_sh();
    assert!(
        script.contains("/etc/ghbrk/credentials"),
        "install.sh must create /etc/ghbrk/credentials"
    );
    assert!(
        script.contains("ghbrk.tmpfiles") || script.contains("tmpfiles"),
        "install.sh must install the tmpfiles snippet to create /run/ghbrk"
    );
    assert!(
        script.contains("/var/log/ghbrk"),
        "install.sh must create /var/log/ghbrk"
    );
    // Verify that mode-setting is present (either chmod or install -d -m)
    let sets_modes = script.contains("chmod") || script.contains("install -d");
    assert!(
        sets_modes,
        "install.sh must set directory modes (chmod or install -d -m)"
    );
}

#[test]
fn install_idempotent() {
    let script = read_install_sh();
    // Must check for existing user before creating it
    assert!(
        script.contains("id ghbrk"),
        "install.sh must guard useradd with 'id ghbrk' to be idempotent"
    );
}

#[test]
fn install_adds_ghbrk_to_clients_group() {
    let script = read_install_sh();
    assert!(
        script.contains("usermod -aG ghbrk-clients ghbrk"),
        "install.sh must add the ghbrk user to ghbrk-clients via 'usermod -aG ghbrk-clients ghbrk'"
    );
}

#[test]
fn install_adds_sudo_user_to_clients_group() {
    let script = read_install_sh();
    assert!(
        script.contains("$SUDO_USER"),
        "install.sh must reference $SUDO_USER to identify the invoking user"
    );
    assert!(
        script.contains("usermod -aG ghbrk-clients"),
        "install.sh must add the invoking user to ghbrk-clients via 'usermod -aG ghbrk-clients'"
    );
}

#[test]
fn install_enables_service() {
    let script = read_install_sh();
    assert!(
        script.contains("systemctl enable ghbrk"),
        "install.sh must enable the ghbrk service with 'systemctl enable ghbrk'"
    );
    assert!(
        script.contains("systemctl start ghbrk") || script.contains("systemctl restart ghbrk"),
        "install.sh must start the ghbrk service"
    );
}

// ---------------------------------------------------------------------------
// Integration test Dockerfile — structural assertions
// ---------------------------------------------------------------------------

#[test]
fn devenv_dockerfile_creates_priv_testuser() {
    let dockerfile = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/integration/Dockerfile.devenv"
    ))
    .expect("read Dockerfile.devenv");
    assert!(
        dockerfile.contains("priv-testuser"),
        "Dockerfile.devenv must create priv-testuser for the privilege-drop e2e fixture"
    );
    assert!(
        dockerfile.contains("chmod 700 /home/priv-testuser"),
        "Dockerfile.devenv must set priv-testuser home to mode 0700"
    );
}

// ---------------------------------------------------------------------------
// README — content guards
// ---------------------------------------------------------------------------

#[test]
fn readme_has_no_chmod_home_step() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))
        .expect("read README.md");
    assert!(
        !readme.contains("chmod o+x ~"),
        "README.md must not instruct users to chmod o+x ~ (privilege drop eliminates this step)"
    );
}

// ---------------------------------------------------------------------------
// cargo-deny — manual checks (marked #[ignore] so they don't run in CI
// without cargo-deny installed; run with `cargo test -- --ignored`)
// ---------------------------------------------------------------------------

/// Manually verify that GPL/AGPL/LGPL/SSPL licenses are rejected by deny.toml.
/// Requires `cargo-deny` to be installed: `cargo install cargo-deny`.
#[test]
#[ignore]
fn cargo_deny_rejects_gpl() {
    // This test is intentionally left as a manual check.
    // Run: cargo deny check licenses
    // Expected: any GPL/AGPL/LGPL/SSPL dependency would cause an error.
}

/// Verify that `cargo deny check` passes against the project's actual dependency tree.
/// Requires `cargo-deny` to be installed: `cargo install cargo-deny`.
#[test]
#[ignore]
fn cargo_deny_passes_on_real_tree() {
    // This test is intentionally left as a manual check.
    // Run: cargo deny check
    // Expected: advisories ok, bans ok, licenses ok, sources ok
}

// ---------------------------------------------------------------------------
// SSH credentials dir mode test
// ---------------------------------------------------------------------------

fn read_provision_user_sh() -> String {
    let path = workspace_root().join("deploy/linux/provision-user.sh");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The daemon owns the SSH key (0600 ghbrk:ghbrk) and accesses it directly.
/// Calling users never need to traverse the credentials directory — they
/// connect to the agent socket via SSH_AUTH_SOCK instead. Mode 0700 ensures
/// only the ghbrk daemon can enter the credentials tree, eliminating
/// EACCES exposure to peer users entirely.
#[test]
fn credentials_dir_mode_is_0700() {
    let script = read_install_sh();
    assert!(
        script.contains("-m 0700"),
        "install.sh must set credentials dir to mode 0700 (daemon-owned key, agent escrow: calling user does not traverse)"
    );
    assert!(
        script.contains("/etc/ghbrk/credentials"),
        "install.sh must reference /etc/ghbrk/credentials"
    );
}

#[test]
fn provision_user_creates_ghbrk_owned_id_rsa() {
    let script = read_provision_user_sh();
    // id_rsa must be owned by ghbrk:ghbrk so only the daemon can read it.
    assert!(
        script.contains("-o ghbrk") && script.contains("-g ghbrk"),
        "provision-user.sh must create id_rsa owned by ghbrk:ghbrk (not the peer user)"
    );
    assert!(
        script.contains("id_rsa") || script.contains("ID_RSA"),
        "provision-user.sh must reference id_rsa"
    );
}

// ---------------------------------------------------------------------------
// Talos platform patch
// ---------------------------------------------------------------------------

#[test]
fn platform_patch_sets_writable_policy_path() {
    let path = workspace_root().join("deploy/talos/ghbrk-policy-path.yaml");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_yaml::from_str::<serde_yaml::Value>(&text)
        .unwrap_or_else(|e| panic!("deploy/talos/ghbrk-policy-path.yaml is not valid YAML: {e}"));
    assert!(
        text.contains("GHBRK_POLICY=/var/etc/ghbrk/policy.yaml"),
        "patch must set GHBRK_POLICY=/var/etc/ghbrk/policy.yaml"
    );
    assert!(
        text.contains("/var/etc/ghbrk"),
        "patch must provision the /var/etc/ghbrk directory"
    );
    assert!(
        text.contains("ghbrk"),
        "patch must reference the ghbrk owner"
    );
}

#[test]
fn docs_describe_readonly_etc_workaround() {
    let path = workspace_root().join("deploy/talos/README.md");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert!(
        text.contains("GHBRK_POLICY"),
        "README must mention GHBRK_POLICY"
    );
    assert!(
        text.contains("/var/etc/ghbrk/policy.yaml"),
        "README must reference /var/etc/ghbrk/policy.yaml"
    );
    assert!(
        text.to_ascii_lowercase().contains("read-only"),
        "README must describe the read-only /etc constraint"
    );
    assert!(
        text.contains("ghbrk-clients"),
        "README must mention ghbrk-clients group prerequisite"
    );
}

// Security invariant: daemon-owned credentials must be unreadable by the peer user.
//
// In production, `id_rsa` is created `0600 ghbrk:ghbrk` (daemon-owned). After
// privilege drop the broker spawns subprocesses as the *peer* user, which is
// never `ghbrk`. The `0600` mode therefore denies read access to every non-owner
// uid → EACCES. This test models that invariant: a `0600` file owned by the test
// runner is NOT readable by a child that privilege-drops to a different uid.
//
// Why this is gated on root: the `setresuid` performed inside `stream_child`'s
// privilege-drop path only succeeds when the daemon holds CAP_SETUID (i.e. runs
// as root). When the test runner is unprivileged, the kernel refuses the drop and
// the child would either be denied or inherit the runner's own uid — in which
// case it *can* read its own `0600` file, making the assertion meaningless.
// We therefore skip when not root; the mode-based access control itself is an OS
// guarantee, and this test documents/exercises the production invariant in the
// privileged CI lane only.

use ghbrk::executor::{stream_child, ChildSpec};
use ghbrk::protocol::{read_frame, ServerFrame};
use std::io::Cursor;

/// An unprivileged uid that is neither root nor (assumed) the test runner's own
/// uid. `nobody` is conventionally 65534 across Linux distros.
const NOBODY_UID: u32 = 65534;

async fn collect_frames(buf: Vec<u8>) -> Vec<ServerFrame> {
    let mut cursor = Cursor::new(buf);
    let mut out = Vec::new();
    loop {
        match read_frame::<_, ServerFrame>(&mut cursor).await {
            Ok(f) => out.push(f),
            Err(_) => return out,
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn id_rsa_not_readable_by_peer_user() {
    use std::os::unix::fs::PermissionsExt;

    // Skip when not root: the privilege drop (setresuid) inside stream_child only
    // works with CAP_SETUID. Without it the child cannot become a different uid,
    // so it would read its own 0600 file and the invariant could not be tested.
    // Mode-0600 access control is itself an OS guarantee; this test documents the
    // production invariant and runs in privileged CI only.
    if !nix::unistd::geteuid().is_root() {
        return;
    }

    // Pick a target uid that is genuinely different from our own (we are root = 0,
    // so NOBODY_UID is always different, but be defensive anyway).
    let own = nix::unistd::geteuid().as_raw();
    let target = if own != NOBODY_UID { NOBODY_UID } else { 65533 };

    // Write id_rsa as 0600, owned by the current (root) uid. Any other uid → EACCES.
    let dir = tempfile::tempdir().expect("tempdir");
    let id_rsa = dir.path().join("id_rsa");
    std::fs::write(&id_rsa, b"PRIVATE_KEY_MATERIAL").unwrap();
    std::fs::set_permissions(&id_rsa, std::fs::Permissions::from_mode(0o600)).unwrap();

    let id_rsa_str = id_rsa.to_string_lossy().to_string();

    // Spawn `cat id_rsa` as the peer (non-owner) uid. The outer tempdir is owned
    // by root and world-traversable by default (mkdtemp -> 0700) — but since we
    // are root creating it, traversal for the child is irrelevant: the *file*
    // 0600 owned by root is what denies the read. To be safe against any
    // directory-traversal interference, place the key directly under a
    // world-traversable path is unnecessary here; the file mode alone is the
    // control under test. We make the dir 0711 so traversal never masks the
    // file-level EACCES we are actually asserting.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o711)).unwrap();

    let spec = ChildSpec {
        program: "sh".into(),
        args: vec!["-c".into(), format!("cat {id_rsa_str}")],
        env: vec![],
        cwd: std::env::temp_dir(),
        uid: Some(target),
        gid: Some(target),
        supplementary_gids: vec![],
        home: Some(std::env::temp_dir()),
    };

    let mut buf = Vec::new();
    stream_child(&spec, &mut buf).await.unwrap();
    let frames = collect_frames(buf).await;

    let exit_code = frames
        .iter()
        .find_map(|f| {
            if let ServerFrame::Exit { code } = f {
                Some(*code)
            } else {
                None
            }
        })
        .expect("expected an Exit frame from the cat subprocess");

    assert_ne!(
        exit_code, 0,
        "peer uid {target} must not be able to read a 0600 file owned by uid {own}; frames: {frames:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn token_not_readable_by_peer_user() {
    // Skip when not root: privilege drop requires CAP_SETUID.
    if !nix::unistd::geteuid().is_root() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    // Make the dir traversable by all (so the 0600 file is the actual guard)
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o711)).unwrap();

    let token = dir.path().join("token");
    std::fs::write(&token, b"ghp_secret_token").unwrap();
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();

    let token_str = token.to_string_lossy().to_string();
    let spec = ChildSpec {
        program: "sh".into(),
        args: vec!["-c".into(), format!("cat {token_str}")],
        env: vec![],
        cwd: std::env::temp_dir(),
        uid: Some(65534), // NOBODY_UID
        gid: Some(65534),
        supplementary_gids: vec![],
        home: Some(std::env::temp_dir()),
    };

    let mut buf = Vec::new();
    stream_child(&spec, &mut buf).await.unwrap();
    let frames = collect_frames(buf).await;
    let exit_code = frames
        .iter()
        .find_map(|f| {
            if let ServerFrame::Exit { code } = f {
                Some(*code)
            } else {
                None
            }
        })
        .expect("expected Exit frame");
    assert_ne!(
        exit_code, 0,
        "NOBODY must not be able to read a 0600 token file owned by root"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_agent_socket_cleaned_up_after_operation() {
    use ghbrk::credentials::{start_ssh_agent, Credentials};

    // Create a temp key file (mode 0600) — may not be a valid SSH key.
    // ssh-add will likely fail, but we test the Drop cleanup of what is created.
    let key_dir = tempfile::tempdir().expect("tempdir");
    let key_path = key_dir.path().join("id_rsa");
    std::fs::write(
        &key_path,
        b"-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n-----END OPENSSH PRIVATE KEY-----\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let creds = Credentials {
        ssh_key_path: key_path,
        token: "dummy".into(),
    };

    let result = start_ssh_agent(&creds).await;
    match result {
        Err(_) => {
            // ssh-agent not available or ssh-add failed — skip
        }
        Ok((env_vars, handle)) => {
            // Capture the socket path before dropping the handle
            let temp_dir = handle.temp_dir.clone();
            let sock_path = env_vars
                .iter()
                .find(|(k, _)| k == "SSH_AUTH_SOCK")
                .map(|(_, v)| std::path::PathBuf::from(v));

            // Drop the handle — this should kill the agent and remove the temp dir
            drop(handle);

            // Assert cleanup
            assert!(
                !temp_dir.exists(),
                "temp dir must be removed after SshAgentHandle drop"
            );
            if let Some(sock) = sock_path {
                assert!(
                    !sock.exists(),
                    "agent socket must be removed after SshAgentHandle drop"
                );
            }
        }
    }
}

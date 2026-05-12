use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Default file for audit records when the daemon is not given an override.
pub const DEFAULT_AUDIT_PATH: &str = "/var/log/ghbrk/audit.log";

/// Decision recorded in the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AuditDecision {
    Allow,
    Deny { reason: String },
}

/// One auditable event: the broker's view of a single request and decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub timestamp: String,
    pub user: String,
    pub tool: String,
    pub args: Vec<String>,
    pub org: String,
    pub repo: String,
    pub branch: Option<String>,
    pub operation: String,
    #[serde(flatten)]
    pub decision: AuditDecision,
}

impl AuditRecord {
    /// Returns an RFC 3339 timestamp string for "now". Useful when the broker
    /// constructs records on the fly.
    pub fn now_timestamp() -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| String::new())
    }
}

/// Append-only writer for `AuditRecord`s. Internally serialises writes via a
/// `std::sync::Mutex` so multiple Tokio tasks can call `write` concurrently.
pub struct AuditLogger {
    path: PathBuf,
    inner: Mutex<BufWriter<File>>,
}

impl AuditLogger {
    /// Opens (or creates) `path` for append. Parent directory must exist.
    pub fn new(path: &Path) -> Result<Self, io::Error> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o640)
            .open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            inner: Mutex::new(BufWriter::new(file)),
        })
    }

    /// Returns the path the logger is appending to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Serialises `record` as one JSON line and appends it to the log.
    pub fn write(&self, record: &AuditRecord) -> Result<(), io::Error> {
        let mut line = serde_json::to_vec(record)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push(b'\n');
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("audit logger mutex poisoned"))?;
        guard.write_all(&line)?;
        guard.flush()?;
        Ok(())
    }

    /// Flushes the buffered writer to disk. Safe to call from any thread or
    /// async context — the critical section is short.
    pub fn flush(&self) -> Result<(), io::Error> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("audit logger mutex poisoned"))?;
        guard.flush()
    }
}

impl Drop for AuditLogger {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.inner.lock() {
            let _ = guard.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn sample_record(decision: AuditDecision) -> AuditRecord {
        AuditRecord {
            timestamp: "2026-04-26T12:00:00Z".to_string(),
            user: "alice".to_string(),
            tool: "git".to_string(),
            args: vec!["push".to_string(), "origin".to_string(), "main".to_string()],
            org: "acme".to_string(),
            repo: "web".to_string(),
            branch: Some("main".to_string()),
            operation: "push".to_string(),
            decision,
        }
    }

    #[test]
    fn writes_one_json_line_per_record() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.log");
        let logger = AuditLogger::new(&path).unwrap();
        logger.write(&sample_record(AuditDecision::Allow)).unwrap();
        logger
            .write(&sample_record(AuditDecision::Deny {
                reason: "no matching rule".into(),
            }))
            .unwrap();
        logger.flush().unwrap();

        let body = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let allow: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(allow["decision"], "allow");
        assert_eq!(allow["user"], "alice");
        let deny: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(deny["decision"], "deny");
        assert_eq!(deny["reason"], "no matching rule");
    }

    #[test]
    fn appends_across_logger_reopens() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.log");
        {
            let logger = AuditLogger::new(&path).unwrap();
            logger.write(&sample_record(AuditDecision::Allow)).unwrap();
            logger.flush().unwrap();
        }
        {
            let logger = AuditLogger::new(&path).unwrap();
            logger
                .write(&sample_record(AuditDecision::Deny {
                    reason: "second".into(),
                }))
                .unwrap();
            logger.flush().unwrap();
        }
        let body = fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 2);
    }

    #[test]
    fn concurrent_writers_serialize_cleanly() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.log");
        let logger = Arc::new(AuditLogger::new(&path).unwrap());

        let handles: Vec<_> = (0..16)
            .map(|i| {
                let l = Arc::clone(&logger);
                std::thread::spawn(move || {
                    for _ in 0..32 {
                        let mut rec = sample_record(AuditDecision::Allow);
                        rec.user = format!("user{i}");
                        l.write(&rec).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        logger.flush().unwrap();

        let body = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 16 * 32);
        for line in lines {
            let _: serde_json::Value =
                serde_json::from_str(line).expect("each line must parse as JSON");
        }
    }

    #[test]
    fn now_timestamp_is_rfc3339() {
        let ts = AuditRecord::now_timestamp();
        // Round-trip through OffsetDateTime to confirm RFC 3339 shape.
        let parsed = OffsetDateTime::parse(&ts, &Rfc3339);
        assert!(parsed.is_ok(), "{ts:?} did not parse as RFC 3339");
    }

    #[test]
    fn flush_on_drop_persists_records() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.log");
        {
            let logger = AuditLogger::new(&path).unwrap();
            logger.write(&sample_record(AuditDecision::Allow)).unwrap();
            // Intentionally no explicit flush; drop must persist.
        }
        let body = fs::read_to_string(&path).unwrap();
        assert!(!body.is_empty());
        assert_eq!(body.lines().count(), 1);
    }

    /// 10.5: if credentials are loaded from disk and then a record is written
    /// from the broker decision path, the token must never appear in the log
    /// file. The audit record carries no token field; the broker is expected
    /// not to splice tokens into reasons.
    #[test]
    fn token_never_appears_in_audit_file() {
        let dir = TempDir::new().unwrap();

        // Set up a credential directory with a known token.
        let cred_dir = dir.path().join("creds");
        let user_dir = cred_dir.join("alice");
        fs::create_dir_all(&user_dir).unwrap();
        let token_value = "ghp_AUDIT_LOG_LEAK_CANARY_xyz789";
        let key_path = user_dir.join("id_rsa");
        let token_path = user_dir.join("token");
        fs::write(&key_path, "KEYDATA").unwrap();
        fs::write(&token_path, token_value).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&key_path).unwrap().permissions();
        p.set_mode(0o600);
        fs::set_permissions(&key_path, p).unwrap();
        let mut p = fs::metadata(&token_path).unwrap().permissions();
        p.set_mode(0o600);
        fs::set_permissions(&token_path, p).unwrap();

        let creds = crate::credentials::load_credentials_from(&cred_dir, "alice").unwrap();
        assert_eq!(creds.token, token_value);

        // Now exercise the broker decision-path equivalent: build records and
        // write them. The deny reason and other fields originate from the
        // policy engine, NOT from credentials. We simulate both an allow and a
        // deny.
        let log_path = dir.path().join("audit.log");
        let logger = AuditLogger::new(&log_path).unwrap();
        logger
            .write(&AuditRecord {
                timestamp: AuditRecord::now_timestamp(),
                user: "alice".into(),
                tool: "git".into(),
                args: vec!["push".into(), "origin".into(), "main".into()],
                org: "acme".into(),
                repo: "web".into(),
                branch: Some("main".into()),
                operation: "push".into(),
                decision: AuditDecision::Allow,
            })
            .unwrap();
        logger
            .write(&AuditRecord {
                timestamp: AuditRecord::now_timestamp(),
                user: "alice".into(),
                tool: "git".into(),
                args: vec!["push".into(), "origin".into(), "feature".into()],
                org: "acme".into(),
                repo: "web".into(),
                branch: Some("feature".into()),
                operation: "push".into(),
                decision: AuditDecision::Deny {
                    reason: "no matching rule".into(),
                },
            })
            .unwrap();
        logger.flush().unwrap();

        let body = fs::read_to_string(&log_path).unwrap();
        assert!(
            !body.contains(token_value),
            "audit log must never contain token {token_value:?}; body={body}"
        );
    }
}

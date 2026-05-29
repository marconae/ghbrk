use std::io;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const WILDCARD: &str = "*";

/// Operations that ghbrk recognises. Maps to YAML strings via snake_case.
///
/// Every operation serialises to a bare snake_case tag — including
/// `GhApiRead`, which serialises to `gh_api_read` and discards its `path`. A
/// policy rule names operation *kinds*; the request-side `path` payload is not
/// part of the policy vocabulary, so matching ignores it (see `same_kind`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Operation {
    Push,
    Fetch,
    Pull,
    Clone,
    PrOpen,
    PrComment,
    PrClose,
    PrMerge,
    PrReview,
    IssueOpen,
    IssueComment,
    IssueClose,
    ReleaseCreate,
    GhApiRead { path: String },
}

impl Operation {
    /// True if this operation operates on a specific branch and therefore
    /// should be matched against the rule's branch patterns.
    pub fn has_branch(&self) -> bool {
        matches!(self, Operation::Push)
    }

    /// The bare snake_case tag for this operation, ignoring any payload.
    fn tag(&self) -> &'static str {
        match self {
            Operation::Push => "push",
            Operation::Fetch => "fetch",
            Operation::Pull => "pull",
            Operation::Clone => "clone",
            Operation::PrOpen => "pr_open",
            Operation::PrComment => "pr_comment",
            Operation::PrClose => "pr_close",
            Operation::PrMerge => "pr_merge",
            Operation::PrReview => "pr_review",
            Operation::IssueOpen => "issue_open",
            Operation::IssueComment => "issue_comment",
            Operation::IssueClose => "issue_close",
            Operation::ReleaseCreate => "release_create",
            Operation::GhApiRead { .. } => "gh_api_read",
        }
    }

    fn from_tag(tag: &str) -> Option<Operation> {
        let op = match tag {
            "push" => Operation::Push,
            "fetch" => Operation::Fetch,
            "pull" => Operation::Pull,
            "clone" => Operation::Clone,
            "pr_open" => Operation::PrOpen,
            "pr_comment" => Operation::PrComment,
            "pr_close" => Operation::PrClose,
            "pr_merge" => Operation::PrMerge,
            "pr_review" => Operation::PrReview,
            "issue_open" => Operation::IssueOpen,
            "issue_comment" => Operation::IssueComment,
            "issue_close" => Operation::IssueClose,
            "release_create" => Operation::ReleaseCreate,
            "gh_api_read" => Operation::GhApiRead {
                path: String::new(),
            },
            _ => return None,
        };
        Some(op)
    }

    /// True when two operations name the same kind, ignoring any payload such
    /// as `GhApiRead`'s `path`. Policy rules match on kind, not payload.
    fn same_kind(&self, other: &Operation) -> bool {
        self.tag() == other.tag()
    }
}

impl Serialize for Operation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.tag())
    }
}

impl<'de> Deserialize<'de> for Operation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let tag = String::deserialize(deserializer)?;
        Operation::from_tag(&tag)
            .ok_or_else(|| serde::de::Error::unknown_variant(&tag, &["push", "..."]))
    }
}

/// Allow or deny effect of a matched rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Deny,
}

/// Single policy rule. Field order in the YAML mirrors firewall conventions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub user: String,
    pub org: String,
    pub repo: String,
    pub operations: Vec<Operation>,
    #[serde(default = "default_branches")]
    pub branches: Vec<String>,
    pub effect: Effect,
}

fn default_branches() -> Vec<String> {
    vec![WILDCARD.to_string()]
}

/// Top-level policy document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub rules: Vec<Rule>,
}

/// Inputs the engine matches against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request<'a> {
    pub user: &'a str,
    pub org: &'a str,
    pub repo: &'a str,
    pub operation: Operation,
    pub branch: Option<&'a str>,
}

/// Outcome of an evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny { reason: String },
}

/// Errors loading or validating a policy document.
#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("rule {index} has empty operations list")]
    EmptyOperations { index: usize },
    #[error("rule {index} has invalid branch glob '{pattern}': {source}")]
    InvalidBranchGlob {
        index: usize,
        pattern: String,
        #[source]
        source: glob::PatternError,
    },
}

impl Policy {
    /// Parse a YAML document and validate every rule strictly.
    pub fn from_yaml(text: &str) -> Result<Self, PolicyError> {
        let policy: Policy = serde_yaml::from_str(text)?;
        policy.validate()?;
        Ok(policy)
    }

    /// Read a YAML reader and validate.
    pub fn from_reader<R: io::Read>(reader: R) -> Result<Self, PolicyError> {
        let policy: Policy = serde_yaml::from_reader(reader)?;
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> Result<(), PolicyError> {
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.operations.is_empty() {
                return Err(PolicyError::EmptyOperations { index });
            }
            for pattern in &rule.branches {
                glob::Pattern::new(pattern).map_err(|source| PolicyError::InvalidBranchGlob {
                    index,
                    pattern: pattern.clone(),
                    source,
                })?;
            }
        }
        Ok(())
    }

    /// Evaluate `request` against the rules in document order, returning the
    /// first matching rule's effect, or default-deny when none match.
    pub fn evaluate(&self, request: &Request<'_>) -> Decision {
        for rule in &self.rules {
            if rule_matches(rule, request) {
                return match rule.effect {
                    Effect::Allow => Decision::Allow,
                    Effect::Deny => Decision::Deny {
                        reason: format!("denied by policy rule for {}/{}", rule.org, rule.repo),
                    },
                };
            }
        }
        Decision::Deny {
            reason: "no matching rule".to_string(),
        }
    }
}

fn field_matches(pattern: &str, value: &str) -> bool {
    pattern == WILDCARD || pattern == value
}

fn rule_matches(rule: &Rule, request: &Request<'_>) -> bool {
    if !field_matches(&rule.user, request.user) {
        return false;
    }
    if !field_matches(&rule.org, request.org) {
        return false;
    }
    if !field_matches(&rule.repo, request.repo) {
        return false;
    }
    if !rule
        .operations
        .iter()
        .any(|op| op.same_kind(&request.operation))
    {
        return false;
    }
    if !branch_matches(&rule.branches, request) {
        return false;
    }
    true
}

fn branch_matches(branches: &[String], request: &Request<'_>) -> bool {
    if !request.operation.has_branch() {
        return true;
    }
    let Some(branch) = request.branch else {
        return branches.iter().any(|p| p == WILDCARD);
    };
    branches.iter().any(|pattern| {
        glob::Pattern::new(pattern)
            .map(|p| p.matches(branch))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(
        user: &'a str,
        org: &'a str,
        repo: &'a str,
        operation: Operation,
        branch: Option<&'a str>,
    ) -> Request<'a> {
        Request {
            user,
            org,
            repo,
            operation,
            branch,
        }
    }

    #[test]
    fn allow_exact_match() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [push]
    branches: [main]
    effect: allow
"#,
        )
        .unwrap();
        let decision = policy.evaluate(&req("alice", "acme", "web", Operation::Push, Some("main")));
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn default_deny_when_no_rule_matches() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [push]
    branches: [main]
    effect: allow
"#,
        )
        .unwrap();
        let decision = policy.evaluate(&req("bob", "acme", "web", Operation::Push, Some("main")));
        match decision {
            Decision::Deny { reason } => assert!(reason.contains("no matching rule")),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn first_match_wins() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: "*"
    org: acme
    repo: web
    operations: [push]
    branches: ["*"]
    effect: deny
  - user: "*"
    org: acme
    repo: "*"
    operations: [push]
    branches: ["*"]
    effect: allow
"#,
        )
        .unwrap();
        let decision = policy.evaluate(&req("alice", "acme", "web", Operation::Push, Some("main")));
        match decision {
            Decision::Deny { .. } => {}
            other => panic!("expected Deny from first rule, got {other:?}"),
        }
    }

    #[test]
    fn wildcard_user_matches_anyone() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: "*"
    org: acme
    repo: web
    operations: [push]
    branches: [main]
    effect: allow
"#,
        )
        .unwrap();
        assert_eq!(
            policy.evaluate(&req("dave", "acme", "web", Operation::Push, Some("main"))),
            Decision::Allow
        );
    }

    #[test]
    fn branch_glob_release_wildcard() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [push]
    branches: ["release/*"]
    effect: allow
"#,
        )
        .unwrap();
        assert_eq!(
            policy.evaluate(&req(
                "alice",
                "acme",
                "web",
                Operation::Push,
                Some("release/v1.2")
            )),
            Decision::Allow
        );
        // Branch outside glob falls through to default deny.
        assert!(matches!(
            policy.evaluate(&req("alice", "acme", "web", Operation::Push, Some("main"))),
            Decision::Deny { .. }
        ));
    }

    #[test]
    fn operation_mismatch_no_match() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [push, fetch]
    branches: ["*"]
    effect: allow
"#,
        )
        .unwrap();
        let decision = policy.evaluate(&req(
            "alice",
            "acme",
            "web",
            Operation::PrMerge,
            Some("main"),
        ));
        assert!(matches!(decision, Decision::Deny { .. }));
    }

    #[test]
    fn unknown_operation_rejected() {
        let result = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [frobnicate]
    branches: ["*"]
    effect: allow
"#,
        );
        let err = result.expect_err("expected load failure");
        let msg = format!("{err}");
        assert!(
            msg.contains("frobnicate"),
            "error should mention operation: {msg}"
        );
    }

    #[test]
    fn empty_operations_rejected() {
        let result = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: []
    branches: ["*"]
    effect: allow
"#,
        );
        let err = result.expect_err("expected load failure");
        assert!(matches!(err, PolicyError::EmptyOperations { index: 0 }));
    }

    #[test]
    fn branchless_operation_ignores_branch() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [issue_open]
    branches: [main]
    effect: allow
"#,
        )
        .unwrap();
        let decision = policy.evaluate(&req("alice", "acme", "web", Operation::IssueOpen, None));
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn unknown_field_rejected() {
        let result = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [push]
    branches: ["*"]
    effect: allow
    surprise: yes
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn missing_branches_defaults_to_wildcard() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [push]
    effect: allow
"#,
        )
        .unwrap();
        assert_eq!(policy.rules[0].branches, vec!["*".to_string()]);
        assert_eq!(
            policy.evaluate(&req(
                "alice",
                "acme",
                "web",
                Operation::Push,
                Some("anything")
            )),
            Decision::Allow
        );
    }

    #[test]
    fn pull_has_branch_is_false() {
        assert!(!Operation::Pull.has_branch());
    }

    #[test]
    fn gh_api_read_has_branch_is_false() {
        assert!(!Operation::GhApiRead {
            path: "user".to_string()
        }
        .has_branch());
    }

    #[test]
    fn policy_loads_gh_api_read() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: "*"
    repo: "*"
    operations: [gh_api_read]
    effect: allow
"#,
        )
        .unwrap();
        assert_eq!(policy.rules[0].operations.len(), 1);
    }

    #[test]
    fn gh_api_read_allowed_user_scope() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: "*"
    repo: "*"
    operations: [gh_api_read]
    effect: allow
"#,
        )
        .unwrap();
        let op = Operation::GhApiRead {
            path: "user".to_string(),
        };
        assert_eq!(
            policy.evaluate(&req("alice", "*", "*", op, None)),
            Decision::Allow
        );
    }

    #[test]
    fn gh_api_read_ignores_branch() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: "*"
    repo: "*"
    operations: [gh_api_read]
    branches: [main]
    effect: allow
"#,
        )
        .unwrap();
        let op = Operation::GhApiRead {
            path: "repos/acme/web".to_string(),
        };
        assert_eq!(
            policy.evaluate(&req("alice", "*", "*", op, Some("anything"))),
            Decision::Allow
        );
    }

    #[test]
    fn gh_api_read_default_deny() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [push]
    effect: allow
"#,
        )
        .unwrap();
        let op = Operation::GhApiRead {
            path: "user".to_string(),
        };
        assert!(matches!(
            policy.evaluate(&req("alice", "*", "*", op, None)),
            Decision::Deny { .. }
        ));
    }

    #[test]
    fn policy_with_pull_loads() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [pull]
    effect: allow
"#,
        )
        .unwrap();
        assert_eq!(policy.rules[0].operations, vec![Operation::Pull]);
    }

    #[test]
    fn fetch_rule_does_not_match_pull() {
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [fetch]
    effect: allow
"#,
        )
        .unwrap();
        // Fetch rule must NOT match a pull request.
        assert!(matches!(
            policy.evaluate(&req("alice", "acme", "web", Operation::Pull, None)),
            Decision::Deny { .. }
        ));
        // Pull rule must match a pull request.
        let pull_policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [pull]
    effect: allow
"#,
        )
        .unwrap();
        assert_eq!(
            pull_policy.evaluate(&req("alice", "acme", "web", Operation::Pull, None)),
            Decision::Allow
        );
    }

    #[test]
    fn pull_ignores_branch_field() {
        // A rule with operations: [pull] and branches: [main] must match
        // operation: Pull with branch: None (branchless operation).
        let policy = Policy::from_yaml(
            r#"
rules:
  - user: alice
    org: acme
    repo: web
    operations: [pull]
    branches: [main]
    effect: allow
"#,
        )
        .unwrap();
        assert_eq!(
            policy.evaluate(&req("alice", "acme", "web", Operation::Pull, None)),
            Decision::Allow
        );
    }
}

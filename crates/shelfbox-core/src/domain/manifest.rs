use serde::{Deserialize, Serialize};

/// Candidate-ranking hints. These are never proof of identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IdentityHints {
    /// Normalized remote hints, e.g. `github.com/org/repo`.
    #[serde(default)]
    pub remote_hints: Vec<String>,
    /// Recent repository directory names, most recent first.
    #[serde(default)]
    pub repo_name_hints: Vec<String>,
    /// Last successful explicit association or repair timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attached_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identity_hints_omits_absent_last_attached_at() {
        let hints = IdentityHints {
            remote_hints: vec!["github.com/example/app".into()],
            repo_name_hints: vec!["app".into()],
            last_attached_at: None,
        };

        assert_eq!(
            serde_json::to_value(hints).unwrap(),
            json!({
                "remote_hints": ["github.com/example/app"],
                "repo_name_hints": ["app"]
            })
        );
    }

    #[test]
    fn identity_hints_defaults_missing_collections() {
        let hints: IdentityHints = serde_json::from_value(json!({})).unwrap();

        assert!(hints.remote_hints.is_empty());
        assert!(hints.repo_name_hints.is_empty());
        assert_eq!(hints.last_attached_at, None);
    }
}

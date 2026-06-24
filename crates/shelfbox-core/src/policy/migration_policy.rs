use std::collections::HashMap;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestVersionDecision {
    ConvertLegacy,
    AlreadyCurrent,
    SkipUnsupported,
}

pub(crate) fn decide_manifest_version(
    version: u32,
    legacy_version: u32,
    current_version: u32,
) -> ManifestVersionDecision {
    if version == legacy_version {
        ManifestVersionDecision::ConvertLegacy
    } else if version == current_version {
        ManifestVersionDecision::AlreadyCurrent
    } else {
        ManifestVersionDecision::SkipUnsupported
    }
}

pub(crate) fn unsupported_version_message(
    version: u32,
    legacy_version: u32,
    current_version: u32,
) -> String {
    format!(
        "unsupported manifest version {version}; expected one of: {legacy_version}, {current_version}"
    )
}

pub(crate) fn record_unique_id(
    seen: &mut HashMap<String, String>,
    label: &str,
    id: &str,
    repo_store_dir: &str,
) -> Result<()> {
    if let Some(first_dir) = seen.insert(id.to_string(), repo_store_dir.to_string()) {
        return Err(AppError::Internal(format!(
            "duplicate {label} '{id}' found in '{first_dir}' and '{repo_store_dir}'"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_versions_are_classified() {
        assert_eq!(
            decide_manifest_version(2, 2, 3),
            ManifestVersionDecision::ConvertLegacy
        );
        assert_eq!(
            decide_manifest_version(3, 2, 3),
            ManifestVersionDecision::AlreadyCurrent
        );
        assert_eq!(
            decide_manifest_version(4, 2, 3),
            ManifestVersionDecision::SkipUnsupported
        );
    }

    #[test]
    fn duplicate_ids_abort_before_writes() {
        let mut seen = HashMap::new();
        record_unique_id(&mut seen, "repo_id", "repo-1", "first").unwrap();

        let err = record_unique_id(&mut seen, "repo_id", "repo-1", "second").unwrap_err();

        assert!(err.to_string().contains("duplicate repo_id"));
        assert!(err.to_string().contains("first"));
        assert!(err.to_string().contains("second"));
    }
}

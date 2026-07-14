//! Versioned, secret-free durable recovery records.
//!
//! These data types deliberately contain only normalized paths, opaque file
//! identities, and content fingerprints.  They never carry file contents or
//! other plaintext.  Storage owns atomic persistence; recovery owns decisions
//! about whether a record can be cleaned up or must block mutation.

use std::{
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ulid::Ulid;

use super::{
    copy_safety::ArtifactScope,
    materialization::MaterializationStrategy,
    path::{RepoRelativePath, StoreRelativePath},
    recovery_fingerprint::RecoveryFingerprint,
};

pub(crate) const OPERATION_RECORD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryRecord {
    pub schema_version: u32,
    pub record_id: String,
    pub created_at: String,
    pub record: RecoveryRecordKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Durable JSON records favor direct DTO ergonomics over heap indirection.
pub(crate) enum RecoveryRecordKind {
    Operation(OperationRecord),
    Artifact(ArtifactRecord),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OperationRecord {
    pub operation: OperationKind,
    pub phase: OperationPhase,
    pub repo_id: String,
    pub repo_root: RecoveryAbsolutePath,
    /// Repository-specific store directory needed to replay manifest changes
    /// during lifecycle recovery.  This is global-store-relative and carries
    /// no plaintext or user content.
    #[serde(default)]
    pub repo_store_path: Option<StoreRelativePath>,
    pub strategy: MaterializationStrategy,
    pub direction: Option<OperationDirection>,
    pub pre_state: OperationPreState,
    /// Destination observations for operations that move an item's logical
    /// path.  Older records intentionally omit this field and remain
    /// fail-closed until their legacy recovery is resolved.
    #[serde(default)]
    pub post_state: Option<OperationPostState>,
    #[serde(default)]
    pub artifact_record_ids: Vec<String>,
    pub backup: Option<RecoveryBackupMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationKind {
    Add,
    Restore,
    Move,
    Relink,
    Sync,
    Repair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationPhase {
    RecordCreated,
    ExcludeWritten,
    DestinationExcluded,
    StoreTransferred,
    RepoMaterialized,
    RepoRegularized,
    StoreStaged,
    RepoMoved,
    ManifestSaved,
    ManifestRemoved,
    ExcludeUpdated,
    ExcludeFinalized,
    ContentSynchronized,
    OwnershipAttached,
    PostCommitValidated,
}

impl OperationPhase {
    pub(crate) const fn is_finalized(self) -> bool {
        matches!(self, Self::PostCommitValidated)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationDirection {
    FromStore,
    FromRepo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct OperationPreState {
    pub repo_path: Option<RepoRelativePath>,
    /// Path relative to the global store root, not only to one repo store.
    pub store_path: Option<StoreRelativePath>,
    pub repo_fingerprint: Option<RecoveryFingerprint>,
    pub store_fingerprint: Option<RecoveryFingerprint>,
    pub manifest_contains_item: Option<bool>,
    pub exclude_owned: Option<bool>,
    /// The exact exclude state expected once a lifecycle operation completes.
    /// This is separate from `exclude_owned`: restore may deliberately remove
    /// a managed exclude, while legacy detach deliberately retains it.
    #[serde(default)]
    pub final_exclude_owned: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct OperationPostState {
    pub repo_path: Option<RepoRelativePath>,
    pub store_path: Option<StoreRelativePath>,
    pub manifest_contains_item: Option<bool>,
    pub exclude_owned: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryBackupMetadata {
    pub artifact_record_id: String,
    pub location: ArtifactLocation,
    pub expected_identity: Option<RecoveryFileIdentity>,
    pub fingerprint: Option<RecoveryFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArtifactRecord {
    pub repo_id: String,
    pub scope: ArtifactScope,
    pub location: ArtifactLocation,
    pub state: ArtifactState,
    pub repo_temp_exclude: Option<RepoTempExclude>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "root", rename_all = "snake_case")]
pub(crate) enum ArtifactLocation {
    Repo {
        repo_root: RecoveryAbsolutePath,
        path: RepoRelativePath,
    },
    Store {
        path: StoreRelativePath,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ArtifactState {
    /// The path has been durably reserved, but no file may exist yet.
    Planned,
    /// An empty private file was created and its exact identity persisted.
    Created {
        identity: RecoveryFileIdentity,
        plaintext_authorized: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepoTempExclude {
    pub path: RepoRelativePath,
    pub added_by_operation: bool,
    pub verified: bool,
}

/// Platform-neutral serialization of the opaque identity used for safe temp
/// cleanup. `file_hex` is exactly the 16 bytes returned by the platform
/// adapter, encoded as lowercase hexadecimal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RecoveryFileIdentity {
    volume: u64,
    file_hex: String,
}

impl RecoveryFileIdentity {
    pub(crate) fn new(volume: u64, file_hex: impl Into<String>) -> Option<Self> {
        let file_hex = file_hex.into();
        is_canonical_file_hex(&file_hex).then_some(Self { volume, file_hex })
    }
}

#[derive(Serialize)]
struct RecoveryFileIdentityRef<'a> {
    volume: u64,
    file_hex: &'a str,
}

#[derive(Deserialize)]
struct RecoveryFileIdentityRepr {
    volume: u64,
    file_hex: String,
}

impl Serialize for RecoveryFileIdentity {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        RecoveryFileIdentityRef {
            volume: self.volume,
            file_hex: &self.file_hex,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RecoveryFileIdentity {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let repr = RecoveryFileIdentityRepr::deserialize(deserializer)?;
        Self::new(repr.volume, repr.file_hex).ok_or_else(|| {
            serde::de::Error::custom("recovery file identity must use 32 lowercase hex digits")
        })
    }
}

/// Absolute repo roots are persisted only to associate repo-side artifacts
/// with the exact checkout that created them. Recovery compares this value to
/// the currently opened repo before it ever touches the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveryAbsolutePath(PathBuf);

impl RecoveryAbsolutePath {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Option<Self> {
        let path = path.into();
        is_safe_absolute_path(&path).then_some(Self(path))
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Serialize for RecoveryAbsolutePath {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string_lossy())
    }
}

impl<'de> Deserialize<'de> for RecoveryAbsolutePath {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(PathBuf::from(value)).ok_or_else(|| {
            serde::de::Error::custom(
                "recovery absolute path must be absolute and contain no parent components",
            )
        })
    }
}

impl RecoveryRecord {
    pub(crate) fn validate(&self) -> std::result::Result<(), String> {
        if self.schema_version != OPERATION_RECORD_SCHEMA_VERSION {
            return Err("unexpected operation record schema version".into());
        }
        validate_record_id(&self.record_id)?;
        if self.created_at.is_empty() {
            return Err("created_at must not be empty".into());
        }
        match &self.record {
            RecoveryRecordKind::Operation(operation) => operation.validate(),
            RecoveryRecordKind::Artifact(artifact) => artifact.validate(),
        }
    }

    pub(crate) fn is_finalized_operation(&self) -> bool {
        matches!(
            &self.record,
            RecoveryRecordKind::Operation(OperationRecord {
                phase: OperationPhase::PostCommitValidated,
                ..
            })
        )
    }
}

impl OperationRecord {
    fn validate(&self) -> std::result::Result<(), String> {
        validate_repo_id(&self.repo_id)?;
        for id in &self.artifact_record_ids {
            validate_record_id(id)?;
        }
        if let Some(backup) = &self.backup {
            validate_record_id(&backup.artifact_record_id)?;
            backup.location.validate()?;
        }
        if self.operation == OperationKind::Sync {
            if self.direction.is_none() {
                return Err("sync operation requires an explicit direction".into());
            }
            if self.pre_state.repo_path.is_none() || self.pre_state.store_path.is_none() {
                return Err("sync operation requires repository and store paths".into());
            }
            let (Some(repo), Some(store)) = (
                self.pre_state.repo_fingerprint.as_ref(),
                self.pre_state.store_fingerprint.as_ref(),
            ) else {
                return Err("sync operation requires both endpoint fingerprints".into());
            };
            if repo == store {
                return Err("sync operation cannot record equal endpoint fingerprints".into());
            }
            if self.pre_state.manifest_contains_item != Some(true)
                || self.pre_state.exclude_owned != Some(true)
            {
                return Err(
                    "sync operation requires attached manifest and exclude observations".into(),
                );
            }
        }
        Ok(())
    }
}

impl ArtifactRecord {
    fn validate(&self) -> std::result::Result<(), String> {
        validate_repo_id(&self.repo_id)?;
        self.location.validate()?;
        match (&self.scope, &self.location, &self.repo_temp_exclude) {
            (ArtifactScope::RepoSide, ArtifactLocation::Repo { path, .. }, Some(exclude)) => {
                if exclude.path != *path {
                    return Err("repo temp exclude path must match artifact path".into());
                }
            }
            (ArtifactScope::RepoSide, ArtifactLocation::Repo { .. }, None) => {}
            (ArtifactScope::StoreSide, ArtifactLocation::Store { .. }, None) => {}
            (ArtifactScope::StoreSide, ArtifactLocation::Store { .. }, Some(_)) => {
                return Err("store-side artifact must not contain a repo exclude".into());
            }
            _ => return Err("artifact scope does not match its path root".into()),
        }
        Ok(())
    }
}

impl ArtifactLocation {
    fn validate(&self) -> std::result::Result<(), String> {
        match self {
            Self::Repo { repo_root, path } => {
                if path.as_str().is_empty() || !repo_root.as_path().is_absolute() {
                    return Err("invalid repo artifact location".into());
                }
            }
            Self::Store { path } => {
                if path.as_str().is_empty() {
                    return Err("invalid store artifact location".into());
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_record_id(value: &str) -> std::result::Result<(), String> {
    Ulid::from_str(value)
        .map(|_| ())
        .map_err(|_| "record_id must be a ULID".into())
}

fn validate_repo_id(value: &str) -> std::result::Result<(), String> {
    (!value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some(())
    .ok_or_else(|| "repo_id contains unsafe characters".into())
}

fn is_canonical_file_hex(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn id() -> String {
        "01JTB4P3D1H3W0Z6R9M6B8E5QK".into()
    }

    fn repo_root() -> RecoveryAbsolutePath {
        RecoveryAbsolutePath::new(if cfg!(windows) { r"C:\repo" } else { "/repo" }).unwrap()
    }

    #[test]
    fn record_serialization_is_versioned_and_secret_free() {
        let record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            record_id: id(),
            created_at: "2026-07-12T00:00:00Z".into(),
            record: RecoveryRecordKind::Operation(OperationRecord {
                operation: OperationKind::Add,
                phase: OperationPhase::RecordCreated,
                repo_id: "repo_1".into(),
                repo_root: repo_root(),
                repo_store_path: None,
                strategy: MaterializationStrategy::Copy,
                direction: None,
                pre_state: OperationPreState {
                    repo_path: Some("secret.env".parse().unwrap()),
                    store_path: Some("repos/project/items/secret.env".parse().unwrap()),
                    repo_fingerprint: Some(
                        RecoveryFingerprint::from_reader("secret".as_bytes()).unwrap(),
                    ),
                    ..OperationPreState::default()
                },
                post_state: None,
                artifact_record_ids: Vec::new(),
                backup: None,
            }),
        };

        record.validate().unwrap();
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["record"]["kind"], json!("operation"));
        assert!(!value.to_string().contains("secret\""));
    }

    #[test]
    fn artifact_scope_and_location_must_agree() {
        let artifact = ArtifactRecord {
            repo_id: "repo-1".into(),
            scope: ArtifactScope::RepoSide,
            location: ArtifactLocation::Store {
                path: "repos/project/.tmp".parse().unwrap(),
            },
            state: ArtifactState::Planned,
            repo_temp_exclude: None,
        };
        let record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            record_id: id(),
            created_at: "2026-07-12T00:00:00Z".into(),
            record: RecoveryRecordKind::Artifact(artifact),
        };

        assert!(record.validate().is_err());
    }

    #[test]
    fn deserialization_rejects_unsafe_paths_and_identities() {
        assert!(serde_json::from_value::<RecoveryAbsolutePath>(json!("../repo")).is_err());
        assert!(serde_json::from_value::<RecoveryFileIdentity>(json!({
            "volume": 1,
            "file_hex": "not-an-identity"
        }))
        .is_err());
    }
}

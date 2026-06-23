use std::{
    fmt,
    path::{Component, Path},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoRelativePath(String);

impl RepoRelativePath {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        relative_path(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for RepoRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RepoRelativePath {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value).ok_or("repo-relative path must be normalized and relative")
    }
}

impl Serialize for RepoRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RepoRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            serde::de::Error::custom("repo-relative path must be normalized and relative")
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreRelativePath(String);

impl StoreRelativePath {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        relative_path(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for StoreRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for StoreRelativePath {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value).ok_or("store-relative path must be normalized and relative")
    }
}

impl Serialize for StoreRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for StoreRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            serde::de::Error::custom("store-relative path must be normalized and relative")
        })
    }
}

fn relative_path(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    if value.is_empty()
        || value.contains('\\')
        || value.split('/').any(str::is_empty)
        || Path::new(&value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }

    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_relative_path_is_transparent_normalized_string() {
        let path: RepoRelativePath = "notes/design.md".parse().unwrap();

        assert_eq!(path.as_str(), "notes/design.md");
        assert_eq!(serde_json::to_string(&path).unwrap(), "\"notes/design.md\"");
        assert!(RepoRelativePath::new("").is_none());
        assert!(RepoRelativePath::new("/absolute").is_none());
        assert!(RepoRelativePath::new("../outside").is_none());
        assert!(RepoRelativePath::new("notes\\design.md").is_none());
        assert!(serde_json::from_str::<RepoRelativePath>("\"../outside\"").is_err());
    }

    #[test]
    fn store_relative_path_is_transparent_normalized_string() {
        let path: StoreRelativePath = "items/secrets.env".parse().unwrap();

        assert_eq!(path.as_str(), "items/secrets.env");
        assert_eq!(
            serde_json::to_string(&path).unwrap(),
            "\"items/secrets.env\""
        );
        assert!(StoreRelativePath::new("").is_none());
        assert!(StoreRelativePath::new("/absolute").is_none());
        assert!(StoreRelativePath::new("items/../outside").is_none());
        assert!(StoreRelativePath::new("items\\secrets.env").is_none());
        assert!(serde_json::from_str::<StoreRelativePath>("\"items/../outside\"").is_err());
    }
}

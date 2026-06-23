use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoId(String);

impl RepoId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        non_empty(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RepoId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value).ok_or("repo id must not be empty")
    }
}

impl Serialize for RepoId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RepoId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| serde::de::Error::custom("repo id must not be empty"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(String);

impl ItemId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        non_empty(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ItemId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value).ok_or("item id must not be empty")
    }
}

impl Serialize for ItemId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ItemId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| serde::de::Error::custom("item id must not be empty"))
    }
}

fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_id_is_transparent_non_empty_string() {
        let id: RepoId = "01JWPQ3VKGE93V9BDHAENVXFA5".parse().unwrap();

        assert_eq!(id.as_str(), "01JWPQ3VKGE93V9BDHAENVXFA5");
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"01JWPQ3VKGE93V9BDHAENVXFA5\""
        );
        assert!(RepoId::new("").is_none());
        assert!(serde_json::from_str::<RepoId>("\"\"").is_err());
    }

    #[test]
    fn item_id_is_transparent_non_empty_string() {
        let id: ItemId = "01JWPQ3VKGE93V9BDHAENVXFA6".parse().unwrap();

        assert_eq!(id.as_str(), "01JWPQ3VKGE93V9BDHAENVXFA6");
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"01JWPQ3VKGE93V9BDHAENVXFA6\""
        );
        assert!(ItemId::new("").is_none());
        assert!(serde_json::from_str::<ItemId>("\"\"").is_err());
    }
}

use std::path::Path;

use crate::{
    error::{AppError, Result},
    git::command::run_git,
};

pub fn remote_url(repo_root: &Path) -> Result<Option<String>> {
    match run_git(&["remote", "get-url", "origin"], repo_root) {
        Ok(url) if !url.is_empty() => Ok(Some(url)),
        Ok(_) => Ok(None),
        Err(AppError::GitCommand { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn normalize_remote_hint(url: &str) -> Option<String> {
    let mut raw = url.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix("git+") {
        raw = stripped;
    }

    let without_query = raw
        .split(['?', '#'])
        .next()
        .unwrap_or(raw)
        .trim()
        .trim_end_matches('/');

    let (host, path) = if let Some((scheme, rest)) = without_query.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https" | "ssh" | "git") {
            return None;
        }
        let rest = rest.trim_start_matches('/');
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host, path)
    } else if let Some((authority, path)) = without_query.split_once(':') {
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host, path)
    } else {
        let (host, path) = without_query.split_once('/')?;
        (host, path)
    };

    let host = host.trim().to_ascii_lowercase();
    let path = path.trim().trim_start_matches('/').trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }

    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() || path.split('/').any(str::is_empty) {
        return None;
    }

    Some(format!("{host}/{path}"))
}

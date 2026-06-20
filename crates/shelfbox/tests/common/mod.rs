use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus},
    sync::OnceLock,
};

use tempfile::TempDir;

pub struct CliFixture {
    config_home: TempDir,
    data_home: TempDir,
}

impl CliFixture {
    pub fn new() -> Self {
        Self {
            config_home: TempDir::new().unwrap(),
            data_home: TempDir::new().unwrap(),
        }
    }

    pub fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_shelfbox"));
        command
            .current_dir(cwd)
            .env_remove("SHELFBOX_STORE")
            .env("XDG_CONFIG_HOME", self.config_home.path())
            .env("XDG_DATA_HOME", self.data_home.path());
        command
    }

    pub fn run<I, S>(&self, cwd: &Path, args: I) -> CliOutput
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(cwd);
        command.args(args);
        run_command(command)
    }

    pub fn config_file_path(&self) -> PathBuf {
        self.config_home.path().join("shelfbox").join("config.toml")
    }

    pub fn default_store_path(&self) -> PathBuf {
        self.data_home.path().join("shelfbox")
    }

    pub fn write_config(&self, contents: &str) {
        let path = self.config_file_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

#[derive(Debug)]
pub struct CliOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    pub fn assert_success(&self) {
        assert!(
            self.status.success(),
            "expected success, got {:?}\nstdout:\n{}\nstderr:\n{}",
            self.status.code(),
            self.stdout,
            self.stderr
        );
    }

    pub fn assert_code(&self, expected: i32) {
        assert_eq!(
            self.status.code(),
            Some(expected),
            "unexpected status\nstdout:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
    }
}

pub fn run_command(mut command: Command) -> CliOutput {
    let output = command.output().expect("failed to spawn shelfbox");
    CliOutput {
        status: output.status,
        stdout: normalize_line_endings(&String::from_utf8_lossy(&output.stdout)),
        stderr: normalize_line_endings(&String::from_utf8_lossy(&output.stderr)),
    }
}

pub fn init_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    init_git_repo_at(dir.path());
    dir
}

pub fn init_git_repo_at(path: &Path) {
    for args in [
        ["init", "-b", "main"].as_slice(),
        ["config", "user.email", "test@example.com"].as_slice(),
        ["config", "user.name", "Test User"].as_slice(),
    ] {
        run_git(path, args);
    }
}

pub fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn git {}: {err}", args[0]));
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args[0],
        String::from_utf8_lossy(&output.stderr)
    );
}

fn try_create_file_symlink(target: &Path, link_path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link_path)
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link_path)
    }
}

pub fn require_symlink_support() -> bool {
    static SYMLINK_SUPPORT_ERROR: OnceLock<Option<String>> = OnceLock::new();

    let unsupported_reason = SYMLINK_SUPPORT_ERROR.get_or_init(|| {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("symlink-target.txt");
        let link_path = dir.path().join("symlink-link.txt");
        std::fs::write(&target, "probe").unwrap();

        match try_create_file_symlink(&target, &link_path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&link_path);
                None
            }
            Err(err) => Some(format!(
                "skipping symlink-dependent CLI test because symlink creation is unavailable: {err}"
            )),
        }
    });

    if let Some(reason) = unsupported_reason {
        if std::env::var_os("SHELFBOX_REQUIRE_SYMLINKS").is_some() {
            panic!("{reason}");
        }
        eprintln!("{reason}");
        return false;
    }

    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSnapshot {
    entries: BTreeMap<String, TreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeEntry {
    Dir,
    File(String),
    Symlink(String),
}

pub fn snapshot_tree(root: &Path) -> TreeSnapshot {
    let mut snapshot = TreeSnapshot {
        entries: BTreeMap::new(),
    };
    if root.exists() {
        snapshot_tree_inner(root, root, &mut snapshot);
    }
    snapshot
}

fn snapshot_tree_inner(root: &Path, path: &Path, snapshot: &mut TreeSnapshot) {
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();

    for entry_path in entries {
        let metadata = std::fs::symlink_metadata(&entry_path)
            .unwrap_or_else(|err| panic!("failed to stat {}: {err}", entry_path.display()));
        let rel = normalize_relative_path(root, &entry_path);

        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&entry_path).unwrap_or_else(|err| {
                panic!("failed to read link {}: {err}", entry_path.display())
            });
            snapshot
                .entries
                .insert(rel, TreeEntry::Symlink(normalize_path_for_display(&target)));
        } else if metadata.is_dir() {
            snapshot.entries.insert(rel, TreeEntry::Dir);
            snapshot_tree_inner(root, &entry_path, snapshot);
        } else {
            let contents = std::fs::read_to_string(&entry_path).unwrap_or_else(|_| {
                String::from_utf8_lossy(&std::fs::read(&entry_path).unwrap()).into_owned()
            });
            snapshot.entries.insert(rel, TreeEntry::File(contents));
        }
    }
}

pub fn normalize_output(output: &str, path_replacements: &[(&Path, &str)]) -> String {
    let mut normalized = normalize_line_endings(output);
    for (path, replacement) in path_replacements {
        normalized = normalized.replace(&path.display().to_string(), replacement);
        normalized = normalized.replace(&normalize_path_for_display(path), replacement);
    }
    normalized = replace_iso8601_timestamps(&normalized);
    replace_ulids(&normalized)
}

pub fn toml_literal_path(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

fn normalize_line_endings(input: &str) -> String {
    input.replace("\r\n", "\n")
}

fn normalize_relative_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    normalize_path_for_display(rel)
}

fn normalize_path_for_display(path: &Path) -> String {
    path.components()
        .map(|component| match component {
            Component::Normal(part) => part.to_string_lossy().into_owned(),
            Component::RootDir => String::new(),
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
            Component::CurDir | Component::ParentDir => {
                component.as_os_str().to_string_lossy().into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn replace_iso8601_timestamps(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut index = 0;

    while index < input.len() {
        let remaining = &input[index..];
        let bytes = remaining.as_bytes();
        if bytes.len() >= 20 && is_iso8601_timestamp(&bytes[..20]) {
            output.push_str("<TIMESTAMP>");
            index += 20;
        } else {
            let ch = remaining.chars().next().unwrap();
            output.push(ch);
            index += ch.len_utf8();
        }
    }

    output
}

fn is_iso8601_timestamp(bytes: &[u8]) -> bool {
    bytes.len() == 20
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b'T'
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b':'
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[16] == b':'
        && bytes[17..19].iter().all(u8::is_ascii_digit)
        && bytes[19] == b'Z'
}

fn replace_ulids(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut token = String::new();

    for ch in input.chars() {
        if is_ulid_char(ch) {
            token.push(ch);
        } else {
            flush_ulid_token(&mut output, &mut token);
            output.push(ch);
        }
    }
    flush_ulid_token(&mut output, &mut token);

    output
}

fn flush_ulid_token(output: &mut String, token: &mut String) {
    if token.len() == 26 {
        output.push_str("<ULID>");
    } else {
        output.push_str(token);
    }
    token.clear();
}

fn is_ulid_char(ch: char) -> bool {
    matches!(ch, '0'..='9' | 'A'..='H' | 'J'..='K' | 'M'..='N' | 'P'..='T' | 'V'..='Z')
}

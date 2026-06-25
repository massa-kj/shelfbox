use std::{ffi::OsString, path::Path};

use serde_json::Value;
use tempfile::TempDir;

mod common;

use common::{run_command, snapshot_tree, CliFixture};

const REPO_ID: &str = "01JWPQ3VKGE93V9BDHAENVXFA5";
const ITEM_ID: &str = "01JWPQ3VKGE93V9BDHAENVXFA6";

#[test]
fn config_get_store_source_follows_precedence() {
    let fixture = CliFixture::new();
    let cwd = TempDir::new().unwrap();
    let file_store = TempDir::new().unwrap();
    let env_store = TempDir::new().unwrap();
    let cli_store = TempDir::new().unwrap();

    fixture.write_config(&format!(
        "store = '{}'\n",
        common::toml_literal_path(file_store.path())
    ));

    let mut command = fixture.command(cwd.path());
    command
        .arg("--store")
        .arg(cli_store.path())
        .args(["config", "get", "store", "--source"])
        .env("SHELFBOX_STORE", env_store.path());
    let output = run_command(command);
    output.assert_success();
    assert_eq!(
        output.stdout,
        format!("{}\nsource: cli\n", cli_store.path().display())
    );
    assert_eq!(output.stderr, "");

    let mut command = fixture.command(cwd.path());
    command
        .args(["config", "get", "store", "--source"])
        .env("SHELFBOX_STORE", env_store.path());
    let output = run_command(command);
    output.assert_success();
    assert_eq!(
        output.stdout,
        format!(
            "{}\nsource: env:SHELFBOX_STORE\n",
            env_store.path().display()
        )
    );
    assert_eq!(output.stderr, "");

    let output = fixture.run(cwd.path(), ["config", "get", "store", "--source"]);
    output.assert_success();
    assert_eq!(
        output.stdout,
        format!("{}\nsource: config\n", file_store.path().display())
    );
    assert_eq!(output.stderr, "");
}

#[test]
fn config_list_json_uses_isolated_defaults_without_creating_config_file() {
    let fixture = CliFixture::new();
    let cwd = TempDir::new().unwrap();

    let output = fixture.run(cwd.path(), ["config", "list", "--format", "json"]);
    output.assert_success();
    assert_eq!(output.stderr, "");
    assert!(
        !fixture.config_file_path().exists(),
        "read-only config list must not create config.toml"
    );

    let rows: Vec<Value> = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(rows.len(), 2);

    let store = row_by_key(&rows, "store");
    assert_eq!(store["type"], "path");
    assert_eq!(store["source"], "default");
    assert_eq!(
        store["current"],
        fixture.default_store_path().display().to_string()
    );

    let default_format = row_by_key(&rows, "default_format");
    assert_eq!(default_format["type"], "enum");
    assert_eq!(default_format["source"], "default");
    assert_eq!(default_format["current"], "table");
}

#[test]
fn store_rebuild_index_dry_run_reports_without_writing_index() {
    let fixture = CliFixture::new();
    let cwd = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    write_v3_manifest(store.path(), "project-a", REPO_ID);
    let before = snapshot_tree(store.path());

    let output = fixture.run(
        cwd.path(),
        store_args(store.path(), ["store", "rebuild-index", "--dry-run"]),
    );

    output.assert_success();
    assert_eq!(output.stderr, "");
    assert_eq!(
        output.stdout,
        "Dry run - no index written.\nWould rebuild index: 1 repositories, 0 warnings\n"
    );
    assert!(!store.path().join("index.json").exists());
    assert_eq!(snapshot_tree(store.path()), before);
}

#[test]
fn store_migrate_manifests_dry_run_reports_without_writing_manifest() {
    let fixture = CliFixture::new();
    let cwd = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    write_v2_manifest(store.path(), "my-project", REPO_ID, ITEM_ID, "stale");
    let before = snapshot_tree(store.path());

    let output = fixture.run(
        cwd.path(),
        store_args(store.path(), ["store", "migrate-manifests", "--dry-run"]),
    );

    output.assert_success();
    assert_eq!(output.stderr, "");
    assert_eq!(
        output.stdout,
        concat!(
            "Dry run - no manifests written.\n",
            "target manifest version: 3\n",
            "manifests converted: 1\n",
            "  v2 -> v3: 1\n",
            "manifests unchanged: 0\n",
            "skipped/failed: 0\n",
            "ownership mappings: stale -> unreachable: 1, adopted -> detached: 0\n",
            "namespace entries dropped: 1\n"
        )
    );
    assert_eq!(snapshot_tree(store.path()), before);
}

#[test]
fn store_gc_dry_run_reports_orphaned_items_without_writing() {
    let fixture = CliFixture::new();
    let cwd = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    write_orphaned_v3_manifest(store.path(), "project-a", REPO_ID, ITEM_ID);
    let before = snapshot_tree(store.path());

    let output = fixture.run(
        cwd.path(),
        store_args(store.path(), ["store", "gc", "--dry-run"]),
    );

    output.assert_success();
    assert_eq!(output.stderr, "");
    assert_eq!(
        output.stdout,
        concat!(
            "Orphaned items eligible for deletion:\n",
            "  repos/project-a/items/old.env [01JWPQ3VKGE93V9BDHAENVXFA5] - 11 B\n",
            "Total: 1 item(s), 11 B.\n",
            "Dry run - no changes made.\n"
        )
    );
    assert_eq!(snapshot_tree(store.path()), before);
}

#[test]
fn item_add_dry_run_does_not_initialize_store() {
    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("dry.txt");
    std::fs::write(&item_path, "dry-run").unwrap();
    let repo_before = snapshot_tree(repo.path());
    let store_before = snapshot_tree(store.path());

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["item", "add", "dry.txt", "--dry-run"]),
    );

    output.assert_success();
    assert_eq!(output.stderr, "");
    assert!(output.stdout.contains("[dry-run] shelve 'dry.txt'\n"));
    assert!(output.stdout.contains("  exclude dry.txt\n"));
    assert_eq!(snapshot_tree(repo.path()), repo_before);
    assert_eq!(snapshot_tree(store.path()), store_before);
    assert_absent(store.path(), "meta.json");
    assert_absent(store.path(), "index.json");
    assert_absent(store.path(), "repos");
    assert_absent(store.path(), ".lock");
}

#[test]
fn item_restore_keep_store_dry_run_reports_plan_without_writing() {
    if !common::require_symlink_support() {
        return;
    }

    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("secret.txt");
    std::fs::write(&item_path, "secret").unwrap();

    fixture
        .run(
            repo.path(),
            store_args(store.path(), ["item", "add", "secret.txt"]),
        )
        .assert_success();
    let repo_before = snapshot_tree(repo.path());
    let store_before = snapshot_tree(store.path());

    let output = fixture.run(
        repo.path(),
        store_args(
            store.path(),
            ["item", "restore", "secret.txt", "--dry-run", "--keep-store"],
        ),
    );

    output.assert_success();
    assert_eq!(output.stderr, "");
    assert_eq!(
        output.stdout,
        concat!(
            "[dry-run] restore --keep-store 'secret.txt'\n",
            "  ownership_state: attached -> detached\n",
            "  (symlink and store item left in place)\n",
            "  remove from exclude: secret.txt\n"
        )
    );
    assert_eq!(snapshot_tree(repo.path()), repo_before);
    assert_eq!(snapshot_tree(store.path()), store_before);
}

#[test]
fn item_move_dry_run_reports_plan_without_writing() {
    if !common::require_symlink_support() {
        return;
    }

    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("original.txt");
    std::fs::write(&item_path, "secret").unwrap();

    fixture
        .run(
            repo.path(),
            store_args(store.path(), ["item", "add", "original.txt"]),
        )
        .assert_success();
    let repo_store = store
        .path()
        .join("repos")
        .join(single_repo_store_dir(store.path()));
    let repo_before = snapshot_tree(repo.path());
    let store_before = snapshot_tree(store.path());

    let output = fixture.run(
        repo.path(),
        store_args(
            store.path(),
            ["item", "move", "original.txt", "renamed.txt", "--dry-run"],
        ),
    );

    output.assert_success();
    assert_eq!(output.stderr, "");
    let normalized = common::normalize_output(
        &output.stdout,
        &[(&repo_store, "<repo-store>"), (repo.path(), "<repo>")],
    );
    assert_eq!(
        normalized,
        concat!(
            "[dry-run] move 'original.txt' → 'renamed.txt'\n",
            "  store   <repo-store>/items/original.txt → <repo-store>/items/renamed.txt\n",
            "  symlink <repo>/original.txt → <repo>/renamed.txt\n",
            "  manifest: update path and store_path\n",
            "  exclude:  remove 'original.txt', add 'renamed.txt'\n"
        )
    );
    assert_eq!(snapshot_tree(repo.path()), repo_before);
    assert_eq!(snapshot_tree(store.path()), store_before);
}

#[test]
fn item_status_exit_codes_are_locked_for_associated_repo() {
    if !common::require_symlink_support() {
        return;
    }

    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("secret.txt");
    std::fs::write(&item_path, "secret").unwrap();

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["item", "add", "secret.txt"]),
    );
    output.assert_success();
    assert_eq!(
        common::normalize_output(&output.stdout, &[]),
        "shelved: secret.txt\n"
    );
    assert_eq!(output.stderr, "");

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["item", "status", "--format", "plain"]),
    );
    output.assert_code(0);
    assert_eq!(output.stdout, "OK secret.txt\n");
    assert_eq!(output.stderr, "");

    std::fs::write(repo.path().join(".git").join("info").join("exclude"), "").unwrap();
    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["item", "status", "--format", "plain"]),
    );
    output.assert_code(1);
    assert_eq!(output.stdout, "WARN secret.txt not in exclude\n");
    assert_eq!(output.stderr, "");

    std::fs::remove_file(&item_path).unwrap();
    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["item", "status", "--format", "plain"]),
    );
    output.assert_code(2);
    assert_eq!(
        output.stdout,
        "ERROR secret.txt symlink missing,not in exclude\n"
    );
    assert_eq!(output.stderr, "");
}

#[test]
fn repo_status_exit_codes_are_locked_for_associated_repo() {
    if !common::require_symlink_support() {
        return;
    }

    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("repo-secret.txt");
    std::fs::write(&item_path, "secret").unwrap();

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["item", "add", "repo-secret.txt"]),
    );
    output.assert_success();
    assert_eq!(output.stderr, "");

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["repo", "status", "--format", "plain"]),
    );
    output.assert_code(0);
    assert_eq!(output.stdout, "OK repo-secret.txt\n");
    assert_eq!(output.stderr, "");

    std::fs::write(repo.path().join(".git").join("info").join("exclude"), "").unwrap();
    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["repo", "status", "--format", "plain"]),
    );
    output.assert_code(1);
    assert_eq!(output.stdout, "ERROR repo-secret.txt\n");
    assert_eq!(output.stderr, "");

    std::fs::remove_file(&item_path).unwrap();
    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["repo", "status", "--format", "plain"]),
    );
    output.assert_code(2);
    assert_eq!(output.stdout, "ERROR repo-secret.txt\n");
    assert_eq!(output.stderr, "");
}

#[test]
fn repo_status_for_unassociated_repo_does_not_initialize_store() {
    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let before = snapshot_tree(store.path());

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["repo", "status", "--format", "plain"]),
    );

    output.assert_code(0);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
    assert_eq!(snapshot_tree(store.path()), before);
    assert_absent(store.path(), "meta.json");
    assert_absent(store.path(), "index.json");
    assert_absent(store.path(), "repos");
    assert_absent(store.path(), ".lock");

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["doctor", "--format", "plain"]),
    );

    output.assert_code(0);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
    assert_eq!(snapshot_tree(store.path()), before);
    assert_absent(store.path(), "meta.json");
    assert_absent(store.path(), "index.json");
    assert_absent(store.path(), "repos");
    assert_absent(store.path(), ".lock");
}

#[test]
fn read_only_item_and_repo_gc_commands_do_not_initialize_absent_store() {
    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let before = snapshot_tree(store.path());

    for (command, expected_stdout) in [
        (vec!["item", "list", "--format", "plain"], ""),
        (vec!["item", "status", "--format", "plain"], ""),
        (vec!["item", "info", "missing.txt", "--format", "plain"], ""),
        (
            vec!["repo", "gc", "--dry-run"],
            "no unreferenced current-repository store files found\n",
        ),
    ] {
        let output = fixture.run(repo.path(), store_args_slice(store.path(), &command));

        output.assert_code(0);
        assert_eq!(output.stdout, expected_stdout);
        assert_eq!(output.stderr, "");
        assert_eq!(snapshot_tree(store.path()), before);
        assert_absent(store.path(), "meta.json");
        assert_absent(store.path(), "index.json");
        assert_absent(store.path(), "repos");
        assert_absent(store.path(), ".lock");
    }
}

#[test]
fn repo_repair_for_unassociated_repo_refuses_without_initializing_store() {
    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let before = snapshot_tree(store.path());

    let output = fixture.run(repo.path(), store_args(store.path(), ["repo", "repair"]));

    output.assert_code(255);
    assert!(output.stderr.contains("Run `shelfbox repo reclaim` first"));
    assert_eq!(output.stdout, "");
    assert_eq!(snapshot_tree(store.path()), before);
    assert_absent(store.path(), "meta.json");
    assert_absent(store.path(), "index.json");
    assert_absent(store.path(), "repos");
    assert_absent(store.path(), ".lock");
}

#[test]
fn repo_repair_dry_run_reports_plan_without_writing() {
    if !common::require_symlink_support() {
        return;
    }

    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("repo-dry.txt");
    std::fs::write(&item_path, "secret").unwrap();

    fixture
        .run(
            repo.path(),
            store_args(store.path(), ["item", "add", "repo-dry.txt"]),
        )
        .assert_success();
    let repo_store = store
        .path()
        .join("repos")
        .join(single_repo_store_dir(store.path()));
    std::fs::remove_file(&item_path).unwrap();
    let repo_before = snapshot_tree(repo.path());
    let store_before = snapshot_tree(store.path());

    let output = fixture.run(
        repo.path(),
        store_args(store.path(), ["repo", "repair", "--dry-run"]),
    );

    output.assert_success();
    assert_eq!(output.stderr, "");
    let normalized = common::normalize_output(
        &output.stdout,
        &[(&repo_store, "<repo-store>"), (repo.path(), "<repo>")],
    );
    assert_eq!(
        normalized,
        concat!(
            "[dry-run] repair 'repo-dry.txt'\n",
            "  recreate symlink <repo>/repo-dry.txt → <repo-store>/items/repo-dry.txt\n",
            "repo repair:\n",
            "  symlinks would repair: 1\n",
            "  symlinks already healthy: 0\n",
            "  symlinks failed: 0\n",
            "  exclude: already current\n",
            "  index: already current\n",
            "  identity hints: would update\n"
        )
    );
    assert_eq!(snapshot_tree(repo.path()), repo_before);
    assert_eq!(snapshot_tree(store.path()), store_before);
}

#[test]
fn repo_reclaim_explicit_target_updates_association_without_repairing_symlinks() {
    if !common::require_symlink_support() {
        return;
    }

    let fixture = CliFixture::new();
    let original = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = original.path().join("secret.txt");
    std::fs::write(&item_path, "secret").unwrap();

    fixture
        .run(
            original.path(),
            store_args(store.path(), ["item", "add", "secret.txt"]),
        )
        .assert_success();
    let repo_id = single_repo_id(store.path());

    let reclone = common::init_git_repo();
    let output = fixture.run(
        reclone.path(),
        store_args_slice(
            store.path(),
            &["repo", "reclaim", "--repo-id", repo_id.as_str()],
        ),
    );

    output.assert_success();
    assert_eq!(
        output.stdout,
        format!("Associated with {repo_id}. Run `shelfbox repo repair` to restore symlinks.\n")
    );
    assert_eq!(output.stderr, "");
    assert!(
        !reclone.path().join("secret.txt").exists(),
        "repo reclaim must not repair repo-side symlinks"
    );

    let index_json: Value =
        serde_json::from_str(&std::fs::read_to_string(store.path().join("index.json")).unwrap())
            .unwrap();
    let repo_entry = &index_json["repos"][repo_id.as_str()];
    assert_eq!(repo_entry["root"], reclone.path().display().to_string());
    let repo_store_dir = repo_entry["repo_store_dir"]
        .as_str()
        .expect("repo entry should record repo_store_dir");
    assert!(store
        .path()
        .join("repos")
        .join(repo_store_dir)
        .join("items")
        .join("secret.txt")
        .exists());
}

#[test]
fn read_only_cli_commands_do_not_update_last_seen_at() {
    if !common::require_symlink_support() {
        return;
    }

    let fixture = CliFixture::new();
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("secret.txt");
    std::fs::write(&item_path, "secret").unwrap();

    fixture
        .run(
            repo.path(),
            store_args(store.path(), ["item", "add", "secret.txt"]),
        )
        .assert_success();

    let index_path = store.path().join("index.json");
    let mut index_json: Value =
        serde_json::from_str(&std::fs::read_to_string(&index_path).unwrap())
            .expect("index.json should be valid JSON");
    let repos = index_json
        .get_mut("repos")
        .and_then(Value::as_object_mut)
        .expect("index.json should contain repos object");
    for entry in repos.values_mut() {
        entry["last_seen_at"] = Value::String("2026-01-01T00:00:00Z".to_string());
    }
    std::fs::write(
        &index_path,
        serde_json::to_string_pretty(&index_json).unwrap(),
    )
    .unwrap();

    let index_before = std::fs::read_to_string(&index_path).unwrap();
    for command in [
        vec!["item", "list", "--format", "plain"],
        vec!["item", "status", "--format", "plain"],
        vec!["item", "info", "secret.txt", "--format", "plain"],
        vec!["repo", "status", "--format", "plain"],
        vec!["repo", "gc", "--dry-run"],
        vec!["doctor", "--format", "plain"],
    ] {
        let output = fixture.run(repo.path(), store_args_slice(store.path(), &command));
        output.assert_code(0);
        assert_eq!(
            std::fs::read_to_string(&index_path).unwrap(),
            index_before,
            "{command:?} must not update last_seen_at"
        );
    }
}

fn row_by_key<'a>(rows: &'a [Value], key: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["key"] == key)
        .unwrap_or_else(|| panic!("missing config list row for {key}"))
}

fn assert_absent(root: &Path, rel: &str) {
    assert!(
        !root.join(rel).exists(),
        "expected {} to remain absent",
        root.join(rel).display()
    );
}

fn store_args<const N: usize>(store: &Path, args: [&str; N]) -> Vec<OsString> {
    let mut out = vec![OsString::from("--store"), store.as_os_str().to_os_string()];
    out.extend(args.into_iter().map(OsString::from));
    out
}

fn store_args_slice(store: &Path, args: &[&str]) -> Vec<OsString> {
    let mut out = vec![OsString::from("--store"), store.as_os_str().to_os_string()];
    out.extend(args.iter().map(OsString::from));
    out
}

fn single_repo_id(store: &Path) -> String {
    let index_json: Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("index.json")).unwrap())
            .expect("index.json should be valid JSON");
    let repos = index_json
        .get("repos")
        .and_then(Value::as_object)
        .expect("index.json should contain repos object");
    assert_eq!(repos.len(), 1, "expected exactly one repo in index");
    repos.keys().next().unwrap().to_string()
}

fn single_repo_store_dir(store: &Path) -> String {
    let index_json: Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("index.json")).unwrap())
            .expect("index.json should be valid JSON");
    let repos = index_json
        .get("repos")
        .and_then(Value::as_object)
        .expect("index.json should contain repos object");
    assert_eq!(repos.len(), 1, "expected exactly one repo in index");
    repos
        .values()
        .next()
        .and_then(|entry| entry.get("repo_store_dir"))
        .and_then(Value::as_str)
        .expect("repo entry should record repo_store_dir")
        .to_string()
}

fn write_v3_manifest(store: &Path, repo_store_dir: &str, repo_id: &str) {
    let repo_store = store.join("repos").join(repo_store_dir);
    std::fs::create_dir_all(&repo_store).unwrap();
    std::fs::write(
        repo_store.join("manifest.json"),
        format!(
            r#"{{
  "version": 3,
  "repo_id": "{repo_id}",
  "created_at": "2026-04-29T00:00:00Z",
  "identity_hints": {{}},
  "items": []
}}"#
        ),
    )
    .unwrap();
}

fn write_orphaned_v3_manifest(store: &Path, repo_store_dir: &str, repo_id: &str, item_id: &str) {
    let repo_store = store.join("repos").join(repo_store_dir);
    std::fs::create_dir_all(repo_store.join("items")).unwrap();
    std::fs::write(repo_store.join("items").join("old.env"), "orphan-data").unwrap();
    std::fs::write(
        repo_store.join("manifest.json"),
        format!(
            r#"{{
  "version": 3,
  "repo_id": "{repo_id}",
  "created_at": "2026-04-29T00:00:00Z",
  "identity_hints": {{}},
  "items": [{{
    "item_id": "{item_id}",
    "origin_repo_id": "{repo_id}",
    "path": "old.env",
    "store_path": "items/old.env",
    "ownership_state": "orphaned",
    "created_at": "2026-04-29T00:00:00Z",
    "updated_at": "2026-04-30T00:00:00Z"
  }}]
}}"#
        ),
    )
    .unwrap();
}

fn write_v2_manifest(
    store: &Path,
    repo_store_dir: &str,
    repo_id: &str,
    item_id: &str,
    ownership_state: &str,
) {
    let repo_store = store.join("repos").join(repo_store_dir);
    std::fs::create_dir_all(&repo_store).unwrap();
    std::fs::write(
        repo_store.join("manifest.json"),
        format!(
            r#"{{
  "version": 2,
  "repo": {{
    "id": "{repo_id}",
    "name": "my-project",
    "remote": "git@github.com:example/my-project.git"
  }},
  "items": [{{
    "item_id": "{item_id}",
    "origin_repo_id": "{repo_id}",
    "path": ".env",
    "store_path": "items/.env",
    "kind": "file",
    "link": {{"type": "symlink"}},
    "git": {{"was_tracked": false}},
    "ownership_state": "{ownership_state}",
    "created_at": "2026-04-29T00:00:00Z",
    "updated_at": "2026-04-30T00:00:00Z"
  }}],
  "namespaces": [{{
    "path": "secrets/",
    "created_at": "2026-04-29T00:00:00Z",
    "updated_at": "2026-04-29T00:00:00Z"
  }}]
}}"#
        ),
    )
    .unwrap();
}

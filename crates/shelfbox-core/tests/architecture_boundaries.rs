//! Source-boundary guard for the copy-mode operation ports.
//!
//! D6 lands before existing symlink operations migrate. Their explicitly
//! enumerated legacy dependencies may shrink but cannot spread to new
//! production operation modules. New copy-aware operations must use the
//! `Materializer` and `CanonicalTransfer` ports instead.

use std::{
    fs,
    path::{Path, PathBuf},
};

const LEGACY_LINK_STRATEGY_FILES: &[&str] = &[
    "add.rs",
    "info.rs",
    "integrity.rs",
    "move_item.rs",
    "relink.rs",
    "repair.rs",
    "restore.rs",
    "status.rs",
];

const LEGACY_DIRECT_FILESYSTEM_ALLOWLIST: &[(&str, &[&str])] = &[
    ("std::fs::copy(", &["move_item.rs"]),
    (
        "std::fs::rename(",
        &["add.rs", "move_item.rs", "restore.rs"],
    ),
    ("std::fs::remove_file(", &["gc.rs", "move_item.rs"]),
    ("std::fs::remove_dir_all(", &["gc.rs"]),
    ("fs::read_link(", &["info.rs", "integrity.rs"]),
];

#[test]
fn production_operations_cannot_gain_low_level_materialization_dependencies() {
    let ops_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ops");
    let sources = rust_sources(&ops_dir);
    let link_strategy_references = sources
        .iter()
        .map(|(_, source)| count(source, "LinkStrategy"))
        .sum::<usize>();

    // The eight pre-D6 symlink-only modules use 30 references. The ceiling
    // allows migration to reduce the count but prevents new direct imports or
    // uses from entering this transitional branch.
    assert!(
        link_strategy_references <= 30,
        "D6 migration boundary permits at most the pre-existing 30 LinkStrategy references; use Materializer instead"
    );

    for (path, source) in &sources {
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();

        if source.contains("LinkStrategy") {
            assert!(
                LEGACY_LINK_STRATEGY_FILES.contains(&file_name),
                "{file_name} imports or uses LinkStrategy; production operations must use Materializer"
            );
        }

        for forbidden in [
            "crate::fs::platform",
            "fs::platform",
            "secure_transfer",
            "fs::symlink::",
            "std::os::unix::fs::symlink",
            "std::os::windows::fs::symlink",
        ] {
            assert!(
                !source.contains(forbidden),
                "{file_name} depends on forbidden low-level filesystem detail `{forbidden}`; use an operation-facing port"
            );
        }

        for (token, allowed_files) in LEGACY_DIRECT_FILESYSTEM_ALLOWLIST {
            if source.contains(token) {
                assert!(
                    allowed_files.contains(&file_name),
                    "{file_name} directly calls `{token}`; use Materializer or CanonicalTransfer"
                );
            }
        }
    }
}

fn rust_sources(directory: &Path) -> Vec<(PathBuf, String)> {
    let mut sources = Vec::new();
    collect_rust_sources(directory, &mut sources);
    sources
}

fn collect_rust_sources(directory: &Path, sources: &mut Vec<(PathBuf, String)>) {
    for entry in fs::read_dir(directory).expect("operations directory must be readable") {
        let entry = entry.expect("operation directory entry must be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, sources);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            && path.file_name().and_then(|name| name.to_str())
                != Some("materialization_boundary_tests.rs")
        {
            let source = fs::read_to_string(&path).expect("operation source must be UTF-8");
            sources.push((path, source));
        }
    }
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

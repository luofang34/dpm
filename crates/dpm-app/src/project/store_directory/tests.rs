use super::*;
use dpm_model::WorkspaceId;

fn project(store: &str) -> (tempfile::TempDir, ProjectLocation) {
    let root = tempfile::tempdir().expect("directory");
    let directory = root.path().join(".dpm");
    fs::create_dir(&directory).expect("store directory");
    for name in [
        "project.toml",
        ".gitignore",
        "state.sqlite",
        "state.sqlite-wal",
        "state.sqlite-shm",
        "state.sqlite.runs",
    ] {
        fs::write(directory.join(name), "x").expect("project file");
    }
    fs::create_dir_all(directory.join(".dpm-claude/state.sqlite")).expect("host records");
    let location = ProjectLocation {
        workspace: WorkspaceId::new(),
        asset: None,
        root: root.path().to_path_buf(),
        source: ProjectSource::Database(directory.join(store)),
    };
    (root, location)
}

#[test]
fn a_store_directory_holding_only_project_state_reports_nothing() {
    let (_root, location) = project("state.sqlite");
    assert_eq!(location.stray_store_content_blocking(), None);
}

#[test]
fn copied_evidence_is_named_and_measured_recursively() {
    let (root, location) = project("state.sqlite");
    let directory = root.path().join(".dpm");
    fs::write(directory.join("screenshot.png"), [0_u8; 300]).expect("screenshot");
    fs::create_dir_all(directory.join("artifacts/run/final")).expect("artifacts");
    fs::write(
        directory.join("artifacts/run/final/tests.wasm"),
        [0_u8; 700],
    )
    .expect("binary");
    let stray = location
        .stray_store_content_blocking()
        .expect("stray content");
    assert_eq!(stray.directory, directory);
    assert_eq!(stray.entries, ["artifacts", "screenshot.png"]);
    assert_eq!(stray.bytes, 1000);
    assert!(stray.complete);
    assert!(stray.advice.contains("link"), "{}", stray.advice);
}

#[test]
fn store_files_count_as_state_only_for_the_store_the_locator_names() {
    let (_root, location) = project("other.sqlite");
    let stray = location
        .stray_store_content_blocking()
        .expect("stray content");
    assert_eq!(
        stray.entries,
        [
            "state.sqlite",
            "state.sqlite-shm",
            "state.sqlite-wal",
            "state.sqlite.runs"
        ]
    );
}

#[test]
fn an_unreadable_store_directory_does_not_fail_status() {
    let (root, location) = project("state.sqlite");
    fs::remove_dir_all(root.path().join(".dpm")).expect("remove");
    assert_eq!(location.stray_store_content_blocking(), None);
}

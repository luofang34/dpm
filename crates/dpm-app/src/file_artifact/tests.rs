use super::*;

#[test]
fn a_file_is_fingerprinted_by_its_sha256_and_size() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("test.txt");
    std::fs::write(&path, "test").expect("file");
    assert_eq!(
        fingerprint_blocking(&path).expect("fingerprint"),
        (
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
            4
        )
    );
    assert!(
        fingerprint_blocking(dir.path()).is_err(),
        "a directory is not evidence"
    );
}

#[test]
fn a_file_uri_percent_encodes_what_a_path_may_hold() {
    assert_eq!(
        file_uri(Path::new("/Volumes/QA/inspection 17#a.jpg")),
        "file:///Volumes/QA/inspection%2017%23a.jpg"
    );
}

//! Installed diagnostics must never attach to or stop an existing user service.

#[test]
fn package_smoke_never_reuses_an_existing_data_directory() {
    let directory = tempfile::tempdir().expect("isolated fixture");
    let sentinel = directory.path().join("preserve-this-file");
    std::fs::write(&sentinel, "existing application data").expect("fixture write");
    let result = kanban_desktop_lib::package_smoke::run(directory.path());
    assert!(result.unwrap_err().contains("must not exist"));
    assert_eq!(
        std::fs::read_to_string(sentinel).unwrap(),
        "existing application data"
    );
}

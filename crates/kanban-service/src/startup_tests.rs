use super::*;
use crate::test_client::{Client, boot};

fn runtime(data: &Path) -> ServiceRuntime {
    ServiceRuntime {
        mcp_executable: "/usr/bin/false".into(),
        herdr_socket_root: data.join("isolated-herdr"),
        installation_secret: None,
    }
}

#[test]
fn lifecycle_startup_factory_runs_under_ownership_and_preserves_its_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    let result = run_with_runtime_factory(dir.path(), |canonical| {
        assert_eq!(canonical, dir.path().canonicalize().unwrap());
        let contender =
            StartupOwner::acquire_until(canonical, Instant::now() + Duration::from_millis(100));
        assert!(matches!(
            contender,
            Err(ServiceError::StartupTimeout { .. })
        ));
        assert!(!canonical.join("kanban.sqlite").exists());
        Err(ServiceError::InstallationSecret)
    });
    assert!(matches!(result, Err(ServiceError::InstallationSecret)));
    assert!(!dir.path().join("kanban.sqlite").exists());
    let recovered = crate::serve_with_runtime(dir.path(), runtime(dir.path())).unwrap();
    recovered.shutdown();
}

#[test]
fn lifecycle_startup_healthy_reuse_never_calls_the_credential_factory() {
    let dir = tempfile::TempDir::new().unwrap();
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    let instance = client.query("health.get")["service"]["started_at"].clone();
    run_with_runtime_factory(dir.path(), |_| {
        panic!("a follower must not access installation credentials")
    })
    .unwrap();
    assert_eq!(
        client.query("health.get")["service"]["started_at"],
        instance
    );
    core.shutdown();
}

#[test]
fn lifecycle_startup_public_serve_paths_wait_without_touching_the_database() {
    let dir = tempfile::TempDir::new().unwrap();
    let owner = StartupOwner::acquire(dir.path()).unwrap();
    let (sent, results) = std::sync::mpsc::channel();
    let workers: Vec<_> = (0..2)
        .map(|index| {
            let data = owner.data_dir.clone();
            let sent = sent.clone();
            std::thread::spawn(move || {
                let result = if index == 0 {
                    crate::serve(&data)
                } else {
                    crate::serve_with_runtime(&data, runtime(&data))
                };
                sent.send(result.map(|core| {
                    core.shutdown();
                }))
                .unwrap();
            })
        })
        .collect();
    assert!(results.recv_timeout(Duration::from_millis(150)).is_err());
    assert!(!owner.data_dir.join("kanban.sqlite").exists());
    assert!(!owner.data_dir.join("backups").exists());
    let data = owner.data_dir.clone();
    let core = crate::serve_owned_with_runtime(owner, runtime(&data)).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            results.recv_timeout(Duration::from_secs(3)).unwrap(),
            Err(ServiceError::AlreadyRunning { .. })
        ));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    core.shutdown();
}

#[test]
fn lifecycle_startup_storage_errors_are_not_reported_as_reuse() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("kanban.sqlite"), b"not a database").unwrap();
    assert!(matches!(
        run_with_runtime(dir.path(), runtime(dir.path())),
        Err(ServiceError::Storage(_))
    ));
    assert!(!dir.path().join("core.sock").exists());
    assert!(StartupOwner::acquire(dir.path()).is_ok());
}

#[test]
fn lifecycle_startup_released_owner_lock_is_reusable_but_live_owner_times_out() {
    let dir = tempfile::TempDir::new().unwrap();
    let owner = StartupOwner::acquire(dir.path()).unwrap();
    let started = Instant::now();
    assert!(matches!(
        StartupOwner::acquire_until(dir.path(), started + Duration::from_millis(100)),
        Err(ServiceError::StartupTimeout { .. })
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(owner);
    let owner = StartupOwner::acquire(dir.path()).unwrap();
    assert_eq!(owner.canonical_dir, dir.path().canonicalize().unwrap());
}

#[test]
fn lifecycle_startup_keeps_a_short_socket_alias_for_a_long_canonical_installation() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = dir.path().join("installation-".repeat(12));
    std::fs::create_dir(&target).unwrap();
    let alias = dir.path().join("short");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    let core = crate::serve_with_runtime(&alias, runtime(&alias))
        .expect("canonical lock identity must not lengthen the socket address");
    assert_eq!(core.socket_path(), alias.join("core.sock"));
    run_with_runtime_factory(&alias, |_| panic!("reuse must not construct credentials")).unwrap();
    core.shutdown();
    assert!(!target.join("core.sock").exists());
}

#[test]
fn lifecycle_startup_lock_rejects_symlinks_without_modifying_their_target() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = dir.path().join("unrelated");
    std::fs::write(&target, b"untouched").unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("core.lock")).unwrap();
    assert!(matches!(
        StartupOwner::acquire(dir.path()),
        Err(ServiceError::StartupIo { .. })
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
    assert!(!dir.path().join("kanban.sqlite").exists());
}

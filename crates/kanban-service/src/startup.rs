use crate::{ServiceError, ServiceRuntime};
use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;

pub struct ServiceOptions {
    pub data_dir: PathBuf,
    pub launch_once: bool,
    pub http: kanban_transport::loopback::LoopbackHttpConfig,
}
impl ServiceOptions {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, ServiceError> {
        let mut args = args.into_iter();
        let mut data_dir = None;
        let mut launch_once = false;
        let mut http = kanban_transport::loopback::LoopbackHttpConfig::default();
        while let Some(arg) = args.next() {
            if arg == "--launch-once" && !launch_once {
                launch_once = true;
            } else if arg == "--data-dir" && data_dir.is_none() {
                data_dir = Some(PathBuf::from(args.next().ok_or(ServiceError::Arguments)?));
            } else if arg == "--loopback-http" && http.bind.is_none() {
                let address = args.next().ok_or(ServiceError::Arguments)?;
                let address = address
                    .to_str()
                    .and_then(|value| value.parse::<std::net::SocketAddr>().ok())
                    .filter(|address| address.ip().is_loopback())
                    .ok_or(ServiceError::Arguments)?;
                http.bind = Some(address);
            } else if data_dir.is_none() && !arg.to_string_lossy().starts_with('-') {
                data_dir = Some(PathBuf::from(arg));
            } else {
                return Err(ServiceError::Arguments);
            }
        }
        let data_dir = match data_dir {
            Some(path) => path,
            None => kanban_storage::paths::managed_data_dir()?,
        };
        if !data_dir.is_absolute() {
            return Err(ServiceError::Arguments);
        }
        Ok(Self {
            data_dir,
            launch_once,
            http,
        })
    }
}

pub fn launch_detached(executable: &Path, data_dir: &Path) -> std::io::Result<Child> {
    launch_detached_with_http(executable, data_dir, Default::default())
}

fn launch_detached_with_http(
    executable: &Path,
    data_dir: &Path,
    http: kanban_transport::loopback::LoopbackHttpConfig,
) -> std::io::Result<Child> {
    let mut command = Command::new(executable);
    command.arg("--data-dir").arg(data_dir);
    if let Some(address) = http.bind {
        command.arg("--loopback-http").arg(address.to_string());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

/// Run production argument orchestration with an installation-owned runtime factory.
pub fn run_with_args(
    args: impl IntoIterator<Item = OsString>,
    executable: &Path,
    runtime: impl FnOnce(&Path) -> Result<ServiceRuntime, ServiceError>,
) -> Result<(), ServiceError> {
    let options = ServiceOptions::parse(args)?;
    if options.launch_once {
        launch_detached_with_http(executable, &options.data_dir, options.http)
            .map_err(|source| ServiceError::DataDir { source })?;
        return Ok(());
    }
    run_with_http_runtime_factory(&options.data_dir, options.http, runtime)
}

pub fn run_with_runtime(data_dir: &Path, runtime: ServiceRuntime) -> Result<(), ServiceError> {
    run_with_runtime_factory(data_dir, |_| Ok(runtime))
}

pub(crate) fn run_with_runtime_factory(
    data_dir: &Path,
    runtime: impl FnOnce(&Path) -> Result<ServiceRuntime, ServiceError>,
) -> Result<(), ServiceError> {
    run_with_http_runtime_factory(data_dir, Default::default(), runtime)
}

fn run_with_http_runtime_factory(
    data_dir: &Path,
    http: kanban_transport::loopback::LoopbackHttpConfig,
    runtime: impl FnOnce(&Path) -> Result<ServiceRuntime, ServiceError>,
) -> Result<(), ServiceError> {
    let owner = match StartupOwner::acquire(data_dir) {
        Ok(owner) => owner,
        Err(ServiceError::AlreadyRunning { .. }) => return Ok(()),
        Err(error) => return Err(error),
    };
    let runtime = runtime(&owner.canonical_dir)?;
    crate::serve_owned_with_http(owner, runtime, http)?.wait_for_stop();
    Ok(())
}

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_POLL: Duration = Duration::from_millis(20);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(200);
const MAX_HEALTH_FRAME: usize = 1024 * 1024;

pub(crate) struct StartupOwner {
    pub(crate) data_dir: PathBuf,
    canonical_dir: PathBuf,
    // Never unlink this file: waiters must keep locking the same inode.
    _lock: File,
}

impl StartupOwner {
    pub(crate) fn acquire(data_dir: &Path) -> Result<Self, ServiceError> {
        Self::acquire_until(data_dir, Instant::now() + STARTUP_TIMEOUT)
    }

    fn acquire_until(data_dir: &Path, deadline: Instant) -> Result<Self, ServiceError> {
        let io = |source| ServiceError::StartupIo { source };
        std::fs::create_dir_all(data_dir).map_err(io)?;
        let selected_dir = data_dir.to_path_buf();
        let data_dir = data_dir.canonicalize().map_err(io)?;
        std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(data_dir.join("core.lock"))
            .map_err(io)?;
        if !lock.metadata().map_err(io)?.is_file() {
            return Err(io(std::io::Error::other(
                "the startup lock is not a regular file",
            )));
        }
        lock.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io)?;
        // Canonical identity must not lengthen a usable Unix socket address.
        let socket_path = selected_dir.join(kanban_transport::SOCKET_FILE_NAME);
        let mut locked = false;
        loop {
            if !locked {
                match lock.try_lock() {
                    Ok(()) => locked = true,
                    Err(TryLockError::WouldBlock) => {}
                    Err(TryLockError::Error(source)) => return Err(io(source)),
                }
            }
            match std::fs::symlink_metadata(&socket_path) {
                Ok(metadata) if !metadata.file_type().is_socket() => {
                    return Err(kanban_transport::TransportError::SocketPathOccupied {
                        path: socket_path,
                    }
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(error)),
                Ok(_) => {}
            }
            match probe_core(&socket_path, deadline.min(Instant::now() + HEALTH_TIMEOUT))
                .map_err(io)?
            {
                CoreReadiness::Healthy => {
                    return Err(ServiceError::AlreadyRunning { path: socket_path });
                }
                CoreReadiness::Absent if locked => {
                    return Ok(Self {
                        data_dir: selected_dir,
                        canonical_dir: data_dir,
                        _lock: lock,
                    });
                }
                CoreReadiness::Absent | CoreReadiness::Waiting => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ServiceError::StartupTimeout { path: socket_path });
            }
            std::thread::sleep(remaining.min(STARTUP_POLL));
        }
    }
}

enum CoreReadiness {
    Absent,
    Waiting,
    Healthy,
}

fn probe_core(path: &Path, deadline: Instant) -> std::io::Result<CoreReadiness> {
    use socket2::{Domain, SockAddr, Socket, Type};
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Ok(CoreReadiness::Waiting);
    }
    match socket.connect_timeout(&SockAddr::unix(path)?, remaining) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            return Ok(CoreReadiness::Absent);
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            return Ok(CoreReadiness::Waiting);
        }
        Err(error) => return Err(error),
    }
    socket.set_nonblocking(true)?; // cspell:ignore nonblocking
    let mut stream = UnixStream::from(std::os::fd::OwnedFd::from(socket));
    if stream
        .write_all(b"{\"kind\":\"query\",\"operation\":\"health.get\",\"payload\":{}}\n")
        .is_err()
    {
        return Ok(CoreReadiness::Waiting);
    }
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    while Instant::now() < deadline && bytes.len() < MAX_HEALTH_FRAME {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(CoreReadiness::Waiting),
            Ok(count) => {
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                    let healthy = match serde_json::from_slice::<kanban_transport::ResponseFrame>(
                        &bytes[..end],
                    ) {
                        Ok(kanban_transport::ResponseFrame::Response { payload }) => {
                            serde_json::from_value::<kanban_dto::HealthResponse>(payload).is_ok_and(
                                |health| {
                                    health.connected
                                        && !health.service.started_at.is_empty()
                                        && health.database.journal_mode == "wal"
                                        && health.database.schema_version
                                            == kanban_storage::migrations::LATEST_SCHEMA_VERSION
                                },
                            )
                        }
                        _ => false,
                    };
                    return Ok(if healthy {
                        CoreReadiness::Healthy
                    } else {
                        CoreReadiness::Waiting
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(
                    STARTUP_POLL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(_) => return Ok(CoreReadiness::Waiting),
        }
    }
    Ok(CoreReadiness::Waiting)
}

//! Core-owned stdio children receive only an inherited run-scoped channel.
use kanban_app::{Core, agent_authorization::AgentSession};
use kanban_domain::CapabilityId;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;

pub struct ManagedAdapters {
    core: Arc<Core>,
    executable: std::path::PathBuf,
    secret: Option<Arc<kanban_app::secrets::InstallationSecret>>,
}

impl ManagedAdapters {
    pub fn new(
        core: Arc<Core>,
        executable: std::path::PathBuf,
        secret: Option<Arc<kanban_app::secrets::InstallationSecret>>,
    ) -> Self {
        Self {
            core,
            executable,
            secret,
        }
    }
}

impl kanban_transport::agent::AgentLauncher for ManagedAdapters {
    fn serve(&self, capability_id: u64, mut channel: io::BufReader<UnixStream>) -> io::Result<()> {
        use io::Write;
        let mut child = spawn_adapter(
            self.core.clone(),
            CapabilityId::new(capability_id),
            &self.executable,
        )?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("adapter input unavailable"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("adapter output unavailable"))?;
        let mut write_half = channel.get_ref().try_clone()?;
        let response = kanban_transport::ResponseFrame::Response {
            payload: serde_json::json!({}),
        };
        writeln!(write_half, "{}", serde_json::to_string(&response)?)?;
        let secret = self.secret.clone();
        let writer = std::thread::Builder::new()
            .name("kanban-mcp-output".into())
            .spawn(move || {
                let _ = forward_frames(
                    &mut io::BufReader::new(output),
                    &mut write_half,
                    secret.as_deref(),
                );
                let _ = write_half.shutdown(std::net::Shutdown::Both);
            })?;
        let _ = forward_frames(&mut channel, &mut input, self.secret.as_deref());
        drop(input);
        drop(child);
        let _ = writer.join();
        Ok(())
    }
}

/// Check complete envelopes outside the SDK: protocol metadata and request IDs
/// may be reflected before a tool handler runs. The child never holds the key.
fn forward_frames(
    reader: &mut impl io::BufRead,
    writer: &mut impl io::Write,
    secret: Option<&kanban_app::secrets::InstallationSecret>,
) -> io::Result<()> {
    use io::{BufRead, Read};
    const LIMIT: u64 = 16 * 1024 * 1024;
    loop {
        let mut frame = Vec::new();
        (&mut *reader)
            .take(LIMIT + 1)
            .read_until(b'\n', &mut frame)?;
        if frame.is_empty() {
            return Ok(());
        }
        if frame.len() as u64 > LIMIT || !frame.ends_with(b"\n") {
            return Err(io::Error::other("MCP frame is invalid"));
        }
        let value: serde_json::Value =
            serde_json::from_slice(&frame).map_err(|_| io::Error::other("MCP frame is invalid"))?;
        if secret.is_some_and(|secret| value.to_string().contains(secret.expose())) {
            return Err(io::Error::other("MCP frame contains protected material"));
        }
        writer.write_all(&frame)?;
        writer.flush()?;
    }
}

pub fn spawn_adapter(
    core: Arc<Core>,
    capability: CapabilityId,
    executable: &Path,
) -> io::Result<AdapterProcess> {
    let session = AgentSession::new(core, capability)
        .map_err(|_| io::Error::other("run access is unavailable"))?;
    let (parent, child) = UnixStream::pair()?;
    // Keep the source above fd 3 so dup2 always clears close-on-exec there.
    let fd = unsafe { libc::fcntl(child.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 4) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl returned a new owned descriptor, distinct from `child`.
    let inherited = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut command = Command::new(executable);
    command
        .arg("--application-channel")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: the child closure uses only async-signal-safe dup2; the source
    // remains owned in this process until spawn returns.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(fd, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let shutdown = Arc::new(parent.try_clone()?);
    let mut process = command.spawn()?;
    drop(inherited);
    drop(child);
    let worker_shutdown = shutdown.clone();
    let worker = match std::thread::Builder::new()
        .name("kanban-agent-channel".to_owned())
        .spawn(move || {
            let _ = kanban_transport::agent::serve_agent_channel(parent, session);
            // The owner retains a socket clone. Closing this worker's handle
            // alone cannot wake a child waiting for the failed response.
            let _ = worker_shutdown.shutdown(std::net::Shutdown::Both);
        }) {
        Ok(worker) => worker,
        Err(error) => {
            let _ = process.kill();
            let _ = process.wait();
            return Err(error);
        }
    };
    Ok(AdapterProcess {
        stdin: process.stdin.take(),
        stdout: process.stdout.take(),
        process,
        shutdown,
        worker: Some(worker),
    })
}

/// Dropping the owner closes its private channel, kills and reaps the child,
/// and joins the serving thread. A disconnected client cannot strand either.
pub struct AdapterProcess {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    process: Child,
    shutdown: Arc<UnixStream>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl AdapterProcess {
    pub fn id(&self) -> u32 {
        self.process.id()
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.process.kill()
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.process.wait()
    }
}
impl Drop for AdapterProcess {
    fn drop(&mut self) {
        self.stdin.take();
        self.stdout.take();
        let _ = self.shutdown.shutdown(std::net::Shutdown::Both);
        let _ = self.process.kill();
        let _ = self.process.wait();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

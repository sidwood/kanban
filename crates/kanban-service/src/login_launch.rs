use kanban_app::service_lifecycle::LoginLaunchPort;
use kanban_dto::ApiError;
use std::path::PathBuf;
use std::process::Command;

pub struct NativeLoginLaunch {
    executable: PathBuf,
    data_dir: PathBuf,
    plist: PathBuf,
    label: String,
    launchctl: PathBuf,
}
impl NativeLoginLaunch {
    pub fn production(data_dir: PathBuf) -> Result<Self, ApiError> {
        let home =
            std::env::var_os("HOME").ok_or_else(|| failure("home directory is unavailable"))?;
        let executable =
            std::env::current_exe().map_err(|_| failure("service executable is unavailable"))?;
        Ok(Self::for_installation(
            executable,
            data_dir,
            PathBuf::from(home).join("Library/LaunchAgents"),
        ))
    }
    pub fn for_installation(executable: PathBuf, data_dir: PathBuf, directory: PathBuf) -> Self {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let label = format!(
            "dev.kanban.core.{:x}",
            Sha256::digest(data_dir.as_os_str().as_bytes())
        );
        Self {
            executable,
            data_dir,
            plist: directory.join(format!("{label}.plist")),
            label,
            launchctl: "/bin/launchctl".into(),
        }
    }
    #[cfg(test)]
    pub(crate) fn isolated(
        executable: PathBuf,
        data_dir: PathBuf,
        directory: PathBuf,
        label: String,
        launchctl: PathBuf,
    ) -> Self {
        Self {
            executable,
            data_dir,
            plist: directory.join(format!("{label}.plist")),
            label,
            launchctl,
        }
    }
    fn domain(&self) -> String {
        format!("gui/{}", unsafe { libc::geteuid() })
    }
    fn target(&self) -> String {
        format!("{}/{}", self.domain(), self.label)
    }
    fn configuration(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{}</string>
<key>ProgramArguments</key><array><string>{}</string><string>--launch-once</string><string>--data-dir</string><string>{}</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><false/>
</dict></plist>
"#,
            xml(&self.label),
            xml(&self.executable.to_string_lossy()),
            xml(&self.data_dir.to_string_lossy())
        )
    }
    fn loaded(&self) -> Result<bool, ApiError> {
        let output = Command::new(&self.launchctl)
            .arg("print")
            .arg(self.target())
            .output()
            .map_err(|_| failure("launchd readback failed"))?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let canonical = self
                .plist
                .canonicalize()
                .map_err(|_| failure("registered login configuration is missing"))?;
            if !text
                .lines()
                .any(|line| line.trim() == format!("path = {}", canonical.display()))
                || !text
                    .lines()
                    .any(|line| line.trim() == format!("program = {}", self.executable.display()))
                || !text
                    .lines()
                    .any(|line| line.trim() == self.data_dir.to_string_lossy())
            {
                return Err(failure(
                    "launchd registration does not match this installation",
                ));
            }
            Ok(true)
        } else if output.status.code() == Some(113) {
            Ok(false)
        } else {
            Err(failure("launchd readback failed"))
        }
    }
    fn bootstrap(&self) -> Result<(), ApiError> {
        let result = Command::new(&self.launchctl)
            .arg("bootstrap")
            .arg(self.domain())
            .arg(&self.plist)
            .output()
            .map_err(|_| failure("launchd registration failed"))?;
        if result.status.success() {
            Ok(())
        } else {
            Err(failure("launchd registration failed"))
        }
    }
    fn bootout(&self) -> Result<(), ApiError> {
        let result = Command::new(&self.launchctl)
            .arg("bootout")
            .arg(self.target())
            .output()
            .map_err(|_| failure("launchd removal failed"))?;
        if result.status.success() || result.status.code() == Some(113) {
            Ok(())
        } else {
            Err(failure("launchd removal failed"))
        }
    }
}
impl LoginLaunchPort for NativeLoginLaunch {
    fn enabled(&self) -> Result<bool, ApiError> {
        if !cfg!(target_os = "macos") {
            return Err(failure("launch at login requires macOS"));
        }
        let loaded = self.loaded()?;
        match std::fs::read_to_string(&self.plist) {
            Ok(content) if content == self.configuration() && loaded => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !loaded => Ok(false),
            _ => Err(failure(
                "launch at login configuration and OS state disagree",
            )),
        }
    }
    fn set_enabled(&self, enabled: bool) -> Result<(), ApiError> {
        match self.enabled() {
            Ok(actual) if actual == enabled => return Ok(()),
            Ok(_) => {}
            Err(error) => {
                if !enabled
                    && !self.loaded()?
                    && std::fs::read_to_string(&self.plist).ok().as_deref()
                        == Some(self.configuration().as_str())
                {
                    std::fs::remove_file(&self.plist)
                        .map_err(|_| failure("login configuration could not be removed"))?;
                    return self
                        .enabled()
                        .and_then(|actual| if !actual { Ok(()) } else { Err(error) });
                }
                return Err(error);
            }
        }
        if enabled {
            std::fs::create_dir_all(self.plist.parent().expect("plist has parent"))
                .map_err(|_| failure("LaunchAgents directory is unavailable"))?;
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let temporary = self.plist.with_extension("plist.pending");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|_| failure("login configuration could not be created"))?;
            let written = file
                .write_all(self.configuration().as_bytes())
                .and_then(|_| file.sync_all())
                .and_then(|_| std::fs::rename(&temporary, &self.plist));
            let _ = std::fs::remove_file(&temporary);
            written.map_err(|_| failure("login configuration could not be written"))?;
            if let Err(error) = self.bootstrap().and_then(|_| {
                self.enabled().and_then(|actual| {
                    if actual {
                        Ok(())
                    } else {
                        Err(failure("registration was not observed"))
                    }
                })
            }) {
                if self.bootout().is_ok() {
                    let _ = std::fs::remove_file(&self.plist);
                }
                return Err(error);
            }
        } else {
            self.bootout()?;
            if std::fs::remove_file(&self.plist).is_err() {
                let _ = self.bootstrap();
                return Err(failure("login configuration could not be removed"));
            }
            if self.enabled()? {
                return Err(failure("login removal was not observed"));
            }
        }
        Ok(())
    }
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}
fn failure(message: &str) -> ApiError {
    ApiError::internal(message)
}

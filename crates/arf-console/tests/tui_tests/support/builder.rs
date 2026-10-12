use super::terminal::Terminal;
use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tui_test::{AutomaticRecording, AutomaticRecordingMode, OpenOptions, RunOptions, Session};

pub const DEFAULT_CONFIG: &str = r#"[prompt]
format = '{status}ARF> '
[prompt.status.symbol]
error = 'ERR '
"#;

/// Resources which can safely be shared by several terminal sessions.
///
/// In particular, the IPC server uses one directory for all sessions. A
/// session still gets its own working directory, config and history directory
/// so parallel cases cannot overwrite one another's files.
#[derive(Clone)]
pub struct SharedResources {
    root: Arc<TempDir>,
    sessions_dir: PathBuf,
}

impl SharedResources {
    pub fn new() -> Result<Self> {
        let root = Arc::new(tempfile::tempdir()?);
        let sessions_dir = root.path().join("sessions");
        fs::create_dir_all(&sessions_dir)?;
        Ok(Self { root, sessions_dir })
    }

    pub(super) fn session_work(&self) -> Result<TempDir> {
        Ok(tempfile::tempdir_in(self.root.path())?)
    }

    pub(super) fn sessions_dir(&self) -> &std::path::Path {
        &self.sessions_dir
    }
}

/// Configuration used to launch one real arf process.
pub struct TerminalBuilder {
    pub(super) name: String,
    args: Vec<String>,
    config: Option<ConfigSource>,
    config_from_env: bool,
    env: Vec<(String, String)>,
    #[cfg(unix)]
    env_remove: Vec<String>,
    cwd: Option<PathBuf>,
    cols: u16,
    rows: u16,
    history_dir: Option<PathBuf>,
    artifacts: Option<PathBuf>,
    resources: Option<SharedResources>,
    vanilla: bool,
    r_source_overrides: bool,
}

enum ConfigSource {
    Contents(String),
    Path(PathBuf),
}

impl TerminalBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
            config: None,
            config_from_env: false,
            env: Vec::new(),
            #[cfg(unix)]
            env_remove: Vec::new(),
            cwd: None,
            cols: 100,
            rows: 32,
            history_dir: None,
            artifacts: None,
            resources: None,
            vanilla: true,
            r_source_overrides: false,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Evaluate configured R source providers in cases that test selection.
    pub fn with_r_source_overrides(mut self) -> Self {
        self.r_source_overrides = true;
        self
    }

    /// Use TOML contents for the process-local config file.
    pub fn config(mut self, config: impl Into<String>) -> Self {
        self.config = Some(ConfigSource::Contents(config.into()));
        self
    }

    /// Use an existing config file without copying it into the session workdir.
    pub fn config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config = Some(ConfigSource::Path(path.into()));
        self
    }

    /// Select the generated config file through ARF_CONFIG instead of --config.
    pub fn config_from_env(mut self) -> Self {
        self.config_from_env = true;
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Remove inherited environment variables before starting arf.
    #[cfg(unix)]
    pub fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }

    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    pub fn cols(mut self, cols: u16) -> Self {
        self.cols = cols;
        self
    }

    pub fn rows(mut self, rows: u16) -> Self {
        self.rows = rows;
        self
    }

    pub fn history_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.history_dir = Some(path.into());
        self
    }

    pub fn artifacts(mut self, path: impl Into<PathBuf>) -> Self {
        self.artifacts = Some(path.into());
        self
    }

    pub fn resources(mut self, resources: SharedResources) -> Self {
        self.resources = Some(resources);
        self
    }

    /// Keep the default isolated startup flags unless a startup profile must
    /// be exercised by a test.
    pub fn vanilla(mut self, enabled: bool) -> Self {
        self.vanilla = enabled;
        self
    }

    pub(super) fn ensure_artifacts(&mut self) -> Result<PathBuf> {
        if let Some(path) = &self.artifacts {
            fs::create_dir_all(path)?;
            return Ok(path.clone());
        }
        let root = std::env::var_os("ARF_TUI_TEST_ARTIFACTS")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&root)?;
        let path = tempfile::Builder::new()
            .prefix(&format!("arf-tui-{}-", self.name))
            .tempdir_in(root)?
            .keep();
        self.artifacts = Some(path.clone());
        Ok(path)
    }

    /// Spawn arf and return before waiting for its first prompt.
    pub fn spawn(mut self) -> Result<Terminal> {
        let cols = self.cols;
        let rows = self.rows;
        let artifacts = self.ensure_artifacts()?;
        let resources = match self.resources {
            Some(resources) => resources,
            None => SharedResources::new()?,
        };
        let work = resources.session_work()?;
        let config = match self.config {
            Some(ConfigSource::Contents(contents)) => {
                let path = work.path().join("config.toml");
                fs::write(&path, contents)?;
                path
            }
            Some(ConfigSource::Path(path)) => {
                if path.is_absolute() {
                    path
                } else {
                    // Resolve before launching the child: its cwd may be a
                    // session-specific temporary directory.
                    std::env::current_dir()?.join(path)
                }
            }
            None => {
                let path = work.path().join("config.toml");
                fs::write(&path, DEFAULT_CONFIG)?;
                path
            }
        };
        let history_dir = self
            .history_dir
            .unwrap_or_else(|| work.path().join("history"));
        let defaults = OpenOptions::default();
        let mut args = Vec::new();
        if !self.r_source_overrides {
            args.push("--no-r-source-overrides".to_owned());
        }
        if self.vanilla {
            args.insert(0, "--vanilla".to_owned());
        }
        if self.config_from_env {
            if !self.env.iter().any(|(key, _)| key == "ARF_CONFIG") {
                self.env.push((
                    "ARF_CONFIG".to_owned(),
                    config.to_string_lossy().into_owned(),
                ));
            }
        } else {
            args.extend(["--config".to_owned(), config.to_string_lossy().into_owned()]);
        }
        args.extend([
            "--history-dir".to_owned(),
            history_dir.to_string_lossy().into_owned(),
        ]);
        args.extend(self.args);
        #[cfg(unix)]
        let program = if self.env_remove.is_empty() {
            env!("CARGO_BIN_EXE_arf").to_owned()
        } else {
            let mut wrapper_args = Vec::with_capacity(self.env_remove.len() * 2 + args.len() + 1);
            for key in self.env_remove {
                wrapper_args.push("-u".to_owned());
                wrapper_args.push(key);
            }
            wrapper_args.push(env!("CARGO_BIN_EXE_arf").to_owned());
            wrapper_args.extend(args);
            args = wrapper_args;
            "/usr/bin/env".to_owned()
        };
        #[cfg(not(unix))]
        let program = env!("CARGO_BIN_EXE_arf").to_owned();
        let cwd = self.cwd.unwrap_or_else(|| work.path().to_path_buf());
        let mut env = self.env;
        if let Some((_, value)) = env
            .iter_mut()
            .find(|(key, _)| key == "ARF_IPC_SESSIONS_DIR")
        {
            *value = resources.sessions_dir().to_string_lossy().into_owned();
        } else {
            env.push((
                "ARF_IPC_SESSIONS_DIR".into(),
                resources.sessions_dir().to_string_lossy().into_owned(),
            ));
        }
        let mut terminal = Terminal {
            session: Session::new(&self.name),
            artifacts,
            work,
            resources,
            cwd: cwd.clone(),
            pid: None,
            closed: false,
        };
        terminal.stage("spawn")?;
        let opened = terminal.session.run(RunOptions {
            backend: defaults.backend,
            program,
            args,
            profile: defaults.profile,
            cols,
            rows,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            env,
            wait_ready: Some(false),
            restart: false,
            timeouts: defaults.timeouts,
            recording: AutomaticRecording {
                mode: AutomaticRecordingMode::Always,
                directory: Some(terminal.artifacts.clone()),
            },
        })?;
        terminal.pid = opened.shell_pid;
        Ok(terminal)
    }
}

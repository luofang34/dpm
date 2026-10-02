//! Who hosts a provider session: the evidence a recovery needs before it touches a run.
//!
//! A run that has not ended is not proof that whatever hosts its provider has stopped: telemetry
//! goes quiet for many reasons. So an adapter keeps a small host record for each provider session,
//! under the canonical identity of the store the run belongs to, and holds an exclusive advisory
//! lock on it while it hosts the session. The record says whether a provider is running and which
//! process. The kernel releases the lock when the adapter dies, which proves that adapter process
//! is gone and nothing more: it does not prove that its provider, or anything the provider
//! started, has stopped.
//!
//! A recovery therefore fails closed. It proceeds only if the record exists and is well formed,
//! it can take the lock, and the record proves no provider can be running: either none was
//! recorded, or the recorded process is shown by the operating system not to exist. A missing or
//! corrupt record, a provider that was about to be started when the host died, a process that still
//! exists, and a process whose existence cannot be determined all refuse and change nothing.
//! Process identities can be reused, which can only make a recovery refuse when it need not.
//!
//! The claim is released explicitly when the host is dropped, so it does not wait for a process
//! forked meanwhile to close its copy of the descriptor.
//!
//! The lock and the record are a same-machine, local-filesystem guarantee; they do not extend to
//! hosts that share a store over a network.

use rustix::{
    io::Errno,
    process::{Pid, test_kill_process},
};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

/// The most a host record can hold; a longer file is not one.
const RECORD_BYTES: u64 = 512;

/// What a host record says about the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRecord {
    /// No provider is running.
    Nothing,
    /// A provider was about to be started, or was started and its identity not yet recorded: it
    /// may be running and nothing says which process.
    Starting,
    /// A provider was started with this process identity.
    Running(u32),
}

/// Why a session cannot be hosted or recovered.
#[derive(Debug, Error)]
pub enum HostError {
    /// Another adapter process holds the session.
    #[error("another adapter process is hosting session {session}")]
    Active {
        /// The provider session.
        session: String,
    },
    /// A new session's record already exists, so the name is not new.
    #[error("a host record for session {session} already exists")]
    Exists {
        /// The provider session.
        session: String,
    },
    /// There is no record of who hosted the session, so nothing can be shown about its provider.
    #[error(
        "there is no host record for session {session}; nothing shows that its provider stopped"
    )]
    NoRecord {
        /// The provider session.
        session: String,
    },
    /// The record cannot be read as a host record.
    #[error(
        "the host record of session {session} is unreadable ({why}); nothing shows that its provider stopped"
    )]
    Corrupt {
        /// The provider session.
        session: String,
        /// What is wrong with it.
        why: &'static str,
    },
    /// The last host was starting a provider when it ended.
    #[error(
        "the last host of session {session} was starting a provider when it ended; that provider may still run"
    )]
    ProviderStarting {
        /// The provider session.
        session: String,
    },
    /// The provider the last host recorded still exists, or its existence cannot be determined.
    #[error(
        "the provider process {pid} that the last host of session {session} recorded may still be running ({state})"
    )]
    ProviderMayStillRun {
        /// The provider session.
        session: String,
        /// The recorded provider process.
        pid: u32,
        /// What the operating system said.
        state: &'static str,
    },
    /// The session is not a name this contract can use.
    #[error("{0:?} is not a usable session name")]
    BadSession(String),
    /// The record could not be used.
    #[error("host record {path}: {source}")]
    Io {
        /// The record.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
}

/// A held claim on one provider session.
#[derive(Debug)]
pub struct Hosted {
    file: File,
    path: PathBuf,
}

impl Drop for Hosted {
    /// Release the claim now. Closing the descriptor would not do it while a copy is open, and a
    /// process forked by another thread holds a copy of every descriptor until it execs; a lock
    /// belongs to the open file, so unlocking through this descriptor releases it for every copy.
    fn drop(&mut self) {
        if let Err(error) = self.file.unlock() {
            tracing::warn!(path = %self.path.display(), %error, "the host claim could not be released");
        }
    }
}

/// Where the host records of the store at `store` live, tied to the store's canonical identity and
/// not to whoever is asking or from which directory.
pub fn directory(store: &Path) -> io::Result<PathBuf> {
    let store = fs::canonicalize(store)?;
    let name = store
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the store has no file name"))?;
    let parent = store
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the store has no directory"))?;
    Ok(parent.join(".dpm-claude").join(name))
}

fn usable(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 100
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn parse(text: &str) -> Result<ProviderRecord, &'static str> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| "not JSON")?;
    if value.get("v").and_then(serde_json::Value::as_u64) != Some(1)
        || value
            .get("adapter_pid")
            .and_then(serde_json::Value::as_u64)
            .is_none()
    {
        return Err("not a version 1 host record");
    }
    let provider = value.get("provider").ok_or("no provider state")?;
    match provider.get("state").and_then(serde_json::Value::as_str) {
        Some("none") => Ok(ProviderRecord::Nothing),
        Some("starting") => Ok(ProviderRecord::Starting),
        Some("running") => provider
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
            .map(ProviderRecord::Running)
            .ok_or("a running provider without a process identity"),
        _ => Err("an unknown provider state"),
    }
}

/// Whether the operating system says the process exists. Only an explicit "no such process" is
/// absence; permission or any other answer means it exists or cannot be told.
fn existence(pid: u32) -> Result<(), &'static str> {
    let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return Err("not a process identity");
    };
    match test_kill_process(pid) {
        Ok(()) => Err("it exists"),
        Err(Errno::SRCH) => Ok(()),
        Err(_) => Err("its existence cannot be determined"),
    }
}

impl Hosted {
    fn open(path: PathBuf, session: &str, create: bool) -> Result<(File, PathBuf), HostError> {
        let io = |source| HostError::Io {
            path: path.clone(),
            source,
        };
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        if create {
            options.create_new(true);
        }
        match options.open(&path) {
            Ok(file) => Ok((file, path.clone())),
            Err(error) if create && error.kind() == io::ErrorKind::AlreadyExists => {
                Err(HostError::Exists {
                    session: session.to_string(),
                })
            }
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                Err(HostError::NoRecord {
                    session: session.to_string(),
                })
            }
            Err(error) => Err(io(error)),
        }
    }

    fn lock(file: &File, path: &Path, session: &str) -> Result<(), HostError> {
        match file.try_lock() {
            Ok(()) => Ok(()),
            Err(TryLockError::WouldBlock) => Err(HostError::Active {
                session: session.to_string(),
            }),
            Err(TryLockError::Error(source)) => Err(HostError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Host a new session: its record must not exist yet.
    pub fn begin_fresh_blocking(directory: &Path, session: &str) -> Result<Self, HostError> {
        if !usable(session) {
            return Err(HostError::BadSession(session.to_string()));
        }
        fs::create_dir_all(directory).map_err(|source| HostError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let (file, path) = Self::open(directory.join(format!("{session}.lock")), session, true)?;
        Self::lock(&file, &path, session)?;
        let mut hosted = Self { file, path };
        hosted.record(ProviderRecord::Nothing)?;
        Ok(hosted)
    }

    /// Take over an existing session, only if its record proves no provider can be running.
    pub fn recover_blocking(directory: &Path, session: &str) -> Result<Self, HostError> {
        if !usable(session) {
            return Err(HostError::BadSession(session.to_string()));
        }
        let (mut file, path) =
            Self::open(directory.join(format!("{session}.lock")), session, false)?;
        Self::lock(&file, &path, session)?;
        let io = |source| HostError::Io {
            path: path.clone(),
            source,
        };
        let corrupt = |why| HostError::Corrupt {
            session: session.to_string(),
            why,
        };
        let mut text = String::new();
        file.seek(SeekFrom::Start(0)).map_err(io)?;
        (&mut file)
            .take(RECORD_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|_| corrupt("not readable text"))?;
        if text.len() as u64 > RECORD_BYTES {
            return Err(corrupt("longer than a host record"));
        }
        match parse(&text).map_err(corrupt)? {
            ProviderRecord::Nothing => {}
            ProviderRecord::Starting => {
                return Err(HostError::ProviderStarting {
                    session: session.to_string(),
                });
            }
            ProviderRecord::Running(pid) => {
                existence(pid).map_err(|state| HostError::ProviderMayStillRun {
                    session: session.to_string(),
                    pid,
                    state,
                })?
            }
        }
        let mut hosted = Self { file, path };
        hosted.record(ProviderRecord::Nothing)?;
        Ok(hosted)
    }

    /// Record what is now known about the provider. Called before a provider is started, with
    /// [`ProviderRecord::Starting`], so that a host that dies before it can say which process it
    /// started is not taken for one that started none.
    pub fn record(&mut self, provider: ProviderRecord) -> Result<(), HostError> {
        let state = match provider {
            ProviderRecord::Nothing => serde_json::json!({"state": "none"}),
            ProviderRecord::Starting => serde_json::json!({"state": "starting"}),
            ProviderRecord::Running(pid) => serde_json::json!({"state": "running", "pid": pid}),
        };
        let text =
            serde_json::json!({"v": 1, "adapter_pid": std::process::id(), "provider": state})
                .to_string();
        let io = |source| HostError::Io {
            path: self.path.clone(),
            source,
        };
        self.file.set_len(0).map_err(io)?;
        self.file.seek(SeekFrom::Start(0)).map_err(io)?;
        self.file
            .write_all(text.as_bytes())
            .and_then(|()| self.file.sync_data())
            .map_err(io)
    }
}

#[cfg(test)]
mod tests;

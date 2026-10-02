//! One owned provider process with bounded pending work, bounded input and bounded cleanup.
//!
//! The adapter starts the provider, reads its standard output and error on two named threads, and
//! ends it within bounds: input closed, a grace period, then a kill. Pending work is bounded by the
//! output queue: a reader that is ahead of the consumer blocks on the full queue, which stops it
//! reading the pipe, which in turn makes the provider wait; nothing is dropped to make room.
//!
//! Nothing here can block without a way out. All three pipes are nonblocking. Reads wait on `poll`
//! with a short timeout and look at a stop flag between waits, so a descendant of the provider that
//! holds an inherited pipe open cannot keep a reader alive past the stop. Writes wait for the pipe
//! to be writable against an absolute deadline and a stop check, handle partial writes, and give up
//! with a timeout, so a provider that has stopped reading its input cannot hold the adapter past
//! its bounds whatever the pipe's capacity is. The total written is also bounded, as a resource
//! limit and not as a guarantee against blocking. Standard error is counted and never kept. Only
//! the child this adapter started is ever signalled; the provider's own children are the
//! provider's, and a process group is not killed.

use crate::{
    frame::{Frame, LineReader},
    intake::Exit,
};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
};
use std::{
    ffi::OsString,
    io::{self, BufReader, Read, Write},
    os::{fd::AsFd, unix::process::ExitStatusExt},
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use thiserror::Error;

/// Most bytes the adapter writes to the provider's input in a run: a resource limit on what a run
/// may send, independent of how long any one write may wait.
pub const INPUT_BUDGET: usize = 1024 * 1024;

/// How often a blocked read looks at the stop flag.
const WATCH: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 100_000_000,
};

/// What to run.
#[derive(Debug, Clone)]
pub struct Launch {
    /// The provider executable.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
    /// Its working directory, which is the only place its file tools may reach.
    pub directory: PathBuf,
    /// Environment variables removed for the child; the rest are inherited so the runtime finds
    /// its own credentials, which this adapter never reads.
    pub remove_env: Vec<String>,
}

/// Why the provider could not be started or written to.
#[derive(Debug, Error)]
pub enum ProcessError {
    /// The executable could not be started.
    #[error("could not start {program}: {source}")]
    Spawn {
        /// The executable.
        program: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// A pipe thread could not be started.
    #[error("could not start the {name} thread: {source}")]
    Thread {
        /// Which thread.
        name: &'static str,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// Writing to the provider's input failed.
    #[error("could not write to the provider: {0}")]
    Write(#[source] io::Error),
    /// The provider's input did not become writable in time: it has stopped reading.
    #[error("the provider did not read its input within the time allowed")]
    Timeout,
    /// The write was abandoned because the run is stopping.
    #[error("the write was abandoned because the run is stopping")]
    Abandoned,
    /// The run's bounded input budget is spent.
    #[error("the provider's input budget of {budget} bytes is spent")]
    Budget {
        /// The budget.
        budget: usize,
    },
    /// A pipe could not be made nonblocking.
    #[error("could not make a provider pipe nonblocking: {0}")]
    Nonblocking(#[source] io::Error),
}

/// One thing the reader hands over.
#[derive(Debug)]
pub enum Message {
    /// A frame of the provider's output.
    Frame(Frame),
    /// The output ended cleanly between lines.
    End,
    /// Reading the output failed.
    Failed(String),
}

/// What waiting for the next message found.
#[derive(Debug)]
pub enum Received {
    /// A message.
    Message(Message),
    /// Nothing arrived in time.
    Timeout,
}

/// How a stop went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stopped {
    /// How the provider ended; empty when it could not be reaped.
    pub exit: Exit,
    /// Whether it had to be killed instead of exiting after its input closed.
    pub killed: bool,
    /// Whether the provider was reaped. False is an unresolved process, reported and never hidden.
    pub reaped: bool,
    /// Whether its output had not ended when it was stopped, which means something it started may
    /// still hold its pipes.
    pub output_open: bool,
}

/// A read end that waits on `poll`, so a stop is noticed within [`WATCH`] whatever holds the pipe.
struct Polled<R> {
    inner: R,
    stop: Arc<AtomicBool>,
}

impl<R: Read + AsFd> Read for Polled<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "stopped"));
            }
            let mut watched = [PollFd::new(&self.inner, PollFlags::IN)];
            match poll(&mut watched, Some(&WATCH)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => {}
                Ok(_) => match self.inner.read(buffer) {
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    other => return other,
                },
                Err(error) => return Err(error.into()),
            }
        }
    }
}

/// Make a pipe end nonblocking, so no read or write on it can wait without a deadline.
fn nonblocking(end: &impl AsFd) -> Result<(), ProcessError> {
    let flags = fcntl_getfl(end).map_err(|error| ProcessError::Nonblocking(error.into()))?;
    fcntl_setfl(end, flags | OFlags::NONBLOCK)
        .map_err(|error| ProcessError::Nonblocking(error.into()))
}

/// Shared counters of the pipe threads.
#[derive(Debug, Default)]
struct Gauges {
    pending: AtomicUsize,
    high_water: AtomicUsize,
    stderr_bytes: AtomicU64,
    output_ended: AtomicBool,
}

fn read_output(
    output: Polled<std::process::ChildStdout>,
    limit: usize,
    messages: &SyncSender<Message>,
    gauges: &Gauges,
) {
    let mut lines = LineReader::new(BufReader::new(output), limit);
    loop {
        let message = match lines.next_blocking() {
            Ok(Some(frame)) => Message::Frame(frame),
            Ok(None) => Message::End,
            Err(error) => Message::Failed(error.to_string()),
        };
        let last = !matches!(message, Message::Frame(_));
        if last {
            gauges.output_ended.store(true, Ordering::SeqCst);
        }
        let now = gauges
            .pending
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        gauges.high_water.fetch_max(now, Ordering::SeqCst);
        if messages.send(message).is_err() || last {
            return;
        }
    }
}

fn count_errors(mut error: Polled<std::process::ChildStderr>, gauges: &Gauges) {
    // Counted and discarded: the provider's diagnostics are never retained.
    let mut buffer = [0_u8; 4096];
    while let Ok(count) = error.read(&mut buffer) {
        if count == 0 {
            return;
        }
        gauges
            .stderr_bytes
            .fetch_add(count as u64, Ordering::SeqCst);
    }
}

/// An owned provider child and its bounded output queue.
#[derive(Debug)]
pub struct Provider {
    child: Child,
    input: Option<ChildStdin>,
    written: usize,
    budget: usize,
    messages: Option<Receiver<Message>>,
    threads: Vec<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    gauges: Arc<Gauges>,
    killed: bool,
}

fn named<T: Send + 'static>(
    name: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<JoinHandle<T>, ProcessError> {
    thread::Builder::new()
        .name(format!("dpm-claude-{name}"))
        .spawn(work)
        .map_err(|source| ProcessError::Thread { name, source })
}

impl Provider {
    /// Start the provider with its output queue bounded to `queue` messages and each line to
    /// `line_limit` bytes.
    pub fn spawn_blocking(
        launch: &Launch,
        queue: usize,
        line_limit: usize,
    ) -> Result<Self, ProcessError> {
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .current_dir(&launch.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in &launch.remove_env {
            command.env_remove(name);
        }
        let mut child = command.spawn().map_err(|source| ProcessError::Spawn {
            program: launch.program.clone(),
            source,
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let gauges = Arc::new(Gauges::default());
        let (sender, messages) = mpsc::sync_channel(queue.max(1));
        let mut threads = Vec::new();
        let started = (|| {
            for end in [
                child.stdin.as_ref().map(nonblocking),
                child.stdout.as_ref().map(nonblocking),
                child.stderr.as_ref().map(nonblocking),
            ] {
                end.transpose()?;
            }
            if let Some(output) = child.stdout.take() {
                let (gauges, stop) = (gauges.clone(), stop.clone());
                threads.push(named("stdout", move || {
                    read_output(
                        Polled {
                            inner: output,
                            stop,
                        },
                        line_limit,
                        &sender,
                        &gauges,
                    )
                })?);
            }
            if let Some(error) = child.stderr.take() {
                let (gauges, stop) = (gauges.clone(), stop.clone());
                threads.push(named("stderr", move || {
                    count_errors(Polled { inner: error, stop }, &gauges)
                })?);
            }
            Ok::<(), ProcessError>(())
        })();
        let input = child.stdin.take();
        let mut provider = Self {
            child,
            input,
            written: 0,
            budget: INPUT_BUDGET,
            messages: Some(messages),
            threads,
            stop,
            gauges,
            killed: false,
        };
        // A provider whose pipe threads could not start is ended before the error is returned.
        started.inspect_err(|_| {
            provider.stop_blocking(Duration::from_millis(200));
        })?;
        Ok(provider)
    }

    /// Wait up to `wait` for the next message.
    pub fn next_blocking(&self, wait: Duration) -> Received {
        let Some(messages) = self.messages.as_ref() else {
            return Received::Message(Message::End);
        };
        match messages.recv_timeout(wait) {
            Ok(message) => {
                self.gauges.pending.fetch_sub(1, Ordering::SeqCst);
                Received::Message(message)
            }
            Err(RecvTimeoutError::Timeout) => Received::Timeout,
            Err(RecvTimeoutError::Disconnected) => Received::Message(Message::End),
        }
    }

    /// The next message if one is already waiting.
    pub fn try_next(&self) -> Option<Message> {
        match self.messages.as_ref()?.try_recv() {
            Ok(message) => {
                self.gauges.pending.fetch_sub(1, Ordering::SeqCst);
                Some(message)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// The provider process's identity, which a host records so a recovery can tell whether it
    /// still exists.
    #[must_use]
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    /// Whether the provider process has exited, without waiting. A descendant that holds the
    /// provider's pipes can keep its output from ever reaching end of input, so the exit of the
    /// process itself is the evidence that no more output will be produced by it.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// The most messages ever counted as waiting at once. The count includes the message the reader
    /// is handing over and the one the consumer is taking, so it never exceeds the queue's size by
    /// more than two; the memory held is the queue's bound and those two messages.
    #[must_use]
    pub fn pending_high_water(&self) -> usize {
        self.gauges.high_water.load(Ordering::SeqCst)
    }

    /// How many bytes the provider wrote to standard error, which are not kept.
    #[must_use]
    pub fn stderr_bytes(&self) -> u64 {
        self.gauges.stderr_bytes.load(Ordering::SeqCst)
    }

    /// Lower the input budget, for a caller or a test that wants a tighter resource limit.
    pub fn limit_input(&mut self, budget: usize) {
        self.budget = budget.min(INPUT_BUDGET);
    }

    /// Write one line to the provider's input within `within`, or until `abandon` says to stop.
    /// The pipe is nonblocking: the write waits for it to be writable in short slices, handles
    /// partial writes, and gives up with [`ProcessError::Timeout`] when the provider has stopped
    /// reading, however much a pipe holds. A line that would exceed the input budget is refused.
    pub fn send_line_blocking(
        &mut self,
        line: &str,
        within: Duration,
        abandon: &dyn Fn() -> bool,
    ) -> Result<(), ProcessError> {
        let total = self.written.saturating_add(line.len()).saturating_add(1);
        if total > self.budget {
            return Err(ProcessError::Budget {
                budget: self.budget,
            });
        }
        let Some(input) = self.input.as_mut() else {
            return Err(ProcessError::Write(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the provider's input is closed",
            )));
        };
        let mut bytes = Vec::with_capacity(line.len().saturating_add(1));
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let deadline = Instant::now() + within;
        let mut sent = 0_usize;
        while let Some(rest) = bytes.get(sent..).filter(|rest| !rest.is_empty()) {
            if abandon() {
                return Err(ProcessError::Abandoned);
            }
            if Instant::now() >= deadline {
                return Err(ProcessError::Timeout);
            }
            let mut watched = [PollFd::new(&*input, PollFlags::OUT)];
            match poll(&mut watched, Some(&WATCH)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => continue,
                Ok(_) => {}
                Err(error) => return Err(ProcessError::Write(error.into())),
            }
            match input.write(rest) {
                Ok(count) => sent = sent.saturating_add(count),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(ProcessError::Write(error)),
            }
        }
        self.written = total;
        Ok(())
    }

    /// Close the provider's input, which ends a well-behaved run.
    pub fn close_input(&mut self) {
        self.input = None;
    }

    fn wait_until(&mut self, deadline: Instant) -> Option<ExitStatus> {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) | Err(_) => return None,
            }
        }
    }

    /// End the provider within `grace` for each step: close its input, wait for it to exit, kill
    /// it, wait again. Then stop and join the pipe threads, which cannot outlive the stop. Safe to
    /// call more than once.
    pub fn stop_blocking(&mut self, grace: Duration) -> Stopped {
        self.close_input();
        let output_open = !self.gauges.output_ended.load(Ordering::SeqCst);
        let mut status = self.wait_until(Instant::now() + grace);
        if status.is_none() {
            self.killed = true;
            if let Err(error) = self.child.kill() {
                tracing::warn!(%error, "the provider could not be killed");
            }
            status = self.wait_until(Instant::now() + grace);
        }
        self.stop.store(true, Ordering::SeqCst);
        // Dropping the receiver releases a reader blocked on a full queue.
        self.messages = None;
        for thread in self.threads.drain(..) {
            if thread.join().is_err() {
                tracing::warn!("a provider pipe thread panicked");
            }
        }
        Stopped {
            exit: status
                .map(|status| Exit {
                    code: status.code(),
                    signal: status.signal(),
                })
                .unwrap_or_default(),
            killed: self.killed,
            reaped: status.is_some(),
            output_open,
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        // An abandoned provider is not left running, and its threads end with the stop flag.
        self.stop_blocking(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests;

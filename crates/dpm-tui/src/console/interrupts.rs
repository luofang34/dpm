//! Termination requests delivered as signals rather than keys. Raw mode turns the ^C key into an
//! ordinary key event, but `kill -INT` or `kill -TERM` would otherwise end the process before the
//! terminal guard restores the cooked mode, main screen and mouse reporting.

use signal_hook::consts::{SIGINT, SIGTERM};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Flag set by SIGINT or SIGTERM while the console runs; the event loop checks it each tick.
pub(super) struct Interrupts {
    received: Arc<AtomicBool>,
    handlers: Vec<signal_hook::SigId>,
}

impl Interrupts {
    /// Replace the default action of SIGINT and SIGTERM until dropped. A second signal while the
    /// first is still pending exits at once, so a stalled loop can always be ended.
    pub(super) fn register() -> io::Result<Self> {
        let received = Arc::new(AtomicBool::new(false));
        let mut handlers = Vec::new();
        for signal in [SIGINT, SIGTERM] {
            // Order matters: the shutdown check must run before this signal sets the flag.
            handlers.push(signal_hook::flag::register_conditional_shutdown(
                signal,
                1,
                Arc::clone(&received),
            )?);
            handlers.push(signal_hook::flag::register(signal, Arc::clone(&received))?);
        }
        Ok(Self { received, handlers })
    }

    /// Whether a termination signal arrived since registration.
    pub(super) fn received(&self) -> bool {
        self.received.load(Ordering::SeqCst)
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        for handler in self.handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;

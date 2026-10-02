//! Newline framing with a bound on every frame and no unbounded buffering.
//!
//! A frame is one UTF-8 line. The reader never holds more than the limit of one frame: a longer
//! line is discarded as it streams past and reported as a fault once its newline arrives, so a
//! client that sends endless bytes costs a counter and not memory, and the next frame after it is
//! still read in step. End of input in the middle of a frame is a fault of its own, never a short
//! frame that is quietly accepted.

use std::io::{self, BufRead};

/// What was wrong with a frame that could not be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFault {
    /// The line was longer than the limit.
    TooLarge {
        /// The limit it exceeded, in bytes.
        limit: usize,
    },
    /// The line is not valid UTF-8.
    NotUtf8,
}

/// One frame read from the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A whole line, without its newline.
    Complete(String),
    /// A whole line that cannot be used; the next read continues after it.
    Rejected(FrameFault),
}

/// What reading the next frame found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// A frame, accepted or rejected.
    Frame(Frame),
    /// The input ended between frames.
    End,
    /// The input ended inside a frame: its newline never arrived.
    Truncated,
}

/// Reads frames of at most `limit` bytes from a buffered input.
pub struct FrameReader<R> {
    reader: R,
    limit: usize,
    /// The most bytes this reader has held for one frame, so a test can state the bound.
    high_water: usize,
}

impl<R: BufRead> FrameReader<R> {
    /// Read frames of at most `limit` bytes.
    pub fn new(reader: R, limit: usize) -> Self {
        Self {
            reader,
            limit,
            high_water: 0,
        }
    }

    /// The most bytes held for any one frame so far.
    #[must_use]
    pub fn high_water(&self) -> usize {
        self.high_water
    }

    /// Read until the next newline, the end of input, or an I/O failure.
    pub fn next_blocking(&mut self) -> io::Result<Next> {
        let mut line: Vec<u8> = Vec::new();
        let mut oversized = false;
        let mut started = false;
        loop {
            let available = match self.reader.fill_buf() {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                return Ok(if started { Next::Truncated } else { Next::End });
            }
            started = true;
            let newline = available.iter().position(|byte| *byte == b'\n');
            let (chunk, _) = available.split_at(newline.unwrap_or(available.len()));
            let used = chunk.len() + usize::from(newline.is_some());
            if !oversized {
                if line.len() + chunk.len() > self.limit {
                    oversized = true;
                    line = Vec::new();
                } else {
                    line.extend_from_slice(chunk);
                    self.high_water = self.high_water.max(line.len());
                }
            }
            self.reader.consume(used);
            if newline.is_some() {
                return Ok(Next::Frame(if oversized {
                    Frame::Rejected(FrameFault::TooLarge { limit: self.limit })
                } else {
                    match String::from_utf8(line) {
                        Ok(text) => Frame::Complete(text),
                        Err(_) => Frame::Rejected(FrameFault::NotUtf8),
                    }
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests;

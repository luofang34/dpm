//! Bounded newline framing of the provider's standard output.
//!
//! A line longer than the bound is never buffered: it is counted while it streams past and reported
//! as oversized, so a provider that never ends a line cannot grow memory. A line that is not UTF-8
//! is reported, never repaired, and input that ends inside a line is a truncated frame, never a
//! short one.

use std::io::{self, BufRead, ErrorKind};

/// What reading the next line found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// One complete line of UTF-8 without its newline.
    Line(String),
    /// A line longer than the bound; its bytes were discarded as they streamed.
    Oversized {
        /// Length of the discarded line.
        bytes: usize,
    },
    /// A complete line that is not valid UTF-8.
    NotUtf8 {
        /// Length of the discarded line.
        bytes: usize,
    },
    /// Input ended inside a line.
    Truncated {
        /// Length of the partial line.
        bytes: usize,
    },
}

/// Reads bounded lines from a buffered source.
#[derive(Debug)]
pub struct LineReader<R> {
    inner: R,
    limit: usize,
}

impl<R: BufRead> LineReader<R> {
    /// A reader that keeps at most `limit` bytes of any line.
    pub fn new(inner: R, limit: usize) -> Self {
        Self { inner, limit }
    }

    /// The next frame, or `None` at a clean end of input between lines.
    pub fn next_blocking(&mut self) -> io::Result<Option<Frame>> {
        let mut line = Vec::new();
        let mut total = 0_usize;
        let mut oversized = false;
        loop {
            let available = match self.inner.fill_buf() {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                return Ok((total > 0).then_some(Frame::Truncated { bytes: total }));
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let content = newline.unwrap_or(available.len());
            if !oversized {
                if line.len().saturating_add(content) > self.limit {
                    oversized = true;
                    line.clear();
                } else if let Some(part) = available.get(..content) {
                    line.extend_from_slice(part);
                }
            }
            total = total.saturating_add(content);
            let consumed = content.saturating_add(usize::from(newline.is_some()));
            self.inner.consume(consumed);
            if newline.is_some() {
                return Ok(Some(finish(line, total, oversized)));
            }
        }
    }
}

fn finish(line: Vec<u8>, total: usize, oversized: bool) -> Frame {
    if oversized {
        return Frame::Oversized { bytes: total };
    }
    match String::from_utf8(line) {
        Ok(text) => Frame::Line(text),
        Err(_) => Frame::NotUtf8 { bytes: total },
    }
}

#[cfg(test)]
mod tests;

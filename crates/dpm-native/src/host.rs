//! The serving loop: the one owner of the application and the one writer of responses.
//!
//! The helper process exists to own one workspace, so a single loop on one thread reads a frame,
//! runs it on the [`Application`], writes the whole answer, and reads the next. Exchanges are
//! serialized by construction and no worker thread can be left behind on any exit. Requests that a
//! client sends before an answer arrives wait in the input pipe, which the operating system
//! bounds; the helper holds at most one frame of at most the request limit, so pending work is
//! bounded by the pipe and by backpressure on the client's write, and no frame is dropped.

use crate::{
    frame::{Frame, FrameFault, FrameReader, Next},
    limits::{ID_ALPHABET, Limits, MAX_ID_BYTES},
    rejection::{codes, rejection},
};
use dpm_app::{Application, NativeResponse, NativeResult};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use thiserror::Error;

/// How serving ended without a failure of the helper itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The input closed between frames. Everything received was answered first.
    Closed,
    /// The input closed inside a frame, which was refused and not processed.
    Truncated,
}

/// A failure of the helper's own I/O or threads.
#[derive(Debug, Error)]
pub enum HostError {
    /// Reading requests failed.
    #[error("reading requests failed: {0}")]
    Read(#[source] io::Error),
    /// Writing a response failed, usually because the client went away. The request it answered
    /// was processed; a command in it may have committed.
    #[error("writing a response failed: {0}")]
    Write(#[source] io::Error),
}

/// A writer that keeps the first `limit` bytes it is given and only counts the rest, so encoding
/// an answer never holds more than the limit and still learns how long the whole answer is.
struct Capped {
    bytes: Vec<u8>,
    limit: usize,
    total: usize,
}

impl Write for Capped {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.total = self.total.saturating_add(buffer.len());
        let room = self.limit.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(buffer.get(..room.min(buffer.len())).unwrap_or_default());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serve `app` until the input ends: read frames of at most `limits.max_request_bytes`, answer each
/// in order, and send no answer longer than `limits.max_response_bytes()`.
///
/// A request that has been read is always processed to the end and its answer written if it can
/// be, even when the client has already gone; the helper never rolls back a commit because nobody
/// is listening.
pub fn serve_blocking(
    mut app: Application,
    input: impl BufRead,
    mut output: impl Write,
    limits: Limits,
) -> Result<Ending, HostError> {
    let mut frames = FrameReader::new(input, limits.max_request_bytes());
    loop {
        match frames.next_blocking().map_err(HostError::Read)? {
            Next::Frame(Frame::Complete(text)) => {
                let line = answer_blocking(&mut app, &text, limits);
                send_blocking(&mut output, &line)?;
            }
            Next::Frame(Frame::Rejected(fault)) => {
                tracing::warn!(?fault, "refused a request frame");
                send_blocking(&mut output, &fault_line(fault, limits))?;
            }
            Next::End => return Ok(Ending::Closed),
            Next::Truncated => {
                tracing::warn!("input ended inside a request frame");
                let message = "input ended inside a request frame; it was not processed";
                let line = rejection(
                    "",
                    codes::TRUNCATED_FRAME,
                    message,
                    None,
                    limits.max_response_bytes(),
                );
                send_blocking(&mut output, line.as_bytes())?;
                return Ok(Ending::Truncated);
            }
        }
    }
}

/// Run one request on the application and return the bytes of the line to send, which are never
/// more than the limit: an answer that would be longer is replaced whole by a refusal that says so
/// and never cut. The typed answer is encoded straight into a capped writer, so no full-length
/// copy of an oversized answer is ever built.
fn answer_blocking(app: &mut Application, text: &str, limits: Limits) -> Vec<u8> {
    let limit = limits.max_response_bytes();
    // The identifier is echoed in every answer, so its form is checked before anything runs: a
    // request whose identifier could not be echoed within the bound is refused unprocessed.
    if let Some(reason) = invalid_id(text) {
        tracing::warn!(%reason, "refused a request with an unusable identifier");
        let message = format!("the request identifier {reason}, and the request was not processed");
        let details = json!({"max_id_bytes": MAX_ID_BYTES, "allowed": ID_ALPHABET});
        return rejection("", codes::INVALID_ID, &message, Some(details), limit).into_bytes();
    }
    let response = app.native_line_blocking(text);
    tracing::debug!(id = %response.id, bytes = text.len(), "request");
    let mut capped = Capped {
        bytes: Vec::new(),
        limit,
        total: 0,
    };
    if let Err(error) = serde_json::to_writer(&mut capped, &response) {
        tracing::error!(%error, id = %response.id, "an answer could not be encoded");
        let message =
            format!("the answer could not be encoded: {error}; the request itself was processed");
        let details =
            committed_identity(&response).map(|committed| json!({"committed": committed}));
        return rejection(
            &response.id,
            codes::ENCODING_FAILED,
            &message,
            details,
            limit,
        )
        .into_bytes();
    }
    if capped.total <= limit {
        return capped.bytes;
    }
    tracing::warn!(id = %response.id, size = capped.total, "answer exceeds the response limit");
    let committed = committed_identity(&response);
    let message = match &committed {
        Some(_) => format!(
            "the command was committed, but its {}-byte answer is over the {limit}-byte frame limit \
             and was not sent; the identity of the committed operation is in the details",
            capped.total
        ),
        None => format!(
            "the answer is {} bytes, over the {limit}-byte frame limit, and was not sent; \
             the request itself was processed",
            capped.total
        ),
    };
    let mut details = json!({"limit": limit, "size": capped.total});
    if let (Some(committed), Some(map)) = (committed, details.as_object_mut()) {
        map.insert("committed".into(), committed);
    }
    rejection(
        &response.id,
        codes::RESPONSE_TOO_LARGE,
        &message,
        Some(details),
        limit,
    )
    .into_bytes()
}

/// What a client needs to reconcile a command whose answer was not delivered: the operation that
/// was committed. Anything else about a refused answer is a read, which has nothing to reconcile.
fn committed_identity(response: &NativeResponse) -> Option<Value> {
    match &response.result {
        Some(NativeResult::Committed(committed)) => {
            let recorded = &committed.envelope.data;
            Some(json!({
                "operation_id": recorded.operation.id,
                "resulting_revision": recorded.operation.resulting_revision,
                "lineage_id": recorded.lineage_id,
            }))
        }
        _ => None,
    }
}

/// Why the request's correlation identifier cannot be accepted, if it cannot. An identifier is a
/// token of at most [`MAX_ID_BYTES`] characters from [`ID_ALPHABET`]: nothing in it needs a JSON
/// escape, so an echo is exactly as long as the identifier, and a refusal that must carry it always
/// fits the smallest bound.
fn invalid_id(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        id: Option<String>,
    }
    let id = serde_json::from_str::<Probe>(text).ok()?.id?;
    if id.len() > MAX_ID_BYTES {
        return Some(format!(
            "is {} bytes; at most {MAX_ID_BYTES} are accepted",
            id.len()
        ));
    }
    if id
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')))
    {
        return Some(format!("uses characters outside {ID_ALPHABET}"));
    }
    None
}

fn fault_line(fault: FrameFault, limits: Limits) -> Vec<u8> {
    let limit = limits.max_response_bytes();
    match fault {
        FrameFault::TooLarge { limit: bound } => rejection(
            "",
            codes::FRAME_TOO_LARGE,
            &format!("request frame is over {bound} bytes and was discarded unread"),
            Some(json!({"limit": bound})),
            limit,
        ),
        FrameFault::NotUtf8 => rejection(
            "",
            codes::INVALID_ENCODING,
            "request frame is not valid UTF-8 and was not processed",
            None,
            limit,
        ),
    }
    .into_bytes()
}

/// Write one whole frame: the bytes, the newline that ends it, and a flush, so a client never
/// sees a frame's start without its end unless the connection itself is cut.
fn send_blocking(output: &mut impl Write, line: &[u8]) -> Result<(), HostError> {
    output
        .write_all(line)
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .map_err(HostError::Write)
}

#[cfg(test)]
mod tests;

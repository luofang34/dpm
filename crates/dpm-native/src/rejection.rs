//! The typed refusals the transport itself makes, in the shape every native error has, and always
//! within the response bound, built with bounded work.
//!
//! The application already refuses what concerns the contract. These concern the frame: a request
//! that never became a request, or an answer too large to send whole. A refusal keeps, in this
//! order of importance: its correlation identifier, its stable code, and for a command that
//! committed, the identity of the committed operation. Diagnostic text and details are bounded
//! before the refusal is rendered, never after: text is cut to a bounded prefix (marked with an
//! ellipsis) and details that would not fit are dropped (and marked), so no work or memory scales
//! with how large the offending input or error text was.

use dpm_app::{API_VERSION, NATIVE_PROTOCOL_VERSION, NativeErrorBody, NativeResponse};
use serde_json::{Value, json};
use std::{fmt, io};

/// Stable codes of the transport's own refusals.
pub mod codes {
    /// A request line was longer than the helper accepts.
    pub const FRAME_TOO_LARGE: &str = "frame_too_large";
    /// A request line was not valid UTF-8.
    pub const INVALID_ENCODING: &str = "invalid_encoding";
    /// The input ended before a request line's newline.
    pub const TRUNCATED_FRAME: &str = "truncated_frame";
    /// An answer was longer than the helper sends, so it was not sent.
    pub const RESPONSE_TOO_LARGE: &str = "response_too_large";
    /// A request carried a correlation identifier outside the accepted form; it was not processed.
    pub const INVALID_ID: &str = "invalid_id";
    /// An answer could not be encoded at all. The request was processed.
    pub const ENCODING_FAILED: &str = "response_encoding_failed";
    /// The helper was started with arguments it cannot run.
    pub const INVALID_OPTIONS: &str = "invalid_options";
    /// The helper could not read its working directory to discover a project from.
    pub const WORKING_DIRECTORY: &str = "working_directory_unavailable";
}

/// A `fmt::Write` that keeps at most `limit` bytes and refuses the rest, so formatting a value
/// whose text may be enormous stops as soon as the bound is reached.
struct BoundedText {
    text: String,
    limit: usize,
    cut: bool,
}

impl fmt::Write for BoundedText {
    fn write_str(&mut self, piece: &str) -> fmt::Result {
        let room = self.limit.saturating_sub(self.text.len());
        if piece.len() <= room {
            self.text.push_str(piece);
            return Ok(());
        }
        // Keep a whole-character prefix of what still fits, then stop the formatting.
        let end = piece
            .char_indices()
            .map(|(index, character)| index + character.len_utf8())
            .take_while(|end| *end <= room)
            .last()
            .unwrap_or(0);
        self.text.push_str(piece.get(..end).unwrap_or_default());
        self.cut = true;
        Err(fmt::Error)
    }
}

/// The text of `value`, at most `limit` bytes of it, ending in an ellipsis when it was cut.
/// Formatting stops at the bound, so a value with a huge text costs only the bound.
pub(crate) fn bounded_display(value: &(impl fmt::Display + ?Sized), limit: usize) -> String {
    use fmt::Write as _;
    let mut bounded = BoundedText {
        text: String::new(),
        limit,
        cut: false,
    };
    // An error from the writer is the bound being reached, which `cut` records; a value that fails
    // to format by itself leaves whatever it produced.
    write!(bounded, "{value}").ok();
    if bounded.cut {
        bounded.text.push('…');
    }
    bounded.text
}

/// A writer that counts bytes and keeps none, to learn how large a value would be on the wire.
struct Counted(usize);

impl io::Write for Counted {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buffer.len());
        // Stop early once the answer is known to be over any bound worth keeping.
        if self.0 > 1 << 20 {
            return Err(io::Error::other("larger than any refusal keeps"));
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Whether `details` is small enough to keep; measuring it holds nothing and stops early.
fn small_enough(details: &Value, budget: usize) -> bool {
    let mut counted = Counted(0);
    serde_json::to_writer(&mut counted, details).is_ok() && counted.0 <= budget
}

fn render(id: &str, code: &str, message: &str, details: Option<Value>) -> String {
    let response = NativeResponse {
        protocol: NATIVE_PROTOCOL_VERSION,
        id: id.to_string(),
        ok: false,
        result: None,
        error: Some(NativeErrorBody {
            api_version: API_VERSION,
            code: code.to_string(),
            message: message.to_string(),
            details,
        }),
    };
    // A refusal is plain strings and numbers, which always encode.
    serde_json::to_string(&response).unwrap_or_default()
}

/// What survives when details must be dropped: the committed operation, if there is one and it is
/// a small identity.
fn surviving(details: Option<&Value>) -> Value {
    match details
        .and_then(|found| found.get("committed"))
        .filter(|committed| small_enough(committed, 512))
    {
        Some(committed) => json!({"committed": committed, "details_dropped": true}),
        None => json!({"details_dropped": true}),
    }
}

/// A refusal as one response line of at most `limit` bytes. `id` is empty when no request
/// identifier could be read or when the one given cannot be echoed.
///
/// The message is read only as far as the bound, so a long one costs the bound and not its length.
/// A validated bound is far larger than the identifier (an escape-free token of at most 128
/// bytes), the code and a committed operation's identity together, so those always fit.
pub(crate) fn rejection(
    id: &str,
    code: &str,
    message: &(impl fmt::Display + ?Sized),
    details: Option<Value>,
    limit: usize,
) -> String {
    // Work is bounded by the limit from here on, whatever the sizes of the inputs.
    let text = bounded_display(message, limit / 2);
    // Details that are too large to keep are replaced, not omitted: the committed identity stays.
    let keep = match details.as_ref() {
        Some(found) if small_enough(found, limit / 4) => Some(found.clone()),
        Some(found) => Some(surviving(Some(found))),
        None => None,
    };
    let line = render(id, code, &text, keep);
    if line.len() <= limit {
        return line;
    }
    let reduced = surviving(details.as_ref());
    let line = render(id, code, &text, Some(reduced.clone()));
    if line.len() <= limit {
        return line;
    }
    // Shorten the already bounded text until the refusal fits: its size never shrinks as the
    // prefix grows, so a binary search over the prefix length finds the longest that fits.
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    let prefix = |count: usize| {
        let end = boundaries.get(count).copied().unwrap_or(text.len());
        format!("{}…", text.get(..end).unwrap_or_default())
    };
    let (mut low, mut high) = (0, boundaries.len());
    while low < high {
        let middle = (low + high).div_ceil(2);
        if render(id, code, &prefix(middle), Some(reduced.clone())).len() <= limit {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    render(id, code, &prefix(low), Some(reduced))
}

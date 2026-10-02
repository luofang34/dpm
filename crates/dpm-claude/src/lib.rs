//! Adapter that connects the installed Claude Code runtime to DPM run records.
//!
//! It starts one provider process confined to a directory, speaks the runtime's documented
//! stream-json control protocol, and records only an explicit allowlist of the provider's public
//! events as managed run activity and lifecycle through the shared application. It refuses every
//! permission request, keeps no credentials, tool arguments or tool output, drops reasoning at
//! intake, and never submits or verifies the task a run belongs to. Provider types and process
//! I/O live here and nowhere in the domain crates.

mod command;
mod event;
mod frame;
mod host;
mod intake;
mod options;
mod process;
mod redact;
mod session;
mod sink;

pub use command::{CommandError, MAX_PROMPT_BYTES, Outcome, execute_blocking};
pub use event::{
    DETAIL_BYTES, Dropped, Finish, ParseError, Parsed, PublicEvent, Session, TEXT_BYTES, Verdict,
    parse_line, tool_detail,
};
pub use frame::{Frame, LineReader};
pub use host::{HostError, Hosted, ProviderRecord, directory as host_directory};
pub use intake::{
    Action, Closing, End, Evidence, Exit, Intake, MALFORMED_NOTES, SEEN_LIMIT, Step, Stop, Tally,
    Terminal,
};
pub use options::{Command, Options, OptionsError, Parsed as ParsedOptions, Prompt, USAGE};
pub use process::{INPUT_BUDGET, Launch, Message, ProcessError, Provider, Received, Stopped};
pub use redact::{REDACTED, bounded, identifier, public_text, redact};
pub use session::{
    BeginError, Begun, DriveFailure, HandshakeError, Report, Retry, Turn, drive_blocking,
};
pub use sink::{
    AppSink, NewRun, Prior, Receipt, RunSink, SinkError, Started, close_prior_blocking,
    prior_blocking, start_blocking, terminal_event,
};

mod refusals;

use super::*;
use crate::limits::{
    DEFAULT_MAX_REQUEST_BYTES, DEFAULT_MAX_RESPONSE_BYTES, MAX_ID_BYTES, MIN_FRAME_BYTES,
};
use dpm_app::QueryClock;
use dpm_model::Plan;
use serde_json::{Value, json};
use std::{
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex},
};

fn fixture() -> Plan {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture plan");
    plan.decisions.clear();
    plan
}

/// Output the test can read after the owner thread has finished with it.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn lines(output: &Shared) -> Vec<Value> {
    let bytes = output.0.lock().expect("output").clone();
    String::from_utf8(bytes)
        .expect("the helper writes UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("every frame is JSON"))
        .collect()
}

fn run(
    app: Application,
    input: Vec<u8>,
    limits: Limits,
) -> (Result<Ending, HostError>, Vec<Value>, usize) {
    let output = Shared::default();
    let ending = serve_blocking(app, Cursor::new(input), output.clone(), limits);
    let raw = output.0.lock().expect("output").len();
    (ending, lines(&output), raw)
}

fn memory() -> Application {
    Application::in_memory_blocking(&fixture()).expect("application")
}

/// An application whose query clock is pinned, so that two evaluations of one query encode to the
/// same size. A view carries `evaluated_at`, and a system-clock reading prints a varying number of
/// fractional digits depending on its trailing zeros, so the size of an answer is not a property
/// of the answer alone unless the clock is fixed. Each in-memory store mints its own lineage, so
/// the bytes of two answers still differ; only their encoded sizes are equal.
fn pinned(instant: &str) -> Application {
    memory().with_query_clock(QueryClock::Fixed(
        instant.parse().expect("an RFC 3339 instant"),
    ))
}

const NINE_DIGITS: &str = "2026-01-02T03:04:05.123456789Z";

fn request(id: &str, call: &Value) -> Vec<u8> {
    let mut line = json!({"protocol": 1, "id": id, "call": call})
        .to_string()
        .into_bytes();
    line.push(b'\n');
    line
}

fn attach(id: &str) -> Vec<u8> {
    request(id, &json!({"type": "attach"}))
}

fn claim(id: &str, app_plan: &Plan, base_revision: u64) -> Vec<u8> {
    let work = app_plan.find_work_by_key("TEST-A").expect("TEST-A").id;
    request(
        id,
        &json!({"type": "command", "request": {
            "actor": {"kind": "Agent", "name": "worker"},
            "base_revision": base_revision,
            "command": {"Claim": {"work": work}},
        }}),
    )
}

fn history_len(path: &Path) -> usize {
    Application::open_blocking(path)
        .expect("reopen")
        .history_blocking(0, 100)
        .expect("history")
        .entries
        .len()
}

#[test]
fn requests_are_answered_in_order_each_with_its_own_identifier() {
    let mut input = request("h", &json!({"type": "hello", "protocols": [1]}));
    input.extend(attach("a"));
    input.extend(request(
        "q",
        &json!({"type": "query", "query": {"query": "revision"}}),
    ));
    let (ending, answers, _) = run(memory(), input, Limits::default());
    assert_eq!(ending.expect("served"), Ending::Closed);
    let ids: Vec<_> = answers.iter().map(|a| a["id"].as_str()).collect();
    assert_eq!(ids, [Some("h"), Some("a"), Some("q")]);
    assert!(answers.iter().all(|a| a["ok"] == true), "{answers:?}");
    assert_eq!(answers[0]["result"]["kind"], "hello");
    assert_eq!(answers[2]["result"]["kind"], "view");
}

#[test]
fn an_oversized_request_is_refused_unread_and_the_next_one_is_still_correlated() {
    let limits = Limits::new(1024, DEFAULT_MAX_RESPONSE_BYTES).expect("bounds");
    let mut input = vec![b'x'; 200_000];
    input.push(b'\n');
    input.extend(attach("after"));
    let (ending, answers, _) = run(memory(), input, limits);
    assert_eq!(ending.expect("served"), Ending::Closed);
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["ok"], false);
    assert_eq!(answers[0]["error"]["code"], codes::FRAME_TOO_LARGE);
    assert_eq!(answers[0]["error"]["details"]["limit"], 1024);
    assert_eq!(
        (answers[1]["id"].as_str(), answers[1]["ok"].as_bool()),
        (Some("after"), Some(true))
    );
}

#[test]
fn invalid_utf8_and_a_truncated_last_frame_are_explicit_failures() {
    let mut input = b"\xff\xfe not text\n".to_vec();
    input.extend(attach("ok"));
    input.extend(b"{\"protocol\":1,\"id\":\"cut\",\"call\":{\"type\":\"att");
    let (ending, answers, _) = run(memory(), input, Limits::default());
    assert_eq!(ending.expect("served"), Ending::Truncated);
    let codes_seen: Vec<_> = answers
        .iter()
        .map(|a| a["error"]["code"].as_str().unwrap_or("ok"))
        .collect();
    assert_eq!(
        codes_seen,
        [codes::INVALID_ENCODING, "ok", codes::TRUNCATED_FRAME]
    );
}

#[test]
fn malformed_and_unsupported_requests_keep_the_contracts_own_refusals() {
    let mut input = b"not json\n".to_vec();
    input.extend(
        json!({"protocol": 9, "id": "future", "call": {"type": "attach"}})
            .to_string()
            .into_bytes(),
    );
    input.push(b'\n');
    let (_, answers, _) = run(memory(), input, Limits::default());
    assert_eq!(answers[0]["error"]["code"], "invalid_request");
    assert_eq!(answers[1]["error"]["code"], "unsupported_protocol");
    assert_eq!(answers[1]["id"], "future");
}

#[test]
fn an_answer_over_the_limit_is_replaced_whole_by_a_refusal_with_its_identifier() {
    let query = request(
        "big",
        &json!({"type": "query", "query": {"query": "export"}}),
    );
    let (_, whole, raw) = run(pinned(NINE_DIGITS), query.clone(), Limits::default());
    let size = raw - 1;
    assert_eq!(whole[0]["ok"], true, "the export itself succeeds");

    let limits = Limits::new(DEFAULT_MAX_REQUEST_BYTES, 2048).expect("bounds");
    assert!(
        size > 2048,
        "the fixture's export must exceed the test limit, it is {size}"
    );
    // The refusal reports the size of the answer it replaces, so the second evaluation must
    // encode to the same size as the first; the pinned clock is what makes that so.
    let (ending, answers, raw) = run(pinned(NINE_DIGITS), query, limits);
    assert_eq!(ending.expect("served"), Ending::Closed);
    assert_eq!(answers.len(), 1);
    let refusal = &answers[0];
    assert_eq!(
        (refusal["id"].as_str(), refusal["ok"].as_bool()),
        (Some("big"), Some(false))
    );
    assert_eq!(refusal["error"]["code"], codes::RESPONSE_TOO_LARGE);
    assert_eq!(refusal["error"]["details"]["limit"], 2048);
    assert_eq!(refusal["error"]["details"]["size"], size);
    assert!(refusal["error"]["details"].get("committed").is_none());
    assert!(
        refusal.get("result").is_none(),
        "no part of the answer is sent"
    );
    assert!(
        raw < size,
        "the refusal is far smaller than the answer it replaces"
    );
}

#[test]
fn the_size_of_an_answer_follows_the_clock_text_so_size_comparisons_pin_the_clock() {
    let query = request(
        "same",
        &json!({"type": "query", "query": {"query": "export"}}),
    );
    let (_, nine, nine_raw) = run(pinned(NINE_DIGITS), query.clone(), Limits::default());
    let (_, again, again_raw) = run(pinned(NINE_DIGITS), query.clone(), Limits::default());
    // Lineages differ between stores but print at the same width, so the sizes are equal.
    assert_eq!(
        nine_raw, again_raw,
        "one pinned clock gives one encoded size"
    );
    assert_eq!(nine[0]["result"]["evaluated_at"], NINE_DIGITS);
    assert_eq!(again[0]["result"]["evaluated_at"], NINE_DIGITS);

    // The same export read at instants whose printed precision differs is shorter by exactly the
    // digits (and the point) that precision drops, and by nothing else: the clock appears once.
    // A system clock prints whichever precision its trailing zeros allow, so an answer's size
    // is comparable between evaluations only when the clock is pinned.
    for (printed, shorter_by) in [
        ("2026-01-02T03:04:05.123456000Z", 3),
        ("2026-01-02T03:04:05.123000000Z", 6),
        ("2026-01-02T03:04:05.000000000Z", 10),
    ] {
        let (_, answer, raw) = run(pinned(printed), query.clone(), Limits::default());
        let shown = answer[0]["result"]["evaluated_at"].as_str().expect("text");
        assert_eq!(
            shown.len() + shorter_by,
            NINE_DIGITS.len(),
            "{printed} prints as {shown}"
        );
        assert_eq!(nine_raw - raw, shorter_by, "{printed}");
    }
}

#[test]
fn a_committed_command_whose_answer_is_too_large_names_what_was_committed() {
    let directory = tempfile::tempdir().expect("directory");
    let store = directory.path().join("state.sqlite");
    let plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("TEST-A").id;
    let app = Application::initialize_blocking(&store, &plan).expect("store");
    let step = |id: &str, base: u64, command: Value| {
        request(
            id,
            &json!({"type": "command", "request": {
                "actor": {"kind": "Agent", "name": "worker"},
                "base_revision": base,
                "command": command,
            }}),
        )
    };
    let mut input = step("claim", 0, json!({"Claim": {"work": work}}));
    input.extend(step("start", 1, json!({"Start": {"work": work}})));
    // A note whose own echo in the recorded operation makes the answer larger than the bound.
    let note = "n".repeat(6000);
    input.extend(step(
        "progress",
        2,
        json!({"ReportProgress": {"work": work, "percent": 10, "note": note}}),
    ));
    let limits = Limits::new(DEFAULT_MAX_REQUEST_BYTES, 4096).expect("bounds");
    let (ending, answers, _) = run(app, input, limits);
    assert_eq!(ending.expect("served"), Ending::Closed);
    assert_eq!(
        (answers[0]["ok"].as_bool(), answers[1]["ok"].as_bool()),
        (Some(true), Some(true))
    );
    let refusal = &answers[2];
    assert_eq!(refusal["id"], "progress");
    assert_eq!(refusal["error"]["code"], codes::RESPONSE_TOO_LARGE);
    assert!(
        refusal["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("was committed")),
        "{refusal}"
    );
    let committed = &refusal["error"]["details"]["committed"];
    assert_eq!(committed["resulting_revision"], 3);
    // The operation it names is the one in the store: the command was not rejected or rolled back.
    let reopened = Application::open_blocking(&store).expect("reopen");
    let entries = reopened.history_blocking(0, 10).expect("history").entries;
    assert_eq!(entries.len(), 3);
    assert_eq!(
        committed["operation_id"],
        json!(entries[2].operation.operation.id)
    );
}

#[test]
fn a_client_that_disappears_does_not_undo_a_command_it_sent() {
    struct Gone;
    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the client went away",
            ))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let directory = tempfile::tempdir().expect("directory");
    let store = directory.path().join("state.sqlite");
    let plan = fixture();
    let app = Application::initialize_blocking(&store, &plan).expect("store");
    let ending = serve_blocking(
        app,
        Cursor::new(claim("c", &plan, 0)),
        Gone,
        Limits::default(),
    );
    assert!(matches!(ending, Err(HostError::Write(_))), "{ending:?}");
    assert_eq!(
        history_len(&store),
        1,
        "the command was processed and stays committed"
    );
}

#[test]
fn many_pipelined_requests_all_get_an_answer_in_order_through_the_bounded_queue() {
    let mut input = Vec::new();
    for index in 0..500 {
        input.extend(attach(&format!("n{index}")));
    }
    let (ending, answers, _) = run(memory(), input, Limits::default());
    assert_eq!(ending.expect("served"), Ending::Closed);
    assert_eq!(answers.len(), 500);
    for (index, answer) in answers.iter().enumerate() {
        assert_eq!(answer["id"], format!("n{index}"));
    }
}

#[test]
fn a_read_failure_stops_serving_with_a_typed_error() {
    struct OneThenFail(Cursor<Vec<u8>>);
    impl io::Read for OneThenFail {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            match self.0.read(buffer)? {
                0 => Err(io::Error::new(io::ErrorKind::ConnectionReset, "reset")),
                count => Ok(count),
            }
        }
    }
    let output = Shared::default();
    let input = io::BufReader::new(OneThenFail(Cursor::new(attach("a"))));
    let ending = serve_blocking(memory(), input, output.clone(), Limits::default());
    assert!(matches!(ending, Err(HostError::Read(_))), "{ending:?}");
    assert_eq!(
        lines(&output).len(),
        1,
        "what arrived before the failure was answered"
    );
}

#[test]
fn a_command_the_application_refuses_keeps_its_shared_code_and_commits_nothing() {
    let directory = tempfile::tempdir().expect("directory");
    let store = directory.path().join("state.sqlite");
    let plan = fixture();
    let app = Application::initialize_blocking(&store, &plan).expect("store");
    let (_, answers, _) = run(app, claim("stale", &plan, 7), Limits::default());
    assert_eq!(answers[0]["error"]["code"], "revision_conflict");
    assert_eq!(history_len(&store), 0);
}

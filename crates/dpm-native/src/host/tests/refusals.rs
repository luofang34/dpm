//! Refusals under the smallest bounds: correlation, committed identity and bounded rendering.

use super::*;

#[test]
fn an_identifier_that_cannot_be_echoed_is_refused_before_anything_runs() {
    let limits = Limits::new(DEFAULT_MAX_REQUEST_BYTES, MIN_FRAME_BYTES).expect("bounds");
    let directory = tempfile::tempdir().expect("directory");
    let store = directory.path().join("state.sqlite");
    let plan = fixture();
    let app = Application::initialize_blocking(&store, &plan).expect("store");
    // Too long, and short but made of characters that JSON escapes to six bytes each, which would
    // push a refusal past the smallest bound if it were echoed: each carries a mutation.
    let escaped = "\u{0}".repeat(MAX_ID_BYTES);
    let mut input = claim(&"x".repeat(10_000), &plan, 0);
    input.extend(claim(&escaped, &plan, 0));
    input.extend(claim("has space", &plan, 0));
    let (_, answers, _) = run(app, input, limits);
    assert_eq!(answers.len(), 3);
    for refused in &answers {
        assert_eq!(
            refused["id"], "",
            "an identifier that cannot be echoed is not echoed"
        );
        assert_eq!(refused["error"]["code"], codes::INVALID_ID);
        assert!(refused.to_string().len() <= MIN_FRAME_BYTES);
    }
    assert_eq!(history_len(&store), 0, "none of the three mutations ran");
}

#[test]
fn at_the_smallest_bound_a_committed_command_keeps_its_correlation_and_its_identity() {
    let limits = Limits::new(DEFAULT_MAX_REQUEST_BYTES, MIN_FRAME_BYTES).expect("bounds");
    let directory = tempfile::tempdir().expect("directory");
    let store = directory.path().join("state.sqlite");
    let plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("TEST-A").id;
    let app = Application::initialize_blocking(&store, &plan).expect("store");
    // The longest identifier allowed, and a command whose answer cannot fit the smallest bound.
    let longest = "A".repeat(MAX_ID_BYTES);
    let note = "n".repeat(2000);
    let mut input = request(
        &longest,
        &json!({"type": "command", "request": {
            "actor": {"kind": "Agent", "name": "worker"},
            "base_revision": 0,
            "command": {"Claim": {"work": work}},
        }}),
    );
    input.extend(request(
        "start",
        &json!({"type": "command", "request": {
            "actor": {"kind": "Agent", "name": "worker"},
            "base_revision": 1, "command": {"Start": {"work": work}},
        }}),
    ));
    input.extend(request(
        &longest,
        &json!({"type": "command", "request": {
            "actor": {"kind": "Agent", "name": "worker"},
            "base_revision": 2,
            "command": {"ReportProgress": {"work": work, "percent": 5, "note": note}},
        }}),
    ));
    let (_, answers, _) = run(app, input, limits);
    for answer in &answers {
        assert!(answer.to_string().len() <= MIN_FRAME_BYTES, "{answer}");
    }
    let refused = &answers[2];
    assert_eq!(refused["id"], longest.as_str(), "correlation survives");
    assert_eq!(refused["error"]["code"], codes::RESPONSE_TOO_LARGE);
    let committed = &refused["error"]["details"]["committed"];
    assert_eq!(
        committed["resulting_revision"], 3,
        "the committed identity survives"
    );
    assert_eq!(history_len(&store), 3, "the mutation really committed");
    let recorded = Application::open_blocking(&store)
        .expect("reopen")
        .history_blocking(0, 10)
        .expect("history")
        .entries;
    assert_eq!(
        committed["operation_id"],
        json!(recorded[2].operation.operation.id)
    );
}

#[test]
fn a_refusal_that_must_shed_text_keeps_its_code_correlation_and_committed_identity() {
    use crate::rejection::rejection;
    let committed = json!({"committed": {"operation_id": "o", "resulting_revision": 3}, "limit": 1024, "size": 9});
    let line = rejection(
        &"I".repeat(MAX_ID_BYTES),
        codes::RESPONSE_TOO_LARGE,
        &"long text — ".repeat(500),
        Some(committed),
        MIN_FRAME_BYTES,
    );
    assert!(line.len() <= MIN_FRAME_BYTES, "{}", line.len());
    let frame: Value = serde_json::from_str(&line).expect("still JSON");
    assert_eq!(frame["id"], "I".repeat(MAX_ID_BYTES));
    assert_eq!(frame["error"]["code"], codes::RESPONSE_TOO_LARGE);
    assert_eq!(
        frame["error"]["details"]["committed"]["resulting_revision"],
        3
    );
    assert!(
        frame["error"]["message"]
            .as_str()
            .is_some_and(|m| m.ends_with('…'))
    );
}

#[test]
fn details_that_cannot_fit_are_dropped_but_the_committed_identity_survives() {
    use crate::rejection::rejection;
    let big = json!({
        "committed": {"operation_id": "o", "resulting_revision": 3},
        "noise": "z".repeat(50_000),
    });
    let line = rejection(
        "id-1",
        codes::RESPONSE_TOO_LARGE,
        "short",
        Some(big),
        MIN_FRAME_BYTES,
    );
    assert!(line.len() <= MIN_FRAME_BYTES);
    let frame: Value = serde_json::from_str(&line).expect("JSON");
    assert_eq!(frame["error"]["details"]["details_dropped"], true);
    assert_eq!(
        frame["error"]["details"]["committed"]["resulting_revision"],
        3
    );
    assert!(frame["error"]["details"].get("noise").is_none());
}

#[test]
fn formatting_a_huge_message_stops_at_the_bound_instead_of_producing_all_of_it() {
    use crate::rejection::{bounded_display, rejection};
    use std::{
        fmt,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static PIECES: AtomicUsize = AtomicUsize::new(0);
    struct Endless;
    impl fmt::Display for Endless {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            for _ in 0..50_000_000 {
                PIECES.fetch_add(1, Ordering::Relaxed);
                formatter.write_str("0123456789abcdef")?;
            }
            Ok(())
        }
    }
    let text = bounded_display(&Endless, 512);
    assert!(text.len() <= 512 + '…'.len_utf8(), "{}", text.len());
    assert!(text.ends_with('…'));
    assert!(
        PIECES.load(Ordering::Relaxed) <= 512 / 16 + 2,
        "formatting did not stop at the bound"
    );
    // The same through a refusal: the work is the bound, not the length of the text.
    PIECES.store(0, Ordering::Relaxed);
    let line = rejection(
        "",
        codes::RESPONSE_TOO_LARGE,
        &Endless,
        None,
        MIN_FRAME_BYTES,
    );
    assert!(line.len() <= MIN_FRAME_BYTES);
    assert!(PIECES.load(Ordering::Relaxed) <= MIN_FRAME_BYTES / 2 / 16 + 2);
}

#[test]
fn bounds_that_could_not_hold_a_refusal_cannot_be_constructed() {
    assert!(Limits::new(MIN_FRAME_BYTES - 1, DEFAULT_MAX_RESPONSE_BYTES).is_err());
    assert!(Limits::new(DEFAULT_MAX_REQUEST_BYTES, 0).is_err());
    assert!(Limits::new(DEFAULT_MAX_REQUEST_BYTES, usize::MAX).is_err());
    assert!(Limits::new(MIN_FRAME_BYTES, MIN_FRAME_BYTES).is_ok());
}

//! The macOS views and the shared data each one reads.
//!
//! This mapping is the contract between a native client and the application. It names no new
//! query: every entry is a shared [`Query`](crate::Query) or a run feed, so the views cannot drift
//! into holding their own rules. `Hello` returns it, so a client and a host agree on it.

use super::protocol::{Capabilities, Limits, RunCapabilities, UnsupportedControl};
use serde::{Deserialize, Serialize};

/// One macOS view and what feeds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewMapping {
    /// Stable view name: `now`, `live`, `review` or `detail`.
    pub view: String,
    /// What the view answers.
    pub purpose: String,
    /// The shared queries it reads, by `query` name.
    pub queries: Vec<String>,
    /// The cursors it follows through `changes`: `project`, `lifecycle`, `activity` and `links`.
    pub feeds: Vec<String>,
    /// What the view must keep apart.
    pub note: String,
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

/// The four views and their sources.
///
/// Every view names the identity of what it shows: the workspace, lineage and revision of the
/// project data, the `evaluated_at` of its time-dependent values, and the cursor of each feed it
/// follows. Project mutations (the `project` feed, operations in the semantic log) and run
/// lifecycle and activity (the run feeds) are different facts and are never merged into one stream.
#[must_use]
pub fn view_mappings() -> Vec<ViewMapping> {
    let view =
        |view: &str, purpose: &str, queries: &[&str], feeds: &[&str], note: &str| ViewMapping {
            view: view.into(),
            purpose: purpose.into(),
            queries: words(queries),
            feeds: words(feeds),
            note: note.into(),
        };
    vec![
        view(
            "now",
            "What to do now and why: ranked ready work, counts, and runs that are executing.",
            &["next", "status", "runs", "explain", "export"],
            &["project", "lifecycle", "activity", "links"],
            "Readiness comes from next and explain; the client never derives it. A working run is an executor's report and does not make work ready, submitted or done.",
        ),
        view(
            "live",
            "What is happening: unfinished runs, their lifecycle and bounded activity.",
            &[
                "runs",
                "run",
                "run_lifecycle",
                "run_activity",
                "history",
                "export",
            ],
            &["lifecycle", "activity", "links", "project"],
            "Run lifecycle is durable; activity is bounded telemetry whose retention gap is shown, never hidden. A link to a project operation is a third kind of fact: it changes a run's operations with no transition and no activity, so it never makes a run look newer. The links signal says to read the runs shown again; it does not say which. A stale or unknown run is not finished, and a reported_only run's silence proves nothing.",
        ),
        view(
            "review",
            "What awaits independent acceptance, with the evidence attached.",
            &["status", "explain", "show", "history", "export", "runs"],
            &["project", "lifecycle", "activity", "links"],
            "Only a project operation by an independent verifier accepts work. A completed run leaves submitted work awaiting that verifier. Status counts the work awaiting verification and names tasks only by identity, so the snapshot (export) is what names each of them, with its owner, acceptance and evidence; the runs of one task are read by the per-task runs query.",
        ),
        view(
            "detail",
            "One task or run in full: contract, gates, dependencies, evidence, runs and linked operations.",
            &[
                "explain",
                "show",
                "runs",
                "run",
                "run_lifecycle",
                "run_activity",
                "history",
                "export",
            ],
            &["project", "lifecycle", "activity", "links"],
            "Operations linked to a run are attributions recorded after the fact; the operation log itself does not know runs.",
        ),
    ]
}

/// What this build offers.
#[must_use]
pub(super) fn capabilities(commands: bool) -> Capabilities {
    let control = |control: &str, reason: &str| UnsupportedControl {
        control: control.into(),
        reason: reason.into(),
    };
    Capabilities {
        views: view_mappings(),
        runs: RunCapabilities {
            supported: cfg!(feature = "sqlite"),
            observation: words(&["managed", "reported_only"]),
            reported_only: "The executor reports itself through the CLI or tools. Absent reports prove nothing: an unfinished run goes stale, and is never taken for idle or finished.".into(),
            completion: "A completed run is the executor's report. Work stays as it was until its owner submits and an independent verifier accepts it.".into(),
        },
        commands,
        unsupported: vec![
            control("steer_run", "run steering belongs to RUN-30 and its decision"),
            control("stop_run", "run control belongs to RUN-30 and its decision"),
            control("answer_input_request", "run control belongs to RUN-30 and its decision"),
            control("provider_session_control", "provider adapters belong to RUN-20"),
            control("push_events", "this boundary is polled from cursors; a push channel is a host choice"),
            control("remote_access", "hosted sync, accounts and remote hosts are outside the local boundary"),
        ],
        limits: Limits {
            max_page: 1000,
            default_page: 100,
            stale_after_seconds: u32::try_from(dpm_engine::STALE_AFTER.num_seconds())
                .unwrap_or(u32::MAX),
            reevaluate_within_seconds: 60,
        },
    }
}

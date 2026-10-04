//! The view mapping names every shared read and feed the native observer's views actually use.

use crate::{Query, view_mappings};

/// The wire name of a shared query. The match is exhaustive, so adding a query forces a decision
/// here about whether a view may name it.
fn query_name(query: &Query) -> &'static str {
    match query {
        Query::ProposeChange { .. } => "propose_change",
        Query::Revision => "revision",
        Query::History { .. } => "history",
        Query::Export => "export",
        Query::PlanSchema => "plan_schema",
        Query::PlanTemplate => "plan_template",
        Query::ImportMspdi { .. } => "import_mspdi",
        Query::ExportMspdi { .. } => "export_mspdi",
        Query::Status { .. } => "status",
        Query::Schedule { .. } => "schedule",
        Query::Calibration => "calibration",
        Query::Next { .. } => "next",
        Query::Runs { .. } => "runs",
        Query::Run { .. } => "run",
        Query::RunLifecycle { .. } => "run_lifecycle",
        Query::RunActivity { .. } => "run_activity",
        Query::Show { .. } => "show",
        Query::Explain { .. } => "explain",
    }
}

/// Every query a view names is a real shared query: the mapping cannot drift into inventing one.
#[test]
fn every_view_names_only_shared_queries_and_known_feeds() {
    let known: Vec<&str> = [
        Query::Revision,
        Query::Export,
        Query::History {
            after_sequence: 0,
            limit: 1,
        },
        Query::Status {
            probabilistic: false,
            calibrated: false,
        },
        Query::Schedule {
            probabilistic: false,
        },
        Query::Next {
            capabilities: Default::default(),
            probabilistic: false,
            limit: 1,
            project_keys: Default::default(),
            asset_keys: Default::default(),
            actor: None,
        },
        Query::Runs {
            key: None,
            limit: 1,
        },
        Query::Run {
            id: dpm_model::RunId::new(),
        },
        Query::RunLifecycle {
            after_sequence: 0,
            limit: 1,
            run: None,
        },
        Query::RunActivity {
            after_sequence: 0,
            limit: 1,
            run: None,
        },
        Query::Show { key: String::new() },
        Query::Explain { key: String::new() },
    ]
    .iter()
    .map(query_name)
    .collect();
    for mapping in view_mappings() {
        assert!(
            !mapping.queries.is_empty() && !mapping.note.is_empty(),
            "{}",
            mapping.view
        );
        for query in &mapping.queries {
            assert!(
                known.contains(&query.as_str()),
                "{} names {query}",
                mapping.view
            );
        }
        for feed in &mapping.feeds {
            assert!(["project", "lifecycle", "activity", "links"].contains(&feed.as_str()));
        }
    }
    // Project mutations and run activity are separate feeds in every view that shows both.
    let live = view_mappings()
        .into_iter()
        .find(|v| v.view == "live")
        .expect("live");
    let has = |feed: &str| live.feeds.iter().any(|f| f == feed);
    assert!(has("lifecycle") && has("activity"));
}

fn mapping(view: &str) -> crate::ViewMapping {
    view_mappings()
        .into_iter()
        .find(|mapping| mapping.view == view)
        .expect("a view")
}

/// A view that shows a run's receipt, status or activity counts is refreshed by the activity feed,
/// and a view that reads a selected run's window reads its lifecycle and activity queries.
#[test]
fn views_name_the_feeds_and_queries_the_observer_reads_through_them() {
    let has = |view: &str, queries: &[&str], feeds: &[&str]| {
        let found = mapping(view);
        queries
            .iter()
            .all(|query| found.queries.iter().any(|name| name == query))
            && feeds
                .iter()
                .all(|feed| found.feeds.iter().any(|name| name == feed))
    };
    assert!(has("now", &["runs", "explain", "export"], &["activity"]));
    assert!(has("review", &["runs", "export"], &["activity"]));
    assert!(has(
        "live",
        &["run", "run_lifecycle", "run_activity", "export"],
        &["activity", "lifecycle"]
    ));
    assert!(has(
        "detail",
        &["run", "run_lifecycle", "run_activity", "export"],
        &["activity", "lifecycle", "links"]
    ));
    // The Gantt draws the shared schedule projection and reads relations from the snapshot; it owns
    // no scheduling, and the catalog lists every view once, in the order the interface shows them,
    // with the Gantt last.
    assert!(has(
        "gantt",
        &["schedule", "explain", "export"],
        &["project"]
    ));
    let order: Vec<String> = view_mappings().into_iter().map(|m| m.view).collect();
    assert_eq!(order, ["now", "live", "review", "detail", "gantt"]);
}

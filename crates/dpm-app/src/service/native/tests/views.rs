//! The view mapping names every shared read and feed the native observer's views actually use.

use crate::view_mappings;

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
}

use crate::LinkOutcome;
use crate::mspdi::tests::{import, link, outline_document, work, workspace};
use dpm_model::DependencyKind;

#[test]
fn duplicate_links_report_the_merged_lag() {
    let plan = workspace();
    // Spec -> Build FS twice: 0 h and 1 h elapsed; one dependency carries the larger lag.
    let result = import(
        &plan,
        &outline_document(&[link(2, 1, 0, 7), link(2, 1, 600, 6)].concat()),
    );
    let spec = work(&result.candidate, "MSP-2").id;
    let build = work(&result.candidate, "MSP-5").id;
    let merged: Vec<_> = result
        .candidate
        .dependencies
        .iter()
        .filter(|d| d.predecessor == spec && d.successor == build)
        .collect();
    assert_eq!(merged.len(), 1);
    assert_eq!(
        (merged[0].kind, merged[0].lag_hours),
        (DependencyKind::FinishStart, 1.0)
    );
    let (smaller, larger) = (&result.report.links[2], &result.report.links[3]);
    assert_eq!(smaller.dependencies, [merged[0].id]);
    assert_eq!(larger.dependencies, [merged[0].id]);
    assert_eq!(smaller.outcome, LinkOutcome::Approximated);
    assert!(
        smaller
            .notes
            .iter()
            .any(|n| n.contains("FS MSP-2 -> MSP-5") && n.contains("1 h")),
        "{smaller:?}"
    );
    assert_eq!(larger.outcome, LinkOutcome::Preserved);
    assert!(
        larger.notes.iter().any(|n| n.contains("another link")),
        "{larger:?}"
    );
}

#[test]
fn changed_local_lags_and_dropped_edges_are_reported() {
    let current = import(&workspace(), &outline_document(&link(2, 1, 0, 7))).candidate;
    // Review -> Approved lag 2 h becomes 1 h; Spec -> Build disappears from the source.
    let edited = outline_document("").replace(&link(3, 1, 1200, 6), &link(3, 1, 600, 6));
    let result = import(&current, &edited);
    let review = &result.report.links[1];
    assert_eq!(review.outcome, LinkOutcome::Preserved);
    assert!(
        review
            .notes
            .iter()
            .any(|n| n.contains("changes the local lag 2 h to 1 h")),
        "{review:?}"
    );
    let removed = &result.report.removed_dependencies;
    assert_eq!(removed.len(), 1);
    assert_eq!(
        (
            removed[0].predecessor.0.as_str(),
            removed[0].successor.0.as_str()
        ),
        ("MSP-2", "MSP-5")
    );
    assert!(
        !result
            .candidate
            .dependencies
            .iter()
            .any(|d| d.id == removed[0].id)
    );
    let unchanged = import(&current, &outline_document(&link(2, 1, 0, 7)));
    assert!(unchanged.report.removed_dependencies.is_empty());
    assert!(unchanged.report.links.iter().all(|l| l.notes.is_empty()));
}

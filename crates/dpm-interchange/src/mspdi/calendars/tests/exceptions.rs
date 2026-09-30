//! How Microsoft Project exceptions and their recurrences map to calendar exceptions.

use super::*;

#[test]
fn recurring_and_overlapping_exceptions_are_reported_not_guessed() {
    let exceptions = format!(
        "<Exceptions>{}{}{}</Exceptions>",
        recurrence(
            "Every Monday",
            "2026-01-05T00:00:00",
            "2026-03-30T23:59:00",
            12,
            6,
            OFF
        ),
        exception(
            "Audit",
            "2026-05-04T00:00:00",
            "2026-05-06T23:59:00",
            1,
            OFF
        ),
        exception(
            "Overlap",
            "2026-05-06T00:00:00",
            "2026-05-07T23:59:00",
            1,
            OFF
        )
    );
    let xml = document(
        &[calendar(1, "Office", None, &standard_week(), &exceptions)],
        &[],
    );
    let result = import(&workspace(), &xml);
    let office = definition(&result, "Office");
    assert_eq!(office.exceptions.len(), 1);
    assert_eq!(office.exceptions[0].name.as_deref(), Some("Audit"));
    let rejected: Vec<_> = result.report.calendars[0]
        .rejected
        .iter()
        .map(|f| f.detail.as_str())
        .collect();
    assert_eq!(rejected.len(), 2, "{rejected:?}");
    assert!(rejected[0].contains("recurring exception \"Every Monday\""));
    assert!(rejected[1].contains("overlaps exception \"Audit\""));
}

#[test]
fn a_multi_day_exception_written_as_a_daily_recurrence_is_a_date_range() {
    let vacation = exception(
        "Vacation",
        "2026-08-03T00:00:00",
        "2026-08-07T23:59:00",
        5,
        OFF,
    );
    let days = calendar(
        1,
        "Office",
        None,
        &standard_week(),
        &format!("<Exceptions>{vacation}</Exceptions>"),
    );
    let result = import(&workspace(), &document(&[days], &[]));
    let office = definition(&result, "Office");
    let dates: Vec<_> = office
        .exceptions
        .iter()
        .map(|e| (e.from.to_string(), e.last().to_string()))
        .collect();
    assert_eq!(dates, [("2026-08-03".to_owned(), "2026-08-07".to_owned())]);
    assert!(result.report.calendars[0].rejected.is_empty());
}

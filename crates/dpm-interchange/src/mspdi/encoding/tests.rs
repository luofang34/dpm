use super::*;

#[test]
fn format_codes_split_working_elapsed_and_unsupported_units() {
    for working in [3, 5, 7, 9, 11, 35, 37, 39, 41, 43] {
        assert_eq!(time_basis(working), Some(TimeBasis::Working), "{working}");
    }
    for elapsed in [4, 6, 8, 10, 12, 36, 38, 40, 42, 44] {
        assert_eq!(time_basis(elapsed), Some(TimeBasis::Elapsed), "{elapsed}");
    }
    for unsupported in [0, 1, 2, 19, 20, 21, 51, 52, 53] {
        assert_eq!(time_basis(unsupported), None, "{unsupported}");
    }
}

#[test]
fn durations_parse_and_format_whole_seconds() {
    assert_eq!(parse_duration("PT16H0M0S"), Ok(57_600));
    assert_eq!(parse_duration("PT1.5H"), Ok(5_400));
    assert_eq!(parse_duration("PT0H30M0.4S"), Ok(1_800));
    assert_eq!(parse_duration(" PT0H0M0S "), Ok(0));
    for seconds in [0, 1, 59, 3_600, 33_600, 123_456_789] {
        assert_eq!(parse_duration(&format_duration(seconds)), Ok(seconds));
    }
    assert_eq!(format_duration(33_600), "PT9H20M0S");
}

#[test]
fn calendar_dependent_or_malformed_durations_are_rejected() {
    for text in [
        "P1D", "P1DT2H", "PT", "PT5M3H", "PT1H1H", "-PT1H", "PT1.2.3H", "PTH", "8h",
    ] {
        assert!(parse_duration(text).is_err(), "{text}");
    }
}

#[test]
fn exported_duration_is_the_rounded_pert_expectation() {
    assert_eq!(duration_seconds(None), 0);
    let estimate = ThreePointEstimate {
        optimistic_hours: 4.0,
        likely_hours: 8.0,
        pessimistic_hours: 20.0,
    };
    assert_eq!(duration_seconds(Some(estimate)), 33_600);
    assert_eq!(estimate_from_seconds(0), None);
    let single = estimate_from_seconds(5_400).expect("estimate");
    assert_eq!(single.optimistic_hours, 1.5);
    assert_eq!(single.pessimistic_hours, 1.5);
    assert_eq!(duration_seconds(Some(single)), 5_400);
}

#[test]
fn lag_units_are_tenths_of_a_minute_and_invert_exactly() {
    assert_eq!(lag_tenths(8.0), 4_800);
    assert_eq!(lag_tenths(-3.0), -1_800);
    assert_eq!(lag_hours(14_400), 24.0);
    for tenths in [-14_400, -1, 0, 1, 7, 600, 24_000] {
        assert_eq!(lag_tenths(lag_hours(tenths)), tenths);
    }
}

#[test]
fn priority_bands_invert_the_written_values() {
    for priority in [
        Priority::P0,
        Priority::P1,
        Priority::P2,
        Priority::P3,
        Priority::P4,
    ] {
        assert_eq!(priority_from_value(priority_value(priority)), priority);
    }
    assert_eq!(priority_from_value(1000), Priority::P0);
    assert_eq!(priority_from_value(650), Priority::P1);
    assert_eq!(priority_from_value(0), Priority::P4);
}

#[test]
fn relation_codes_follow_the_mspdi_table() {
    for (code, kind) in [
        (0, DependencyKind::FinishFinish),
        (1, DependencyKind::FinishStart),
        (2, DependencyKind::StartFinish),
        (3, DependencyKind::StartStart),
    ] {
        assert_eq!(relation_from_code(code), Some(kind));
        assert_eq!(i64::from(relation_code(kind)), code);
    }
    assert_eq!(relation_from_code(4), None);
}

#[test]
fn derived_identities_are_stable_and_scoped_by_project_and_uid() {
    let project = Uuid::from_u128(0x6f1c_2a4e_1b7d_4c55_9a0e_3d2f_5b8c_9e10);
    let other = Uuid::from_u128(0x0b9d_7c6e_2f41_4a8b_b3c5_7e6f_1d2a_4b30);
    assert_eq!(derived_work_id(project, 9), derived_work_id(project, 9));
    assert_ne!(derived_work_id(project, 9), derived_work_id(project, 10));
    assert_ne!(derived_work_id(project, 9), derived_work_id(other, 9));
    assert_eq!(derived_work_id(project, 9).0.get_version_num(), 8);
    // Neighbouring UIDs must not share the leading half of the identity.
    let (a, b) = (
        derived_work_id(project, 1).0.as_u128() >> 64,
        derived_work_id(project, 2).0.as_u128() >> 64,
    );
    assert!((a ^ b).count_ones() > 16, "{a:x} {b:x}");
}

#[test]
fn identities_are_pinned_so_earlier_imports_keep_resolving() {
    let project = Uuid::from_u128(0x6f1c_2a4e_1b7d_4c55_9a0e_3d2f_5b8c_9e10);
    assert_eq!(
        derived_work_id(project, 9).to_string(),
        "8add68ca-5736-899d-9f3e-f437a926bc3c"
    );
    assert_eq!(
        scoped_work_id(ProjectId(project), "REL", 9).to_string(),
        "10509382-14c6-87fd-ae9a-71cb29dc1c46"
    );
}

#[test]
fn scoped_identities_separate_projects_prefixes_and_uids() {
    let project = ProjectId(Uuid::from_u128(0x6f1c_2a4e_1b7d_4c55_9a0e_3d2f_5b8c_9e10));
    let other = ProjectId(Uuid::from_u128(0x0b9d_7c6e_2f41_4a8b_b3c5_7e6f_1d2a_4b30));
    let id = scoped_work_id(project, "OP", 12);
    assert_eq!(id, scoped_work_id(project, "OP", 12));
    assert_ne!(id, scoped_work_id(other, "OP", 12));
    assert_ne!(id, scoped_work_id(project, "OQ", 12));
    assert_ne!(id, scoped_work_id(project, "OP", 13));
    // Length delimiting keeps a prefix from absorbing bytes of the UID.
    assert_ne!(
        scoped_work_id(project, "OP", 0x0100),
        scoped_work_id(project, "OP\u{0}", 0x01)
    );
    assert_ne!(id, derived_work_id(project.0, 12));
}

#[test]
fn guids_use_the_microsoft_project_spelling() {
    let id = Uuid::from_u128(0xa100_0000_0000_4000_8000_0000_0000_0001);
    assert_eq!(format_guid(id), "A1000000-0000-4000-8000-000000000001");
}

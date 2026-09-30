use super::*;
use crate::query::calibration::tests::Log;

fn holding(plan: &Plan, actor: &str) -> Vec<String> {
    let actor: ActorId = actor.parse().expect("actor");
    advisories(plan, &actor)
        .into_iter()
        .flat_map(|advisory| match advisory {
            Advisory::HoldingClaims { holding, .. } => holding.into_iter().map(|k| k.0),
        })
        .collect()
}

#[test]
fn claimed_started_and_blocked_work_is_held_until_it_is_submitted() {
    let mut log = Log::new();
    assert!(holding(&log.plan, "agent:coder").is_empty());
    log.claim("agent:coder", "TEST-A", 0.0);
    assert_eq!(holding(&log.plan, "agent:coder"), ["TEST-A"]);
    assert!(holding(&log.plan, "agent:other").is_empty());
    log.start("agent:coder", "TEST-A", 1.0, None);
    let work = log.id("TEST-A");
    let reason = "waiting".to_string();
    log.run("agent:coder", crate::Command::Block { work, reason }, 2.0);
    assert_eq!(holding(&log.plan, "agent:coder"), ["TEST-A"]);
    log.run("agent:coder", crate::Command::Unblock { work }, 3.0);
    log.submit("agent:coder", "TEST-A", 4.0, None);
    assert!(holding(&log.plan, "agent:coder").is_empty());
}

#[test]
fn the_advisory_names_the_actor_and_explains_itself() {
    let log = Log::scenario();
    let actor: ActorId = "agent:third".parse().expect("actor");
    let json = serde_json::to_value(advisories(&log.plan, &actor)).expect("json");
    assert_eq!(json[0]["kind"], "holding_claims");
    assert_eq!(json[0]["actor"]["name"], "third");
    assert_eq!(json[0]["holding"][0], "TEST-F");
    let reason = json[0]["reason"].as_str().expect("reason");
    assert!(
        reason.contains("neither filtered nor reordered"),
        "{reason}"
    );
}

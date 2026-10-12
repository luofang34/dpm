//! Inputs of the forecast queries, `project_status` and `get_schedule`.

use serde_json::{Map, Value, json};

pub(super) fn schema(name: &str, properties: &mut Map<String, Value>) {
    if name == "get_schedule" {
        properties.insert(
            "probabilistic".into(),
            json!({"type":"boolean","default":true,"description":"Add seeded criticality and finish percentiles; false projects the deterministic schedule only"}),
        );
        return;
    }
    properties.insert("probabilistic".into(), json!({"type":"boolean"}));
    properties.insert(
        "calibrated".into(),
        json!({"type":"boolean","default":false,"description":"Apply measured factors with enough samples to a copy of the plan and report them under calibration"}),
    );
    properties.insert(
        "horizon_hours".into(),
        json!({"type":"number","minimum":0,"description":"Elapsed hours after the clock reading; forecast then states the sampled probability of finishing the project and reaching each milestone by then, beside the analytic PERT approximation. Omitted leaves those probabilities null"}),
    );
}

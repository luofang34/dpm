use super::required;
use dpm_app::{AppError, Query};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Arguments of the MSPDI tools, which no other tool shares.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MspdiArguments {
    project_key: Option<String>,
    xml: Option<String>,
    key_prefix: Option<String>,
    match_existing_by: Option<dpm_app::ExistingMatch>,
    #[serde(default)]
    keep_existing_priority: bool,
}

pub(super) fn handles(name: &str) -> bool {
    matches!(name, "import_mspdi" | "export_mspdi")
}

/// Input schema for the read-only MSPDI tools; the CLI builds the same queries from flags.
pub(super) fn schema(name: &str, properties: &mut Map<String, Value>, needed: &mut Vec<&str>) {
    properties.insert(
        "project_key".into(),
        json!({"type":"string","minLength":1,"description":"Existing project key; not a directory"}),
    );
    needed.push("project_key");
    if name == "import_mspdi" {
        properties.insert(
            "xml".into(),
            json!({"type":"string","minLength":1,"description":"Complete MSPDI document text"}),
        );
        properties.insert(
            "key_prefix".into(),
            json!({"type":"string","minLength":1,"description":"Prefix for keys of new work (PREFIX-UID); defaults to the project key. Required for a document without GUIDs (such as OmniPlan's), where it names the source"}),
        );
        properties.insert(
            "match_existing_by".into(),
            json!({"type":"string","enum":["title-path"],"description":"Opt-in: match tasks without a GUID to the one existing work item in the project with the same title path (titles from the project root down); ambiguity refuses the import. Absent: never match"}),
        );
        properties.insert(
            "keep_existing_priority".into(),
            json!({"type":"boolean","description":"Keep the priority of existing work and report differing source values; for tools such as OmniPlan that rescale priorities by the highest one in the document. Default false"}),
        );
        needed.push("xml");
    }
}

pub(super) fn query(name: &str, value: Value) -> Result<Query, AppError> {
    let args: MspdiArguments = serde_json::from_value(value)?;
    let project_key = required(args.project_key, "project_key")?;
    Ok(if name == "import_mspdi" {
        Query::ImportMspdi {
            xml: required(args.xml, "xml")?,
            project_key,
            key_prefix: args.key_prefix,
            match_existing_by: args.match_existing_by,
            keep_existing_priority: args.keep_existing_priority,
        }
    } else {
        Query::ExportMspdi { project_key }
    })
}

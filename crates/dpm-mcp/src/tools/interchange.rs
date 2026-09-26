use super::{Arguments, required};
use dpm_app::{AppError, Query};
use serde_json::{Map, Value, json};

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
            json!({"type":"string","minLength":1,"description":"Prefix for keys of new work (PREFIX-UID); defaults to the project key"}),
        );
        needed.push("xml");
    }
}

pub(super) fn query(name: &str, args: &Arguments) -> Result<Query, AppError> {
    let project_key = required(args.project_key.clone(), "project_key")?;
    Ok(if name == "import_mspdi" {
        Query::ImportMspdi {
            xml: required(args.xml.clone(), "xml")?,
            project_key,
            key_prefix: args.key_prefix.clone(),
        }
    } else {
        Query::ExportMspdi { project_key }
    })
}

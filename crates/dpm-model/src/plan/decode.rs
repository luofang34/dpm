//! Inspect the version before decoding domain fields, independent of document field order.

use super::Plan;
use serde::{Deserialize, Deserializer, de};

mod value;
use value::Buffered;

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if !deserializer.is_human_readable() {
            return Shape::deserialize(deserializer);
        }
        let buffered = Buffered::deserialize(deserializer)?;
        if let Buffered::Map(fields) = &buffered {
            let version = fields
                .iter()
                .find(|(key, _)| key == "format_version")
                .ok_or_else(|| de::Error::missing_field("format_version"))?;
            super::read_format_version(version.1.clone()).map_err(de::Error::custom)?;
        }
        Shape::deserialize(buffered).map_err(de::Error::custom)
    }
}

// The remote derive constructs Plan itself, so missing or mistyped model fields fail compilation.
#[derive(Deserialize)]
#[serde(remote = "Plan", rename = "Plan", deny_unknown_fields)]
struct Shape {
    #[serde(deserialize_with = "super::read_format_version")]
    format_version: u32,
    assets: std::collections::BTreeMap<crate::AssetId, crate::WorkspaceAsset>,
    workspace: crate::Workspace,
    revision: u64,
    projects: std::collections::BTreeMap<crate::ProjectId, crate::Project>,
    work_items: std::collections::BTreeMap<crate::WorkItemId, crate::WorkItem>,
    requirements: std::collections::BTreeMap<crate::RequirementId, crate::Requirement>,
    artifacts: std::collections::BTreeMap<crate::ArtifactId, crate::Artifact>,
    decisions: std::collections::BTreeMap<crate::DecisionId, crate::Decision>,
    risks: std::collections::BTreeMap<crate::RiskId, crate::Risk>,
    dependencies: Vec<crate::Dependency>,
    #[serde(default)]
    external_references:
        std::collections::BTreeMap<crate::ExternalReferenceId, crate::ExternalReference>,
    #[serde(default)]
    links: Vec<crate::WorkLink>,
    #[serde(default)]
    calendars: Option<crate::Calendars>,
}

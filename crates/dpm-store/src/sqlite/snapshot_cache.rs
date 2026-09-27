//! Reuse validated decoding only when SQLite returns the exact same snapshot bytes.
use super::snapshot;
use crate::{StoreError, StoredRecord};
use dpm_model::Plan;
use std::{cell::RefCell, path::Path};

#[derive(Default)]
pub(super) struct SnapshotCache(RefCell<Option<Box<(String, Plan)>>>);
impl SnapshotCache {
    pub(super) fn decode(&self, json: &str, path: &Path) -> Result<Plan, StoreError> {
        if let Some((_, plan)) = self
            .0
            .borrow()
            .as_deref()
            .filter(|(cached, _)| cached == json)
        {
            return Ok(plan.clone());
        }
        let plan: Plan = snapshot::decode(json, path, StoredRecord::Snapshot, "snapshot_json")?;
        plan.validate()?;
        *self.0.borrow_mut() = Some(Box::new((json.into(), plan.clone())));
        Ok(plan)
    }
}

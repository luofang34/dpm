//! Preserve unaffected positions while adopting the source's sibling sequence.

use super::Outline;
use crate::InterchangeError;
use dpm_model::{Plan, SiblingOrder, WorkItemId};
use std::collections::BTreeMap;

pub(super) fn assign(current: &Plan, outline: &mut Outline) -> Result<(), InterchangeError> {
    let mut siblings: BTreeMap<Option<WorkItemId>, Vec<i64>> = BTreeMap::new();
    for report in &outline.reports {
        if let Some(work) = outline.work.get(&report.uid) {
            siblings.entry(work.parent).or_default().push(report.uid);
        }
    }
    for uids in siblings.values() {
        let mut previous: Option<(SiblingOrder, WorkItemId)> = None;
        for (index, uid) in uids.iter().enumerate() {
            let Some(work) = outline.work.get(uid) else {
                continue;
            };
            let existing = current
                .work_items
                .get(&work.id)
                .filter(|old| old.parent == work.parent);
            let position = existing.map(|old| (old.order.clone(), old.id));
            let order = if let Some((order, _)) =
                position.filter(|p| previous.as_ref().is_none_or(|prev| prev < p))
            {
                order
            } else {
                let next = uids[index + 1..]
                    .iter()
                    .filter_map(|uid| outline.work.get(uid))
                    .filter_map(|w| {
                        current
                            .work_items
                            .get(&w.id)
                            .filter(|old| old.parent == w.parent)
                    })
                    .map(|w| &w.order)
                    .find(|order| previous.as_ref().is_none_or(|prev| &prev.0 < *order));
                SiblingOrder::between(previous.as_ref().map(|p| &p.0), next)
                    .map_err(|source| InterchangeError::OutlineOrder { uid: *uid, source })?
            };
            previous = Some((order.clone(), work.id));
            if let Some(work) = outline.work.get_mut(uid) {
                work.order = order;
            }
        }
    }
    Ok(())
}

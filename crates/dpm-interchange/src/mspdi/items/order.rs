//! Preserve stable anchors while adopting the source's sibling sequence.

use super::Outline;
use crate::InterchangeError;
use dpm_model::{OrderError, Plan, SiblingOrder, WorkItemId};
use std::collections::{BTreeMap, BTreeSet};

mod anchors;

type Position = (SiblingOrder, WorkItemId);

pub(super) fn assign(current: &Plan, outline: &mut Outline) -> Result<(), InterchangeError> {
    let protected = dpm_engine::protected_work(current);
    let mut siblings: BTreeMap<Option<WorkItemId>, Vec<i64>> = BTreeMap::new();
    for report in &outline.reports {
        if let Some(work) = outline.work.get(&report.uid) {
            siblings.entry(work.parent).or_default().push(report.uid);
        }
    }
    for uids in siblings.values() {
        let positions: Vec<_> = uids
            .iter()
            .map(|uid| {
                let work = outline.work.get(uid)?;
                current
                    .work_items
                    .get(&work.id)
                    .filter(|old| old.parent == work.parent)
                    .map(|old| (old.order.clone(), old.id))
            })
            .collect();
        let fixed = anchors::select(uids, &positions, &protected)?;
        assign_positions(outline, uids, &positions, &fixed)?;
    }
    Ok(())
}

fn assign_positions(
    outline: &mut Outline,
    uids: &[i64],
    positions: &[Option<Position>],
    fixed: &BTreeSet<usize>,
) -> Result<(), InterchangeError> {
    let mut previous: Option<Position> = None;
    let mut anchors = fixed.iter().copied().peekable();
    for (index, uid) in uids.iter().enumerate() {
        // Only mapped work has a place among its siblings.
        let Some(work) = outline.work.get(uid) else {
            continue;
        };
        let position = if anchors.peek() == Some(&index) {
            anchors.next();
            positions.get(index).cloned().flatten()
        } else {
            None
        };
        let (order, id) = match position {
            Some(position) => position,
            None => {
                let next = anchors
                    .peek()
                    .and_then(|i| positions.get(*i))
                    .and_then(Option::as_ref);
                let order = between(previous.as_ref(), next, work.id)
                    .map_err(|source| InterchangeError::OutlineOrder { uid: *uid, source })?;
                (order, work.id)
            }
        };
        previous = Some((order.clone(), id));
        if let Some(work) = outline.work.get_mut(uid) {
            work.order = order;
        }
    }
    Ok(())
}

fn between(
    left: Option<&Position>,
    right: Option<&Position>,
    id: WorkItemId,
) -> Result<SiblingOrder, OrderError> {
    if let Some((left, right)) = left.zip(right)
        && left.0 == right.0
    {
        // Concurrent positions can coincide; never renumber protected neighbours to hide a conflict.
        return if left.1 < id && id < right.1 {
            Ok(left.0.clone())
        } else {
            Err(OrderError::InvalidBounds)
        };
    }
    SiblingOrder::between(left.map(|p| &p.0), right.map(|p| &p.0))
}

//! Keep protected work fixed, then retain a longest increasing sequence in each free interval.

use super::Position;
use crate::InterchangeError;
use dpm_model::{OrderError, WorkItemId};
use std::collections::BTreeSet;

pub(super) fn select(
    uids: &[i64],
    positions: &[Option<Position>],
    protected: &BTreeSet<WorkItemId>,
) -> Result<BTreeSet<usize>, InterchangeError> {
    let required: Vec<_> = positions
        .iter()
        .enumerate()
        .filter(|(_, p)| p.as_ref().is_some_and(|p| protected.contains(&p.1)))
        .map(|(i, _)| i)
        .collect();
    let mut anchors = BTreeSet::new();
    let mut start = 0;
    let mut left = None;
    for end in required.into_iter().chain(std::iter::once(positions.len())) {
        let right = positions.get(end).and_then(Option::as_ref);
        if left.zip(right).is_some_and(|(a, b)| a >= b) {
            return Err(InterchangeError::OutlineOrder {
                uid: uids[end],
                source: OrderError::InvalidBounds,
            });
        }
        anchors.extend(increasing(positions, start, end, left, right));
        if right.is_some() {
            anchors.insert(end);
        }
        left = right;
        start = end + 1;
    }
    Ok(anchors)
}

fn increasing(
    positions: &[Option<Position>],
    start: usize,
    end: usize,
    left: Option<&Position>,
    right: Option<&Position>,
) -> Vec<usize> {
    let mut tails: Vec<(usize, &Position)> = Vec::new();
    let mut previous = BTreeSet::new();
    let mut links = std::collections::BTreeMap::new();
    for (index, position) in positions.iter().enumerate().take(end).skip(start) {
        let Some(position) = position
            .as_ref()
            .filter(|p| left.is_none_or(|left| left < *p) && right.is_none_or(|right| *p < right))
        else {
            continue;
        };
        let slot = tails.partition_point(|(_, tail)| *tail < position);
        if slot > 0 {
            links.insert(index, tails[slot - 1].0);
        }
        if slot == tails.len() {
            tails.push((index, position));
        } else {
            tails[slot] = (index, position);
        }
    }
    let mut cursor = tails.last().map(|(index, _)| *index);
    while let Some(index) = cursor {
        previous.insert(index);
        cursor = links.get(&index).copied();
    }
    previous.into_iter().collect()
}

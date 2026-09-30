//! Ratio distributions, with what each group left out.

use super::super::{
    EstimateSample, ExclusionReason, MIN_SAMPLES, RatioGroup, ReasonCount, ascending,
};
use super::Excluded;
use dpm_model::ActorKind;
use dpm_schedule::percentile;
use std::collections::{BTreeMap, BTreeSet};

/// Ratios and exclusion counts collected for one group.
#[derive(Default)]
struct Collected {
    ratios: Vec<f64>,
    excluded: BTreeMap<ExclusionReason, usize>,
}

type GroupKey = (ActorKind, Option<String>);

/// Ratio distributions per executor kind, or per executor kind and capability; a kind whose
/// verified work was all excluded still has a group, so its exclusions stay visible.
pub(super) fn groups(
    samples: &[EstimateSample],
    excluded: &[Excluded<'_>],
    by_capability: bool,
) -> Vec<RatioGroup> {
    let keys = |kind: ActorKind, capabilities: &BTreeSet<String>| -> Vec<GroupKey> {
        if by_capability {
            capabilities
                .iter()
                .map(|c| (kind, Some(c.clone())))
                .collect()
        } else {
            vec![(kind, None)]
        }
    };
    let mut collected: BTreeMap<GroupKey, Collected> = BTreeMap::new();
    for sample in samples {
        for key in keys(sample.executor, &sample.capabilities) {
            collected.entry(key).or_default().ratios.push(sample.ratio);
        }
    }
    for item in excluded {
        let Some(kind) = item.kind else { continue };
        for key in keys(kind, item.capabilities) {
            let count = collected
                .entry(key)
                .or_default()
                .excluded
                .entry(item.reason)
                .or_default();
            *count = count.wrapping_add(1);
        }
    }
    collected
        .into_iter()
        .map(|((executor, capability), collected)| group(executor, capability, collected))
        .collect()
}

fn group(executor: ActorKind, capability: Option<String>, collected: Collected) -> RatioGroup {
    let sorted = ascending(collected.ratios);
    let known = !sorted.is_empty();
    RatioGroup {
        executor,
        capability,
        samples: sorted.len(),
        median: known.then(|| percentile(&sorted, 0.5)),
        p25: known.then(|| percentile(&sorted, 0.25)),
        p75: known.then(|| percentile(&sorted, 0.75)),
        sufficient: sorted.len() >= MIN_SAMPLES,
        excluded: collected
            .excluded
            .into_iter()
            .map(|(reason, count)| ReasonCount { reason, count })
            .collect(),
    }
}

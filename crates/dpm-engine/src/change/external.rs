//! Reviewed changes to external references follow the link rules for the object, not for the
//! record id: a record that replaces another record of the same object is an edit of that object.

use super::invalid;
use crate::EngineError;
use dpm_model::{ExternalReference, Plan};

/// Each proposed record is compared with every current record it continues: the one with its id
/// (a relabel or namespace move) and the one naming its object (a removal and re-addition under a
/// new id). Observations are attributed records, so none may be added, rewritten or dropped, and a
/// kind may only change as linking would change it.
pub(super) fn protect_references(current: &Plan, proposed: &Plan) -> Result<(), EngineError> {
    for (id, next) in &proposed.external_references {
        let key = next.identity.object_key();
        let continued = current
            .external_references
            .values()
            .filter(|record| record.id == *id || record.identity.object_key() == key);
        let mut matched = false;
        for previous in continued {
            matched = true;
            protect_edit(previous, next)?;
        }
        if !matched && next.observation.is_some() {
            return Err(invalid(
                id,
                "observations are recorded by link; plan changes cannot add or rewrite them",
            ));
        }
    }
    Ok(())
}

fn protect_edit(previous: &ExternalReference, next: &ExternalReference) -> Result<(), EngineError> {
    let rekeyed = previous.id != next.id;
    // A repository transfer or a server migration keeps the object; another number, number space
    // or provider names a different object, so an attributed observation cannot move with it.
    let (old, new) = (previous.identity.object_key(), next.identity.object_key());
    let same_object = old.family == new.family && old.space == new.space && old.id == new.id;
    if !same_object && previous.observation.is_some() {
        return Err(invalid(
            next.id,
            "a reviewed change may move a record to another namespace or instance, but another \
             number, number space or provider is a different object and its observation cannot \
             move with it; unlink this record and link the other object",
        ));
    }
    if !previous.identity.permits_kind_change_to(&next.identity) {
        return Err(invalid(
            next.id,
            "a recorded kind may only change within its number space, and a pull request stays \
             a pull request, including when a plan change re-adds the object under a new record; \
             link the other object instead",
        ));
    }
    if next.observation != previous.observation {
        return Err(invalid(
            next.id,
            if rekeyed {
                "a record replacing one for the same object keeps its observation; observations \
                 are recorded by link, so plan changes cannot add, rewrite or drop them"
            } else {
                "observations are recorded by link; plan changes cannot add or rewrite them"
            },
        ));
    }
    Ok(())
}

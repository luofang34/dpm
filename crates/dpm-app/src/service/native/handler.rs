//! Answering native calls with the shared application.

use super::{
    error::NativeError,
    protocol::{
        Attached, Attachment, Committed, HelloResult, NATIVE_PROTOCOL_VERSION, NATIVE_PROTOCOLS,
        NativeCall, NativeEnvelope, NativeRequest, NativeResponse, NativeResult, ProjectWatermark,
        SourceKind, View, ViewBasis, Watermark,
    },
    views::capabilities,
};
use crate::{AppError, Application, Query, QueryClock};
use chrono::{DateTime, Utc};
use dpm_model::Timeline;
use serde_json::{Value, json};

/// How many times a view or a poll is re-read before the boundary gives up and says the history
/// kept changing identity or revision under it.
pub(super) const MAX_ATTEMPTS: u32 = 3;

/// The project half of a watermark with the kind of source it was read from.
pub(super) struct ProjectState {
    pub(super) watermark: ProjectWatermark,
    pub(super) source: SourceKind,
}

/// Refuse a protocol version this build does not speak.
fn supported_protocol(offered: u32) -> Result<(), NativeError> {
    if NATIVE_PROTOCOLS.contains(&offered) {
        return Ok(());
    }
    Err(NativeError::UnsupportedProtocol {
        offered: vec![offered],
        supported: NATIVE_PROTOCOLS.to_vec(),
    })
}

impl Application {
    /// Answer one request line with one response line.
    ///
    /// The version is checked before the call is decoded, so a request from a newer client is
    /// refused as `unsupported_protocol` rather than as a malformed one. This is the whole host
    /// side of the boundary: a helper process reads lines from a pipe and a foreign-function host
    /// passes strings, and both call this.
    pub fn native_json_blocking(&mut self, line: &str) -> String {
        let response = match serde_json::from_str::<Value>(line) {
            Err(error) => failure("", NativeError::Malformed(error)),
            Ok(value) => self.native_value_blocking(&value),
        };
        serde_json::to_string(&response).unwrap_or_else(|error| {
            json!({"protocol": NATIVE_PROTOCOL_VERSION, "id": response.id, "ok": false,
                   "error": {"api_version": crate::API_VERSION, "code": "invalid_request",
                             "message": format!("response could not be encoded: {error}")}})
            .to_string()
        })
    }

    fn native_value_blocking(&mut self, value: &Value) -> NativeResponse {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let negotiating = value.pointer("/call/type").and_then(Value::as_str) == Some("hello");
        // Checked here as well as in `native_blocking` so a call this build cannot decode is
        // still refused for its version.
        if let Some(offered) = value.get("protocol").and_then(Value::as_u64)
            && !negotiating
            && let Err(error) = supported_protocol(u32::try_from(offered).unwrap_or(u32::MAX))
        {
            return failure(&id, error);
        }
        match serde_json::from_value::<NativeRequest>(value.clone()) {
            Ok(request) => self.native_blocking(request),
            Err(error) => failure(&id, NativeError::Malformed(error)),
        }
    }

    /// Answer one decoded request.
    ///
    /// This is the boundary every caller crosses, typed or through JSON: the request's protocol is
    /// checked here, before any call runs, so an unsupported version can never reach a command.
    /// `hello` alone is exempt, because it is how a version is negotiated.
    pub fn native_blocking(&mut self, request: NativeRequest) -> NativeResponse {
        let NativeRequest { protocol, id, call } = request;
        if !matches!(call, NativeCall::Hello { .. })
            && let Err(error) = supported_protocol(protocol)
        {
            return failure(&id, error);
        }
        match self.call_blocking(call) {
            Ok(result) => NativeResponse {
                protocol: NATIVE_PROTOCOL_VERSION,
                id,
                ok: true,
                result: Some(result),
                error: None,
            },
            Err(error) => failure(&id, error),
        }
    }

    fn call_blocking(&mut self, call: NativeCall) -> Result<NativeResult, NativeError> {
        Ok(match call {
            NativeCall::Hello { protocols } => {
                NativeResult::Hello(self.hello_blocking(&protocols)?)
            }
            NativeCall::Attach { expect_workspace } => {
                NativeResult::Attached(self.attach_blocking(expect_workspace)?)
            }
            NativeCall::Query { query, attached } => {
                NativeResult::View(self.view_blocking(query, attached)?)
            }
            NativeCall::Changes {
                since,
                limit,
                attached,
            } => NativeResult::Changes(self.changes_blocking(&since, limit, attached)?),
            NativeCall::Command { request, attached } => {
                self.check_attached_blocking(attached, true)?;
                let operation = self.execute_blocking(request)?;
                NativeResult::Committed(Committed {
                    envelope: NativeEnvelope {
                        api_version: crate::API_VERSION,
                        revision: Some(operation.operation.resulting_revision),
                        lineage_id: Some(operation.lineage_id),
                        data: operation,
                    },
                })
            }
        })
    }

    fn hello_blocking(&self, offered: &[u32]) -> Result<HelloResult, NativeError> {
        let protocol = NATIVE_PROTOCOLS
            .iter()
            .rev()
            .copied()
            .find(|supported| offered.contains(supported))
            .ok_or_else(|| NativeError::UnsupportedProtocol {
                offered: offered.to_vec(),
                supported: NATIVE_PROTOCOLS.to_vec(),
            })?;
        let live = self
            .project_state_blocking()
            .is_ok_and(|state| state.source == SourceKind::Live);
        Ok(HelloResult {
            protocol,
            supported: NATIVE_PROTOCOLS.to_vec(),
            api_version: crate::API_VERSION,
            capabilities: capabilities(live),
        })
    }

    fn attach_blocking(
        &self,
        expect_workspace: Option<dpm_model::WorkspaceId>,
    ) -> Result<Attached, NativeError> {
        self.check_source_blocking()?;
        let state = self.project_state_blocking()?;
        if let Some(expected) = expect_workspace
            && expected != state.watermark.workspace_id
        {
            return Err(NativeError::WorkspaceMismatch {
                expected,
                actual: state.watermark.workspace_id,
            });
        }
        Ok(Attached {
            attachment: Attachment {
                workspace_id: state.watermark.workspace_id,
                lineage_id: state.watermark.lineage_id,
            },
            source: state.source,
            watermark: self.watermark_blocking()?,
            evaluated_at: self.clock.now(),
        })
    }

    /// Compute a shared query at one clock reading, anchored to where each store it reads stood.
    ///
    /// The clock is pinned for the call, so every time-dependent value in the answer was evaluated
    /// at the `evaluated_at` it returns. Each store the query reads is anchored before it is read:
    /// the project always, the run store only for a run query. An anchor is a lower bound of the
    /// answer, so a subscription from it misses nothing; it may repeat what the answer already
    /// holds, which a consumer discards by feed identity and sequence. Nothing waits for another
    /// store, or for writes to stop, so continuous telemetry cannot starve a read.
    fn view_blocking(
        &mut self,
        query: Query,
        attached: Option<Attachment>,
    ) -> Result<View, NativeError> {
        self.check_attached_blocking(attached, true)?;
        let evaluated_at = self.clock.now();
        let previous = self.clock;
        self.clock = QueryClock::Fixed(evaluated_at);
        let read = self.read_view_blocking(&query, evaluated_at, || {});
        self.clock = previous;
        read
    }

    /// The attempts of one view: anchor, read, and keep the answer only if it is the same history
    /// the anchor was taken in and, for a gate-reading query, the same revision its release
    /// instant was computed from. `after_anchor` runs between the anchor and the read of each
    /// attempt, which is where a concurrent write is hardest on a reader; production passes
    /// nothing.
    pub(super) fn read_view_blocking(
        &self,
        query: &Query,
        evaluated_at: DateTime<Utc>,
        mut after_anchor: impl FnMut(),
    ) -> Result<View, NativeError> {
        for attempt in 1..=MAX_ATTEMPTS {
            let basis = self.anchor_blocking(query)?;
            after_anchor();
            let response = self.query_blocking(query.clone())?;
            let release = if reads_gates(query) {
                let plan = self.plan_blocking()?;
                (plan.revision == response.revision)
                    .then(|| Timeline::at(&plan, evaluated_at).next_release(&plan))
            } else {
                Some(None)
            };
            let same_history = basis
                .project
                .is_some_and(|anchor| anchor.lineage_id == response.lineage_id);
            if let (true, Some(release)) = (same_history, release) {
                return Ok(View {
                    evaluated_at,
                    basis,
                    refresh_at: [earliest_stale_at(&response.data), release]
                        .into_iter()
                        .flatten()
                        .min(),
                    envelope: response.into(),
                });
            }
            tracing::debug!(
                attempt,
                "the project changed under a native view; reading again"
            );
        }
        Err(NativeError::Changing {
            attempts: MAX_ATTEMPTS,
        })
    }

    /// Where each store a query reads stood, read before the query reads it.
    pub(super) fn anchor_blocking(&self, query: &Query) -> Result<ViewBasis, NativeError> {
        Ok(ViewBasis {
            project: Some(self.project_state_blocking()?.watermark),
            runs: reads_runs(query)
                .then(|| self.run_heads_blocking())
                .transpose()?,
        })
    }

    /// Refuse a call whose attachment no longer describes the source, or whose source is no longer
    /// the one the locator selects. The source is checked first: a repointed locator makes every
    /// reading of the old source stale, whatever the client attached to.
    pub(super) fn check_attached_blocking(
        &self,
        attached: Option<Attachment>,
        strict_lineage: bool,
    ) -> Result<(), NativeError> {
        self.check_source_blocking()?;
        let state = self.project_state_blocking()?;
        if let Some(attached) = attached {
            if attached.workspace_id != state.watermark.workspace_id {
                return Err(NativeError::WorkspaceMismatch {
                    expected: attached.workspace_id,
                    actual: state.watermark.workspace_id,
                });
            }
            if strict_lineage && attached.lineage_id != state.watermark.lineage_id {
                return Err(NativeError::LineageMismatch {
                    expected: attached.lineage_id,
                    actual: state.watermark.lineage_id,
                });
            }
        }
        Ok(())
    }

    pub(super) fn project_state_blocking(&self) -> Result<ProjectState, AppError> {
        let workspace_id = self.workspace_identity_blocking()?;
        #[cfg(feature = "sqlite")]
        if let super::super::Backing::Database(store) = &self.backing {
            let found = store.revision_blocking()?.ok_or(AppError::NotInitialized)?;
            return Ok(ProjectState {
                watermark: ProjectWatermark {
                    workspace_id,
                    lineage_id: Some(found.lineage.lineage_id),
                    revision: found.revision,
                    history_head: found.history_head,
                },
                source: if found.lineage.archived {
                    SourceKind::Archive
                } else {
                    SourceKind::Live
                },
            });
        }
        let found = self.revision_blocking()?;
        Ok(ProjectState {
            watermark: ProjectWatermark {
                workspace_id,
                lineage_id: found.lineage_id,
                revision: found.revision,
                history_head: 0,
            },
            source: SourceKind::Preview,
        })
    }

    /// Where the project history and both run feeds stand, each read in its own transaction.
    pub(super) fn watermark_blocking(&self) -> Result<Watermark, AppError> {
        Ok(Watermark {
            project: self.project_state_blocking()?.watermark,
            runs: self.run_heads_blocking()?,
        })
    }
}

fn failure(id: &str, error: NativeError) -> NativeResponse {
    NativeResponse {
        protocol: NATIVE_PROTOCOL_VERSION,
        id: id.to_string(),
        ok: false,
        result: None,
        error: Some(error.body()),
    }
}

/// Whether a query reads the run store. Every other query reads only the project, so run
/// activity is none of its business and never delays or taints it.
fn reads_runs(query: &Query) -> bool {
    matches!(
        query,
        Query::Runs { .. }
            | Query::Run { .. }
            | Query::RunLifecycle { .. }
            | Query::RunActivity { .. }
    )
}

/// Whether an answer's readiness, remaining time or forecast depends on gates whose lag elapses.
fn reads_gates(query: &Query) -> bool {
    matches!(
        query,
        Query::Status { .. } | Query::Next { .. } | Query::Explain { .. } | Query::Show { .. }
    )
}

/// The first instant a run in the answer turns stale with no new revision, if any run is
/// unfinished and fresh.
fn earliest_stale_at(data: &Value) -> Option<DateTime<Utc>> {
    let parse = |value: &Value| {
        value
            .get("stale_at")
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|at| at.with_timezone(&Utc))
    };
    let mut times: Vec<DateTime<Utc>> = data
        .get("runs")
        .and_then(Value::as_array)
        .map(|runs| runs.iter().filter_map(parse).collect())
        .unwrap_or_default();
    times.extend(parse(data));
    times.into_iter().min()
}

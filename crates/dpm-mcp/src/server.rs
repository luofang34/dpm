use crate::tools::{call_tool_blocking, definitions};
use dpm_app::Application;
use dpm_model::ActorId;
use serde_json::{Value, json};

/// One MCP connection, bound to a configured local principal rather than caller-supplied identities.
pub struct McpServer {
    application: Application,
    actor: ActorId,
    initialized: bool,
    ready: bool,
}

impl McpServer {
    /// Bind the process to a local actor; separate verifiers use separate configured processes.
    pub fn new(application: Application, actor: ActorId) -> Self {
        Self {
            application,
            actor,
            initialized: false,
            ready: false,
        }
    }
    /// Handle a single JSON-RPC message; valid notifications never produce responses.
    pub fn handle_blocking(&mut self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str);
        if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || method.is_none()
            || id
                .as_ref()
                .is_some_and(|v| !(v.is_string() || v.is_i64() || v.is_u64()))
        {
            return Some(error(Value::Null, -32600, "Invalid Request"));
        }
        let method = method.unwrap_or_default();
        if id.is_none() {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" if !self.initialized => self.initialize(&params),
            "ping" => Ok(json!({})),
            "tools/list" if self.ready => Ok(json!({"tools":definitions()})),
            "tools/call" if self.ready => self.call_blocking(params),
            _ if !self.ready => Err((-32002, "Server not initialized".to_owned())),
            _ => Err((-32601, "Method not found".to_owned())),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code, message)) => error(id, code, &message),
        })
    }
    fn initialize(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        if !params.get("protocolVersion").is_some_and(Value::is_string)
            || !params.get("capabilities").is_some_and(Value::is_object)
            || !params.get("clientInfo").is_some_and(Value::is_object)
        {
            return Err((
                -32602,
                "initialize requires protocolVersion, capabilities and clientInfo".into(),
            ));
        }
        self.initialized = true;
        Ok(
            json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false}},
            "serverInfo":{"name":"dpm","version":env!("CARGO_PKG_VERSION")},
            "instructions":format!("Configured actor: {}. Query next_work and explain_work, then mutate using the observed base_revision. Verification requires a distinct configured actor.",self.actor)}),
        )
    }
    fn call_blocking(&mut self, params: Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "tool name is required".into()))?;
        if !definitions().iter().any(|d| d["name"] == name) {
            return Err((-32602, format!("Unknown tool {name}")));
        }
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match call_tool_blocking(&mut self.application, &self.actor, name, args) {
            Ok(data) => Ok(
                json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data,"isError":false}),
            ),
            Err(error) => {
                let data =
                    serde_json::to_value(error.response()).map_err(|e| (-32603, e.to_string()))?;
                Ok(
                    json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data,"isError":true}),
                )
            }
        }
    }
}

pub(crate) fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

#[cfg(test)]
mod tests;

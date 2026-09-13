//! Small, side-effect-free helpers for MCP runtime admission and error shaping.

use super::{
    HttpResponse, MCP_RUNTIME_DEFAULT_TIMEOUT, MCP_RUNTIME_INITIALIZE_TIMEOUT,
    MCP_RUNTIME_MAX_TIMEOUT,
};
use crate::mcp_server::jsonrpc_error;
use serde_json::Value;
use std::time::Duration;

pub(super) fn payload_contains_blocking_tool_call(payload: &Value) -> bool {
    let mut pending = vec![payload];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items.iter().rev()),
            Value::Object(object) => {
                if object.get("method").and_then(Value::as_str) != Some("tools/call") {
                    continue;
                }
                let Some(params) = object.get("params").and_then(Value::as_object) else {
                    continue;
                };
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = params.get("arguments").and_then(Value::as_object);
                let background = arguments
                    .and_then(|arguments| arguments.get("background"))
                    .and_then(Value::as_bool)
                    == Some(true);
                if (name == "run_shell" && !background)
                    || matches!(name, "computer_use" | "desktop_control")
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[derive(Debug)]
pub(super) struct JsonRpcErrorShape {
    ids: Vec<Value>,
    batch: bool,
}

impl JsonRpcErrorShape {
    pub(super) fn from_payload(payload: &Value) -> Self {
        fn response_id(value: &Value) -> Option<Value> {
            match value {
                Value::Object(object) => object.get("id").cloned(),
                _ => Some(Value::Null),
            }
        }
        match payload {
            Value::Array(items) => Self {
                ids: items.iter().filter_map(response_id).collect(),
                batch: true,
            },
            value => Self {
                ids: response_id(value).into_iter().collect(),
                batch: false,
            },
        }
    }

    pub(super) fn into_response(self, code: i64, message: String) -> HttpResponse {
        if self.ids.is_empty() {
            return HttpResponse::empty(202);
        }
        let mut responses = self
            .ids
            .into_iter()
            .map(|id| jsonrpc_error(id, code, message.clone()))
            .collect::<Vec<_>>();
        if self.batch {
            HttpResponse::json(200, Value::Array(responses))
        } else {
            HttpResponse::json(
                200,
                responses
                    .pop()
                    .expect("non-batch JSON-RPC error response has one id"),
            )
        }
    }
}

pub(super) fn mcp_jsonrpc_error_response(
    payload: &Value,
    code: i64,
    message: String,
) -> HttpResponse {
    JsonRpcErrorShape::from_payload(payload).into_response(code, message)
}

pub(super) fn runtime_timeout_for_payload(payload: &Value) -> Duration {
    fn one(payload: &Value) -> Duration {
        let Some(object) = payload.as_object() else {
            return MCP_RUNTIME_DEFAULT_TIMEOUT;
        };
        if object.get("method").and_then(Value::as_str) == Some("initialize") {
            return MCP_RUNTIME_INITIALIZE_TIMEOUT;
        }
        if object.get("method").and_then(Value::as_str) != Some("tools/call") {
            return MCP_RUNTIME_DEFAULT_TIMEOUT;
        }
        let Some(params) = object.get("params").and_then(Value::as_object) else {
            return MCP_RUNTIME_DEFAULT_TIMEOUT;
        };
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = params.get("arguments").and_then(Value::as_object);
        let requested = match name {
            "run_shell" => arguments
                .and_then(|value| value.get("timeoutSeconds"))
                .and_then(Value::as_u64)
                .map(|seconds| Duration::from_secs(seconds.saturating_add(30))),
            "computer_use" | "desktop_control" => arguments
                .and_then(|value| value.get("timeout_ms"))
                .and_then(Value::as_u64)
                .map(|millis| Duration::from_millis(millis.saturating_add(30_000))),
            _ => None,
        };
        requested
            .unwrap_or(MCP_RUNTIME_DEFAULT_TIMEOUT)
            .min(MCP_RUNTIME_MAX_TIMEOUT)
    }
    match payload {
        Value::Array(items) => items
            .iter()
            .map(one)
            .max()
            .unwrap_or(MCP_RUNTIME_DEFAULT_TIMEOUT),
        _ => one(payload),
    }
}

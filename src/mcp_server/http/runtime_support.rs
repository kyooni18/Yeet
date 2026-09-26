//! Small, side-effect-free helpers for MCP runtime admission and error shaping.

use super::{
    HttpResponse, LegacyAffinityState, MCP_LEGACY_HANDLE_IDLE_TTL, MCP_LEGACY_STICKY_IDLE_TTL,
};
use crate::mcp_server::jsonrpc_error;
use serde_json::Value;
use std::time::Instant;

impl LegacyAffinityState {
    pub(super) fn prune_expired_handles(&mut self, now: Instant) {
        self.handles.retain(|_, binding| {
            now.saturating_duration_since(binding.last_used) < MCP_LEGACY_HANDLE_IDLE_TTL
        });
        let handles = &self.handles;
        self.handle_order
            .retain(|handle| handles.contains_key(handle));
    }

    pub(super) fn prune_expired_sticky(&mut self, now: Instant) {
        self.sticky.retain(|_, binding| {
            now.saturating_duration_since(binding.last_used) < MCP_LEGACY_STICKY_IDLE_TTL
        });
    }

    pub(super) fn lane_is_pinned(&self, lane: usize) -> bool {
        self.handles.values().any(|binding| binding.lane == lane)
            || self.sticky.values().any(|binding| binding.lane == lane)
    }

    pub(super) fn forget_preferred_lane(&mut self, lane: usize) {
        self.preferred.retain(|_, owner| *owner != lane);
    }
}

#[cfg(test)]
#[test]
fn expired_sticky_affinity_no_longer_pins_lane() {
    let now = Instant::now();
    let mut affinity = LegacyAffinityState::default();
    affinity.sticky.insert(
        "shell:/tmp/expired".into(),
        super::LegacyStickyAffinity {
            lane: 3,
            last_used: now
                .checked_sub(MCP_LEGACY_STICKY_IDLE_TTL + std::time::Duration::from_secs(1))
                .expect("test instant supports sticky TTL"),
        },
    );

    assert!(affinity.lane_is_pinned(3));
    affinity.prune_expired_sticky(now);
    assert!(!affinity.lane_is_pinned(3));
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

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde_json::{Value, json};
use url::Url;

use super::wire::HttpResponse;
use crate::mcp_server::auth::{AuthMode, AuthorizationRequest, OAuthRuntime, bearer_token};

pub(super) fn mcp_authorized(
    headers: &HashMap<String, String>,
    oauth: &OAuthRuntime,
    auth_mode: AuthMode,
    oauth_enabled: bool,
) -> Result<bool> {
    let header = headers.get("authorization").map(String::as_str);
    if auth_mode == AuthMode::None && !oauth_enabled {
        return Ok(true);
    }

    let Some(token) = bearer_token(header) else {
        return Ok(false);
    };

    // OAuth is additive to key authentication. In particular, selecting OAuth
    // as the primary mode must not disable an already configured access key.
    if matches!(auth_mode, AuthMode::Key | AuthMode::Oauth)
        && oauth.auth_store().verify_key(token)?
    {
        return Ok(true);
    }

    if oauth_enabled {
        oauth.verify_oauth_token(token)
    } else {
        Ok(false)
    }
}

pub(super) fn payload_contains_method(payload: &Value, method: &str) -> bool {
    let mut pending = vec![payload];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items.iter()),
            Value::Object(object)
                if object.get("method").and_then(Value::as_str) == Some(method) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

pub(super) fn ensure_oauth_mode(oauth: &OAuthRuntime) -> Result<()> {
    if !oauth.auth_store().status()?.oauth_enabled {
        bail!("OAuth authentication is not enabled for this MCP daemon");
    }
    Ok(())
}

pub(super) fn unauthorized_response(
    mode: AuthMode,
    oauth_enabled: bool,
    issuer: &Url,
) -> HttpResponse {
    let mut response = HttpResponse::json(401, json!({"error":"unauthorized"}));
    let challenge = if oauth_enabled || mode == AuthMode::Oauth {
        let metadata = issuer
            .join(".well-known/oauth-protected-resource")
            .expect("metadata URL");
        format!("Bearer resource_metadata=\"{}\", scope=\"mcp\"", metadata)
    } else {
        "Bearer".into()
    };
    response
        .headers
        .push(("WWW-Authenticate".into(), challenge));
    response
}

pub(super) fn attach_protocol_version(payload: &mut Value, version: &str) {
    if let Some(items) = payload.as_array_mut() {
        for item in items {
            attach_protocol_version(item, version);
        }
        return;
    }
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let params = object.entry("params").or_insert_with(|| json!({}));
    let Some(params) = params.as_object_mut() else {
        return;
    };
    let meta = params.entry("_meta").or_insert_with(|| json!({}));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    meta.entry("io.modelcontextprotocol/protocolVersion")
        .or_insert_with(|| json!(version));
}

pub(super) fn authorization_page(request: &AuthorizationRequest) -> String {
    let state = request.state.as_deref().unwrap_or("");
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Authorize Yeet MCP</title>\
<style>body{{font:16px system-ui;max-width:620px;margin:10vh auto;padding:24px}}input,button{{font:inherit;padding:10px;width:100%;box-sizing:border-box}}code{{overflow-wrap:anywhere}}</style>\
<h1>Authorize Yeet MCP</h1><p>Client: <code>{}</code></p><p>Resource: <code>{}</code></p>\
<form method=\"post\" action=\"/authorize\">\
<input type=\"hidden\" name=\"response_type\" value=\"code\">\
<input type=\"hidden\" name=\"client_id\" value=\"{}\">\
<input type=\"hidden\" name=\"redirect_uri\" value=\"{}\">\
<input type=\"hidden\" name=\"state\" value=\"{}\">\
<input type=\"hidden\" name=\"code_challenge\" value=\"{}\">\
<input type=\"hidden\" name=\"code_challenge_method\" value=\"S256\">\
<input type=\"hidden\" name=\"resource\" value=\"{}\">\
<input type=\"hidden\" name=\"scope\" value=\"{}\">\
<label>Authorization key<br><input autofocus type=\"password\" name=\"access_key\" autocomplete=\"current-password\" required></label><p><button type=\"submit\">Authorize</button></p></form>",
        html_escape(&request.client_id),
        html_escape(&request.resource),
        html_escape(&request.client_id),
        html_escape(&request.redirect_uri),
        html_escape(state),
        html_escape(&request.code_challenge),
        html_escape(&request.resource),
        html_escape(&request.scope),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

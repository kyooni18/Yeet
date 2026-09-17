use std::{
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path as AxumPath, Query, State, WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{any, get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AuthenticationFinishRequest, EnrollmentBeginRequest, FrameSnapshot, KeyLoginRequest,
    RegistrationFinishRequest, RemoteAuthRuntime, RemoteInput, WireInput,
    assets::{LEGACY_REMOTE_PAGE, WEBUI_AVAILABLE, webui_asset},
    protocol::{REMOTE_PROTOCOL_MAX_VERSION, REMOTE_PROTOCOL_MIN_VERSION, REMOTE_PROTOCOL_VERSION},
    websocket::{RemoteHub, serve_socket},
};

const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 256 * 1024;
pub(super) const MAX_ACTIVE_WEBSOCKETS: usize = 128;
const MAX_CONCURRENT_KEY_VERIFICATIONS: usize = 4;

#[derive(Clone)]
struct GatewayState {
    input_tx: Sender<RemoteInput>,
    frame: Arc<Mutex<FrameSnapshot>>,
    auth: Arc<RemoteAuthRuntime>,
    hub: Arc<RemoteHub>,
    websocket_slots: Arc<tokio::sync::Semaphore>,
    key_verification_slots: Arc<tokio::sync::Semaphore>,
}

#[derive(Debug, Default, Deserialize)]
struct AuthStatusQuery {
    enroll: Option<String>,
}

pub(crate) fn serve(
    listener: TcpListener,
    input_tx: Sender<RemoteInput>,
    frame: Arc<Mutex<FrameSnapshot>>,
    auth: Arc<RemoteAuthRuntime>,
    workspace: PathBuf,
    legacy_tui: bool,
    running: Arc<AtomicBool>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Yeet Remote failed to initialize async gateway: {error}");
            return;
        }
    };
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("Yeet Remote failed to adopt HTTP listener: {error}");
                return;
            }
        };
        let state = Arc::new(GatewayState {
            input_tx,
            frame,
            auth,
            hub: Arc::new(RemoteHub::new(workspace)),
            websocket_slots: Arc::new(tokio::sync::Semaphore::new(MAX_ACTIVE_WEBSOCKETS)),
            key_verification_slots: Arc::new(tokio::sync::Semaphore::new(
                MAX_CONCURRENT_KEY_VERIFICATIONS,
            )),
        });
        let app = router(state, legacy_tui);
        let shutdown = async move {
            while running.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        };
        let server = async { axum::serve(listener, app).await };
        tokio::select! {
            result = server => {
                if let Err(error) = result {
                    eprintln!("Yeet Remote HTTP/WebSocket gateway stopped: {error}");
                }
            }
            _ = shutdown => {
                // Drop the server future and then this local Tokio runtime. That
                // forcibly releases upgraded WebSockets too, so `remote stop`
                // cannot hang waiting for a browser that stayed connected.
            }
        }
    });
}

fn router(state: Arc<GatewayState>, legacy_tui: bool) -> Router {
    let router = Router::new()
        .route("/api/protocol", get(protocol_info))
        .route("/api/ws", get(websocket_upgrade))
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/key", post(auth_key))
        .route("/api/auth/passkey/begin", post(passkey_begin))
        .route("/api/auth/passkey/finish", post(passkey_finish))
        .route(
            "/api/auth/passkey/register/begin",
            post(passkey_register_begin),
        )
        .route(
            "/api/auth/passkey/register/finish",
            post(passkey_register_finish),
        )
        .route("/api/{*path}", any(api_not_found));
    let router = if legacy_tui {
        router
            .route("/", get(legacy_index))
            .route("/index.html", get(legacy_index))
            .route("/enroll", get(legacy_index))
            .route("/favicon.ico", get(favicon))
            .route("/api/frame", get(legacy_frame))
            .route("/api/input", post(legacy_input))
    } else {
        router
            .route("/", get(webui_index))
            .route("/index.html", get(webui_index))
            .route("/enroll", get(webui_index))
            .route("/{*path}", get(webui_path))
    };
    router
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(state)
}

async fn api_not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "not found")
}

async fn legacy_index() -> Response {
    let mut response = Html(LEGACY_REMOTE_PAGE).into_response();
    secure_headers(response.headers_mut(), false);
    response
}

async fn webui_index() -> Response {
    webui_asset_response("index.html", true)
}

async fn webui_path(AxumPath(path): AxumPath<String>) -> Response {
    let path = path.trim_start_matches('/');
    if path.starts_with("api/") {
        return text_response(StatusCode::NOT_FOUND, "not found");
    }
    if webui_asset(path).is_some() {
        return webui_asset_response(path, false);
    }
    if path.contains('.') {
        return text_response(StatusCode::NOT_FOUND, "not found");
    }
    webui_asset_response("index.html", true)
}

fn webui_asset_response(path: &str, index: bool) -> Response {
    if !WEBUI_AVAILABLE {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Yeet Remote WebUI assets are not embedded in this build; rebuild the WebUI before compiling Yeet",
        );
    }
    let Some(asset) = webui_asset(path) else {
        return text_response(StatusCode::NOT_FOUND, "not found");
    };
    let mut response = Response::new(Body::from(asset.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(asset.content_type),
    );
    secure_web_headers(response.headers_mut(), index, asset.immutable);
    response
}

async fn favicon() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    secure_headers(response.headers_mut(), true);
    response
}

async fn protocol_info() -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "version": REMOTE_PROTOCOL_VERSION,
            "minVersion": REMOTE_PROTOCOL_MIN_VERSION,
            "maxVersion": REMOTE_PROTOCOL_MAX_VERSION,
            "websocket": "/api/ws",
        }),
    )
}

async fn auth_status(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<AuthStatusQuery>,
    headers: HeaderMap,
) -> Response {
    let (key_enabled, passkey_enabled) = state.auth.methods();
    let enrollment_valid = query
        .enroll
        .as_deref()
        .is_some_and(|token| state.auth.enrollment_valid(token));
    json_response(
        StatusCode::OK,
        json!({
            "required": state.auth.required(),
            "authenticated": state.auth.is_authorized_headers(&headers),
            "key": key_enabled,
            "passkey": passkey_enabled,
            "enrollmentValid": enrollment_valid,
        }),
    )
}

async fn auth_key(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<KeyLoginRequest>,
) -> Response {
    if !state.auth.http_auth_origin_allowed(&headers) {
        return json_error(StatusCode::FORBIDDEN, "browser origin is not allowed");
    }

    // Argon2 verification is deliberately expensive. Keep it off Tokio's
    // request workers and bound the blocking work so unauthenticated bursts
    // cannot multiply Argon2 memory/CPU use without limit.
    let permit = match acquire_key_verification_slot(&state.key_verification_slots) {
        Ok(permit) => permit,
        Err(response) => return *response,
    };
    let auth = Arc::clone(&state.auth);
    let key = body.key;
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth.verify_access_key(&key)
    })
    .await
    {
        Ok(Ok(Some(session))) => session_response(&state.auth, &session),
        Ok(Ok(None)) => json_error(StatusCode::UNAUTHORIZED, "invalid access key"),
        Ok(Err(error)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        Err(error) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("access-key verification worker failed: {error}"),
        ),
    }
}

async fn passkey_begin(State(state): State<Arc<GatewayState>>, headers: HeaderMap) -> Response {
    if !state.auth.http_auth_origin_allowed(&headers) {
        return json_error(StatusCode::FORBIDDEN, "browser origin is not allowed");
    }
    match state.auth.begin_passkey_authentication() {
        Ok(challenge) => json_response(StatusCode::OK, challenge),
        Err(error) => json_error(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn passkey_finish(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<AuthenticationFinishRequest>,
) -> Response {
    if !state.auth.http_auth_origin_allowed(&headers) {
        return json_error(StatusCode::FORBIDDEN, "browser origin is not allowed");
    }
    match state.auth.finish_passkey_authentication(
        &body.transaction,
        &body.credential,
        body.enrollment_token.as_deref(),
    ) {
        Ok(session) => session_response(&state.auth, &session),
        Err(error) => json_error(StatusCode::UNAUTHORIZED, &error.to_string()),
    }
}

async fn passkey_register_begin(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<EnrollmentBeginRequest>,
) -> Response {
    if !state.auth.http_auth_origin_allowed(&headers) {
        return json_error(StatusCode::FORBIDDEN, "browser origin is not allowed");
    }
    match state.auth.begin_passkey_registration(&body.token) {
        Ok(challenge) => json_response(StatusCode::OK, challenge),
        Err(error) => json_error(StatusCode::UNAUTHORIZED, &error.to_string()),
    }
}

async fn passkey_register_finish(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<RegistrationFinishRequest>,
) -> Response {
    if !state.auth.http_auth_origin_allowed(&headers) {
        return json_error(StatusCode::FORBIDDEN, "browser origin is not allowed");
    }
    match state
        .auth
        .finish_passkey_registration(&body.transaction, &body.credential)
    {
        Ok(session) => session_response(&state.auth, &session),
        Err(error) => json_error(StatusCode::UNAUTHORIZED, &error.to_string()),
    }
}

async fn websocket_upgrade(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.auth.is_authorized_headers(&headers) {
        return json_error(StatusCode::UNAUTHORIZED, "authentication required");
    }
    if !state.auth.websocket_origin_allowed(&headers) {
        return json_error(StatusCode::FORBIDDEN, "WebSocket origin is not allowed");
    }
    let permit = match acquire_websocket_slot(&state.websocket_slots) {
        Ok(permit) => permit,
        Err(response) => return *response,
    };
    let hub = Arc::clone(&state.hub);
    let mut response = ws
        .protocols(["yeet.remote.v1"])
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            serve_socket(socket, hub).await;
        })
        .into_response();
    secure_headers(response.headers_mut(), true);
    response
}

fn acquire_websocket_slot(
    slots: &Arc<tokio::sync::Semaphore>,
) -> std::result::Result<tokio::sync::OwnedSemaphorePermit, Box<Response>> {
    match Arc::clone(slots).try_acquire_owned() {
        Ok(permit) => Ok(permit),
        Err(_) => {
            let mut response = json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Remote WebSocket capacity reached; retry shortly",
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            Err(Box::new(response))
        }
    }
}

fn acquire_key_verification_slot(
    slots: &Arc<tokio::sync::Semaphore>,
) -> std::result::Result<tokio::sync::OwnedSemaphorePermit, Box<Response>> {
    match Arc::clone(slots).try_acquire_owned() {
        Ok(permit) => Ok(permit),
        Err(_) => {
            let mut response = json_error(
                StatusCode::TOO_MANY_REQUESTS,
                "Remote access-key verification capacity reached; retry shortly",
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            Err(Box::new(response))
        }
    }
}

async fn legacy_frame(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !state.auth.is_authorized_headers(&headers) {
        return json_error(StatusCode::UNAUTHORIZED, "authentication required");
    }
    let after = query
        .get("after")
        .and_then(|value| value.parse::<u64>().ok());
    let Some(snapshot) = state.frame.lock().ok().map(|value| value.clone()) else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "frame unavailable");
    };
    if after == Some(snapshot.sequence) {
        let mut response = StatusCode::NO_CONTENT.into_response();
        secure_headers(response.headers_mut(), true);
        return response;
    }
    json_response(
        StatusCode::OK,
        json!({
            "sequence": snapshot.sequence,
            "width": snapshot.width,
            "height": snapshot.height,
            "html": snapshot.html,
        }),
    )
}

async fn legacy_input(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(wire): Json<WireInput>,
) -> Response {
    if !state.auth.is_authorized_headers(&headers) {
        return json_error(StatusCode::UNAUTHORIZED, "authentication required");
    }
    if headers
        .get("x-yeet-remote")
        .and_then(|value| value.to_str().ok())
        != Some("1")
    {
        return text_response(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Some(input) = wire.into_remote_input() {
        let _ = state.input_tx.send(input);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    secure_headers(response.headers_mut(), true);
    response
}

fn session_response(auth: &RemoteAuthRuntime, session: &str) -> Response {
    let mut response = json_response(StatusCode::OK, json!({"ok": true}));
    if let Ok(cookie) = HeaderValue::from_str(&auth.session_cookie(session)) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

fn json_error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({"error": message}))
}

fn json_response(status: StatusCode, value: Value) -> Response {
    let mut response = (status, Json(value)).into_response();
    secure_headers(response.headers_mut(), true);
    response
}

fn text_response(status: StatusCode, text: &str) -> Response {
    let mut response = (status, text.to_owned()).into_response();
    secure_headers(response.headers_mut(), true);
    response
}

fn secure_headers(headers: &mut HeaderMap, api: bool) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if !api {
        headers.insert(
            "content-security-policy",
            HeaderValue::from_static(
                "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'",
            ),
        );
    }
}

fn secure_web_headers(headers: &mut HeaderMap, index: bool, immutable: bool) {
    headers.insert(
        header::CACHE_CONTROL,
        if immutable && !index {
            HeaderValue::from_static("public, max-age=31536000, immutable")
        } else {
            HeaderValue::from_static("no-store")
        },
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if index {
        headers.insert(
            "content-security-policy",
            HeaderValue::from_static(
                "default-src 'self'; connect-src 'self' ws: wss: https://cloudflareinsights.com; img-src 'self' data:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' https://static.cloudflareinsights.com/beacon.min.js; object-src 'none'; frame-ancestors 'none'; base-uri 'none'"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_slot_admission_is_bounded_and_recovers() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = acquire_websocket_slot(&slots).expect("first socket slot");

        let response = match acquire_websocket_slot(&slots) {
            Ok(_) => panic!("saturated WebSocket slots unexpectedly admitted another connection"),
            Err(response) => response,
        };
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from_static("1"))
        );

        drop(permit);
        assert!(acquire_websocket_slot(&slots).is_ok());
    }

    #[test]
    fn key_verification_admission_is_bounded_and_recovers() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = acquire_key_verification_slot(&slots).expect("first verification slot");

        let response = match acquire_key_verification_slot(&slots) {
            Ok(_) => panic!("saturated key-verification slots unexpectedly admitted more work"),
            Err(response) => response,
        };
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from_static("1"))
        );

        drop(permit);
        assert!(acquire_key_verification_slot(&slots).is_ok());
    }
}

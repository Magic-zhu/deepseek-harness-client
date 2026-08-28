//! dsh-bridge — the dsh daemon /api wire envelope, client side.
//!
//! One job: build a `client-request`, POST it, validate the
//! `server-response`, and fold business errors into [`ApiError::Rpc`].
//! Loopback plain HTTP only; the daemon's own trust fence pins the rest.

use std::time::Duration;

use serde_json::{json, Value};

/// Unary timeout, mirroring upstream `AbstractApiClient`.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum ApiError {
    /// Connect failure, timeout, or a non-2xx status (transport only; HTTP
    /// status never carries business meaning in this envelope).
    Transport(String),
    /// Business error from the daemon: code/message/details verbatim.
    Rpc { code: String, message: String, details: Value },
    /// Malformed envelope, bad method shape, or rpcId echo mismatch.
    Protocol(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Transport(msg) => write!(f, "传输失败：{msg}"),
            ApiError::Rpc { code, message, .. } => write!(f, "[{code}] {message}"),
            ApiError::Protocol(msg) => write!(f, "协议错误：{msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// One method-name segment: ASCII alnum plus `-`/`_`, nonempty.
fn valid_segment(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Method shape whitelist: `<domain>.<name>` (static method) or
/// `<ns>/<name>` (Typert remote). Rejects path characters so a webview can
/// never smuggle a URL into the POST path.
pub fn validate_method(method: &str) -> Result<(), ApiError> {
    let bad = || ApiError::Protocol(format!("非法 method 形状：{method:?}"));
    if let Some((ns, name)) = method.split_once('/') {
        return if valid_segment(ns) && valid_segment(name) { Ok(()) } else { Err(bad()) };
    }
    if let Some((domain, name)) = method.split_once('.') {
        return if valid_segment(domain) && valid_segment(name) { Ok(()) } else { Err(bad()) };
    }
    Err(bad())
}

/// Typert remotes require the payload wrapped as exactly `{ "args": {...} }`;
/// static methods take the payload verbatim. The frontend never writes the
/// `args` shell itself.
pub fn wrap_payload(method: &str, payload: Value) -> Value {
    if method.contains('/') { json!({ "args": payload }) } else { payload }
}

/// Build the wire body; the minted rpcId is returned for echo validation.
pub fn build_body(method: &str, payload: Value) -> (String, Value) {
    let rpc_id = uuid::Uuid::new_v4().to_string();
    let body = json!({
        "type": "client-request",
        "rpcId": rpc_id,
        "method": method,
        "payload": wrap_payload(method, payload),
    });
    (rpc_id, body)
}

/// Validate a raw response body: envelope shape, rpcId echo, result fold.
/// `void` results have no `value` field and surface as `Value::Null`.
pub fn parse_response(body: &Value, expected_rpc_id: &str) -> Result<Value, ApiError> {
    if body.get("type").and_then(Value::as_str) != Some("server-response") {
        return Err(ApiError::Protocol(format!("响应 type 非 server-response：{body}")));
    }
    let echoed = body.get("rpcId").and_then(Value::as_str).unwrap_or_default();
    if echoed != expected_rpc_id {
        return Err(ApiError::Protocol(format!("rpcId 回声不符：期望 {expected_rpc_id}，收到 {echoed:?}")));
    }
    let result = body.get("result").ok_or_else(|| ApiError::Protocol("响应缺 result 字段".into()))?;
    match result.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(result.get("value").cloned().unwrap_or(Value::Null)),
        Some(false) => {
            let error = result.get("error").cloned().unwrap_or(Value::Null);
            let code = error.get("code").and_then(Value::as_str).unwrap_or("internal").to_string();
            let message = error.get("message").and_then(Value::as_str).unwrap_or("（无 message）").to_string();
            let details = error.get("details").cloned().unwrap_or(Value::Null);
            Err(ApiError::Rpc { code, message, details })
        }
        _ => Err(ApiError::Protocol(format!("result.ok 缺失或非布尔：{result}"))),
    }
}

/// Loopback unary client. One client per call site is fine at management-UI
/// call rates; the port is re-read per call because a restarted daemon gets
/// a fresh one.
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
}

impl ApiClient {
    pub fn new(port: u16) -> Self {
        let http = reqwest::Client::builder()
            .timeout(CALL_TIMEOUT)
            .build()
            .expect("reqwest client build");
        Self { http, base: format!("http://127.0.0.1:{port}") }
    }

    pub async fn call(&self, method: &str, payload: Value) -> Result<Value, ApiError> {
        validate_method(method)?;
        let (rpc_id, body) = build_body(method, payload);
        let url = format!("{}/api/{method}", self.base);
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ApiError::Transport(format!("HTTP {}", response.status())));
        }
        let raw: Value = response.json().await.map_err(|e| ApiError::Protocol(e.to_string()))?;
        parse_response(&raw, &rpc_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_method_accepts_static_and_remote() {
        assert!(validate_method("settings.describe").is_ok());
        assert!(validate_method("pluginInventory/list").is_ok());
        assert!(validate_method("dynamicCordisRunner/stopFromPanel").is_ok());
    }

    #[test]
    fn validate_method_rejects_path_smuggling() {
        for bad in ["", "foo", "a//b", "a/b/c", "a.b.c", "../etc", "a b/c", "a?b/c", "/list", "x/"] {
            assert!(validate_method(bad).is_err(), "应拒绝 {bad:?}");
        }
    }

    #[test]
    fn wrap_payload_only_wraps_remote_methods() {
        assert_eq!(wrap_payload("pluginInventory/list", json!({})), json!({ "args": {} }));
        assert_eq!(
            wrap_payload("dynamicCordisRunner/stopFromPanel", json!({ "pluginId": "p" })),
            json!({ "args": { "pluginId": "p" } }),
        );
        let plain = json!({ "ns": "x" });
        assert_eq!(wrap_payload("settings.update", plain.clone()), plain);
    }

    #[test]
    fn build_body_mints_rpc_id_and_envelope() {
        let (rpc_id, body) = build_body("settings.describe", json!({}));
        assert!(!rpc_id.is_empty());
        assert_eq!(body["type"], "client-request");
        assert_eq!(body["rpcId"], rpc_id);
        assert_eq!(body["method"], "settings.describe");
        assert_eq!(body["payload"], json!({}));
    }

    #[test]
    fn parse_response_ok_with_value() {
        let body = json!({
            "type": "server-response", "rpcId": "r1",
            "result": { "ok": true, "value": { "entries": [] } },
        });
        assert_eq!(parse_response(&body, "r1").unwrap(), json!({ "entries": [] }));
    }

    #[test]
    fn parse_response_ok_void_has_no_value() {
        // Typert void 结果的响应没有 value 字段。
        let body = json!({
            "type": "server-response", "rpcId": "r1",
            "result": { "ok": true },
        });
        assert_eq!(parse_response(&body, "r1").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn parse_response_business_error_keeps_code_message_details() {
        let body = json!({
            "type": "server-response", "rpcId": "r1",
            "result": { "ok": false, "error": {
                "code": "settings-conflict", "message": "revision 过期",
                "details": { "ns": "a", "expected": 1, "actual": 2 },
            } },
        });
        match parse_response(&body, "r1") {
            Err(ApiError::Rpc { code, message, details }) => {
                assert_eq!(code, "settings-conflict");
                assert_eq!(message, "revision 过期");
                assert_eq!(details["actual"], 2);
            }
            other => panic!("应为 Rpc 错误，得到 {other:?}"),
        }
    }

    #[test]
    fn parse_response_rejects_rpc_id_mismatch_and_bad_type() {
        let body = json!({ "type": "server-response", "rpcId": "r2", "result": { "ok": true } });
        assert!(matches!(parse_response(&body, "r1"), Err(ApiError::Protocol(_))));
        let not_response = json!({ "type": "server-request", "rpcId": "r1" });
        assert!(matches!(parse_response(&not_response, "r1"), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn api_error_display_formats() {
        let rpc = ApiError::Rpc { code: "settings-conflict".into(), message: "过期".into(), details: json!({}) };
        assert_eq!(rpc.to_string(), "[settings-conflict] 过期");
        assert_eq!(ApiError::Transport("超时".into()).to_string(), "传输失败：超时");
    }

    /// Loopback stub server: one request in, fixed envelope out (rpcId echoed
    /// from the request). Run on demand with `-- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "本地假服务集成测试，按需跑：cargo test -p dsh-bridge -- --ignored"]
    async fn round_trip_against_stub_server() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let body_text = request.split("\r\n\r\n").nth(1).unwrap_or("{}").to_string();
            let rpc_id = serde_json::from_str::<serde_json::Value>(&body_text)
                .ok()
                .and_then(|v| v["rpcId"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let response_body = serde_json::json!({
                "type": "server-response", "rpcId": rpc_id,
                "result": { "ok": true, "value": { "pong": true } },
            })
            .to_string();
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(response_body.as_bytes()).unwrap();
            tx.send(request).unwrap();
        });

        let client = ApiClient::new(port);
        let value = client.call("pluginInventory/list", json!({})).await.unwrap();
        assert_eq!(value, json!({ "pong": true }));

        let request = rx.recv().unwrap();
        assert!(request.starts_with("POST /api/pluginInventory/list HTTP/1.1"), "{request}");
        assert!(request.contains(r#""payload":{"args":{}}"#), "Remote 方法应自动包 args 壳：{request}");
    }
}

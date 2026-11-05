use serde_json::{json, Value};

/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug, PartialEq)]
pub enum RoutedMessage {
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A request the App Server sends to us (approvals, auth refreshes). Its
    /// id comes from the server's own counter, so it can equal the id of one
    /// of our pending requests and must never be taken for that response.
    Request {
        id: Value,
        method: String,
    },
    Unexpected,
}

pub fn route_value(value: Value) -> RoutedMessage {
    // A method marks a message from the server's side of the conversation;
    // only a message without one can answer something we sent.
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        if let Some(id) = value.get("id").filter(|id| !id.is_null()) {
            return RoutedMessage::Request {
                id: id.clone(),
                method: method.to_owned(),
            };
        }
        return RoutedMessage::Notification {
            method: method.to_owned(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        };
    }
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        if let Some(error) = value.get("error") {
            return RoutedMessage::Response {
                id,
                result: Err(error.to_string()),
            };
        }
        return RoutedMessage::Response {
            id,
            result: Ok(value.get("result").cloned().unwrap_or(Value::Null)),
        };
    }
    RoutedMessage::Unexpected
}

/// The answer to a server request UsageBar does not handle. UsageBar starts
/// no threads, so nothing should ask, but an unanswered request would leave
/// the server waiting on it.
pub fn unsupported_request_reply(id: Value, method: &str) -> Value {
    json!({
        "id": id,
        "error": {
            "code": METHOD_NOT_FOUND,
            "message": format!("UsageBar does not handle {method}"),
        }
    })
}

pub fn route_line(line: &str) -> Result<RoutedMessage, serde_json::Error> {
    serde_json::from_str(line).map(route_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_success_error_and_notification_separately() {
        assert_eq!(
            route_line(r#"{"id":7,"result":{"ok":true}}"#).unwrap(),
            RoutedMessage::Response {
                id: 7,
                result: Ok(serde_json::json!({"ok": true})),
            }
        );
        assert!(matches!(
            route_line(r#"{"id":8,"error":{"message":"nope"}}"#).unwrap(),
            RoutedMessage::Response {
                id: 8,
                result: Err(_)
            }
        ));
        assert_eq!(
            route_line(r#"{"method":"account/rateLimits/updated","params":{"x":1}}"#).unwrap(),
            RoutedMessage::Notification {
                method: "account/rateLimits/updated".into(),
                params: serde_json::json!({"x": 1}),
            }
        );
    }

    #[test]
    fn a_server_request_is_never_taken_for_a_response() {
        // The server numbers its own requests, so id 1 here is not the
        // answer to our request 1. It used to resolve that request with
        // `null`, which read as "signed out" for account/read.
        assert_eq!(
            route_line(
                r#"{"id":1,"method":"item/commandExecution/requestApproval","params":{"x":1}}"#
            )
            .unwrap(),
            RoutedMessage::Request {
                id: serde_json::json!(1),
                method: "item/commandExecution/requestApproval".into(),
            }
        );
        assert_eq!(
            route_line(r#"{"id":"srv-2","method":"account/chatgptAuthTokens/refresh"}"#).unwrap(),
            RoutedMessage::Request {
                id: serde_json::json!("srv-2"),
                method: "account/chatgptAuthTokens/refresh".into(),
            }
        );
        // A null id is a notification in all but name.
        assert!(matches!(
            route_line(r#"{"id":null,"method":"account/updated"}"#).unwrap(),
            RoutedMessage::Notification { .. }
        ));
        let reply = unsupported_request_reply(serde_json::json!("srv-2"), "x/y");
        assert_eq!(reply["id"], serde_json::json!("srv-2"));
        assert_eq!(reply["error"]["code"], serde_json::json!(METHOD_NOT_FOUND));
        assert!(reply.get("result").is_none());
    }

    #[test]
    fn malformed_or_partial_messages_do_not_panic() {
        assert!(route_line("not json").is_err());
        assert_eq!(route_line("{}").unwrap(), RoutedMessage::Unexpected);
    }
}

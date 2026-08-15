use serde_json::Value;

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
    Unexpected,
}

pub fn route_value(value: Value) -> RoutedMessage {
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
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        return RoutedMessage::Notification {
            method: method.to_owned(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        };
    }
    RoutedMessage::Unexpected
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
    fn malformed_or_partial_messages_do_not_panic() {
        assert!(route_line("not json").is_err());
        assert_eq!(route_line("{}").unwrap(), RoutedMessage::Unexpected);
    }
}

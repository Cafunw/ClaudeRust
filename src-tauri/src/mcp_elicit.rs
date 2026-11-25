//! Elicitation (MCP `elicitation/create`): a server asks the user for a few
//! values in a form, or (URL mode, MCP 2025-11-25) to open a page of its in
//! the browser. Each request becomes a card in the UI (event `mcp-elicit`);
//! the user's answer, decline or cancel comes back through
//! `mcp_elicit_respond`. Unanswered for five minutes, a request is declined
//! (and `mcp-elicit-done` takes its card away).

use crate::error::{AppError, AppResult};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::oneshot;

const WAIT: Duration = Duration::from_secs(300);

/// Requests waiting for the user: id → (its card, where the answer goes).
type Waiting = HashMap<String, (Value, oneshot::Sender<Value>)>;
static WAITING: LazyLock<Mutex<Waiting>> = LazyLock::new(Default::default);

/// Set once the UI has loaded (see `mcp_elicit_pending`).
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

fn emit(event: &str, payload: &Value) {
    if let Some(app) = APP.get() {
        let _ = app.emit(event, payload);
    }
}

/// The values field `p` may take when it is a choice: its `enum`, or the
/// `const`s of its `oneOf`/`anyOf` (titled choices); empty when it isn't one.
fn choices(p: &Value) -> Vec<Value> {
    if let Some(e) = p["enum"].as_array() {
        return e.clone();
    }
    let titled = p["oneOf"].as_array().or(p["anyOf"].as_array());
    titled.into_iter().flatten().filter_map(|c| c.get("const").cloned()).collect()
}

/// The card for request `params`, or `None` when it can't be shown: a URL
/// that isn't a web page's, or a schema that isn't a flat object of strings
/// (single choices among them), numbers, integers, booleans and lists of
/// choices. A URL card names the URL's host apart, as it is spelled in the
/// URL (an internationalized one in its xn-- form, `punycode` then set).
fn card(id: &str, server: &str, params: &Value) -> Option<Value> {
    if params["mode"] == "url" {
        let url = url::Url::parse(params["url"].as_str()?).ok().filter(|u| matches!(u.scheme(), "https" | "http"))?;
        let host = url.host_str()?.to_string();
        let punycode = host.split('.').any(|l| l.starts_with("xn--"));
        return Some(json!({"id": id, "server": server, "mode": "url", "message": params["message"], "url": url.as_str(), "host": host, "punycode": punycode}));
    }
    let schema = &params["requestedSchema"];
    let flat = schema["type"] == "object"
        && schema["properties"].as_object()?.values().all(|p| match p["type"].as_str() {
            Some("string" | "number" | "integer" | "boolean") => true,
            Some("array") => !choices(&p["items"]).is_empty(),
            _ => false,
        });
    let form = params["mode"].as_str().is_none_or(|m| m == "form");
    (flat && form).then(|| json!({"id": id, "server": server, "message": params["message"], "schema": schema}))
}

/// `content` checked against the form's `schema`: only its fields, each of
/// its type (and one of its values, for a choice; a list of them within its
/// bounds, for a list), every required one there.
fn checked(schema: &Value, content: Value) -> AppResult<Value> {
    let (Some(props), Some(given)) = (schema["properties"].as_object(), content.as_object()) else {
        return Err(AppError::msg("The answer must be a set of fields."));
    };
    for (k, v) in given {
        let p = props.get(k).ok_or_else(|| AppError::msg(format!("\"{k}\" isn't a field of this form.")))?;
        let typed = match p["type"].as_str() {
            Some("string") => v.is_string(),
            Some("number") => v.is_number(),
            Some("integer") => v.is_i64() || v.is_u64(),
            Some("boolean") => v.is_boolean(),
            Some("array") => v.as_array().is_some_and(|a| {
                let (n, can) = (a.len() as u64, choices(&p["items"]));
                a.iter().all(|x| can.contains(x)) && p["minItems"].as_u64().is_none_or(|m| n >= m) && p["maxItems"].as_u64().is_none_or(|m| n <= m)
            }),
            _ => false,
        };
        let can = choices(p);
        if !typed || (!can.is_empty() && !can.contains(v)) {
            return Err(AppError::msg(format!("\"{k}\" doesn't take {v}.")));
        }
    }
    let mut required = schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str);
    if let Some(missing) = required.find(|r| !given.contains_key(*r)) {
        return Err(AppError::msg(format!("\"{missing}\" is required.")));
    }
    Ok(content)
}

/// Ask the user what server `server` wants (`elicitation/create` params);
/// the result to send back.
pub(super) async fn ask(server: &str, params: &Value) -> Value {
    ask_within(server, params, WAIT).await
}

async fn ask_within(server: &str, params: &Value, wait: Duration) -> Value {
    let id = crate::util::new_id();
    let Some(card) = card(&id, server, params) else { return json!({"action": "decline"}) };
    let (tx, rx) = oneshot::channel();
    {
        let mut waiting = WAITING.lock();
        // one question per server at a time: a server can't stack dialogs up over the app
        if waiting.values().any(|(c, _)| c["server"] == server) {
            return json!({"action": "decline"});
        }
        waiting.insert(id.clone(), (card.clone(), tx));
    }
    emit("mcp-elicit", &card);
    let answer = tokio::time::timeout(wait, rx).await;
    WAITING.lock().remove(&id);
    emit("mcp-elicit-done", &json!({"id": id}));
    match answer {
        Ok(Ok(a)) => a,
        _ => json!({"action": "decline"}),
    }
}

/// Answer request `id`: `accept` with `content`, `decline` or `cancel`.
pub(super) fn respond(id: &str, action: &str, content: Value) -> AppResult<()> {
    let mut waiting = WAITING.lock();
    let Some((card, _)) = waiting.get(id) else {
        return Err(AppError::msg("This request was already answered, or it expired."));
    };
    let answer = match action {
        // the user opened the page (the UI does, on their click): nothing else to send
        "accept" if card["mode"] == "url" => json!({"action": "accept"}),
        "accept" => json!({"action": "accept", "content": checked(&card["schema"], content)?}),
        "decline" | "cancel" => json!({"action": action}),
        _ => return Err(AppError::msg(format!("Unknown answer: {action}"))),
    };
    if let Some((_, tx)) = waiting.remove(id) {
        let _ = tx.send(answer);
    }
    Ok(())
}

/// The requests waiting for an answer, for a UI that has just loaded; this
/// also gives the backend the handle it shows new ones with.
#[tauri::command]
pub fn mcp_elicit_pending(app: tauri::AppHandle) -> Vec<Value> {
    let _ = APP.set(app);
    WAITING.lock().values().map(|(card, _)| card.clone()).collect()
}

#[tauri::command]
pub fn mcp_elicit_respond(id: String, action: String, content: Option<Value>) -> AppResult<()> {
    respond(&id, &action, content.unwrap_or(Value::Null))
}

/// The id of a request from `server` that waits for the user.
#[cfg(test)]
pub(super) fn waiting_for(server: &str) -> Option<String> {
    WAITING.lock().iter().find(|(_, (card, _))| card["server"] == server).map(|(id, _)| id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Value {
        json!({"message": "Your details", "requestedSchema": {"type": "object", "properties": {
            "name": {"type": "string", "title": "Name"},
            "age": {"type": "integer", "minimum": 0},
            "score": {"type": "number"},
            "subscribe": {"type": "boolean"},
            "plan": {"type": "string", "enum": ["free", "pro"]},
        }, "required": ["name"]}})
    }

    #[test]
    fn only_flat_forms_are_shown() {
        let c = card("1", "srv", &form()).unwrap();
        assert_eq!((c["server"].as_str(), c["message"].as_str()), (Some("srv"), Some("Your details")));
        assert_eq!(c["schema"]["required"], json!(["name"]));
        let mut f = form();
        f["mode"] = json!("form");
        assert!(card("1", "srv", &f).is_some());
        // a nested field, a list of free text, no schema: declined
        let mut nested = form();
        nested["requestedSchema"]["properties"]["address"] = json!({"type": "object", "properties": {}});
        assert!(card("1", "srv", &nested).is_none());
        let mut list = form();
        list["requestedSchema"]["properties"]["tags"] = json!({"type": "array", "items": {"type": "string"}});
        assert!(card("1", "srv", &list).is_none());
        assert!(card("1", "srv", &json!({"message": "?"})).is_none());
    }

    #[test]
    fn choices_titled_or_not_and_lists_of_them() {
        let f = json!({"message": "Pick", "requestedSchema": {"type": "object", "properties": {
            "size": {"type": "string", "oneOf": [{"const": "s", "title": "Small"}, {"const": "l", "title": "Large"}]},
            "tags": {"type": "array", "items": {"type": "string", "enum": ["a", "b", "c"]}, "minItems": 1, "maxItems": 2},
            "team": {"type": "array", "items": {"anyOf": [{"const": "x", "title": "X"}, {"const": "y", "title": "Y"}]}},
        }}});
        let s = &card("1", "srv", &f).unwrap()["schema"];
        let ok = json!({"size": "l", "tags": ["a", "c"], "team": ["y"]});
        assert_eq!(checked(s, ok.clone()).unwrap(), ok);
        for bad in [
            json!({"size": "m"}),                // not a choice
            json!({"tags": []}),                 // fewer than minItems
            json!({"tags": ["a", "b", "c"]}),    // more than maxItems
            json!({"tags": ["d"]}),              // not a choice
            json!({"team": "x"}),                // not a list
        ] {
            assert!(checked(s, bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_url_card_names_its_host_and_only_web_pages_are_shown() {
        let url = |u: &str| json!({"mode": "url", "url": u, "message": "Connect your account", "elicitationId": "e1"});
        let c = card("1", "srv", &url("https://auth.example.com/connect?x=1")).unwrap();
        assert_eq!((c["mode"].as_str(), c["host"].as_str(), c["url"].as_str()), (Some("url"), Some("auth.example.com"), Some("https://auth.example.com/connect?x=1")));
        assert_eq!(c["punycode"], false);
        // an internationalized host is shown as it is spelled to the browser, and flagged
        let c = card("1", "srv", &url("https://аpple.com/")).unwrap();
        assert!(c["host"].as_str().unwrap().starts_with("xn--"), "{c}");
        assert_eq!(c["punycode"], true);
        for bad in ["file:///C:/Windows/system32", "javascript:alert(1)", "not a url", "clauderust://settings"] {
            assert!(card("1", "srv", &url(bad)).is_none(), "{bad}");
        }
        assert!(card("1", "srv", &json!({"mode": "url"})).is_none());
    }

    #[test]
    fn answers_are_checked_against_the_form() {
        let s = &form()["requestedSchema"];
        let ok = json!({"name": "Ada", "age": 36, "score": 1.5, "subscribe": true, "plan": "pro"});
        assert_eq!(checked(s, ok.clone()).unwrap(), ok);
        assert!(checked(s, json!({"name": "Ada"})).is_ok());
        for bad in [
            json!({"age": 3}),                    // name is required
            json!({"name": "Ada", "age": 3.5}),   // not an integer
            json!({"name": 7}),                   // not a string
            json!({"name": "Ada", "plan": "gold"}), // not one of the choices
            json!({"name": "Ada", "extra": 1}),   // not a field
            json!("Ada"),
        ] {
            assert!(checked(s, bad.clone()).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn the_user_answers_declines_or_lets_it_expire() {
        let answer = |server: &'static str, action: &'static str, content: Value| async move {
            let asked = tokio::spawn(async move { ask(server, &form()).await });
            let id = loop {
                if let Some(id) = waiting_for(server) {
                    break id;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            // a wrong answer leaves the request waiting
            assert!(respond(&id, "accept", json!({"name": 1})).is_err());
            assert!(respond(&id, "maybe", Value::Null).is_err());
            respond(&id, action, content).unwrap();
            assert!(respond(&id, "decline", Value::Null).is_err(), "answered twice");
            asked.await.unwrap()
        };
        assert_eq!(answer("e-accept", "accept", json!({"name": "Ada"})).await, json!({"action": "accept", "content": {"name": "Ada"}}));
        assert_eq!(answer("e-decline", "decline", Value::Null).await, json!({"action": "decline"}));
        assert_eq!(answer("e-cancel", "cancel", Value::Null).await, json!({"action": "cancel"}));
        assert_eq!(ask_within("e-late", &form(), Duration::from_millis(20)).await, json!({"action": "decline"}));
        assert_eq!(waiting_for("e-late"), None);
        // nothing to show: declined at once
        assert_eq!(ask("e-file", &json!({"mode": "url", "url": "file:///etc/passwd", "message": "Open"})).await, json!({"action": "decline"}));
        // a page the user opened: accepted, with nothing more (whatever content the UI sent)
        let asked = tokio::spawn(async { ask("e-url", &json!({"mode": "url", "url": "https://x.example/a", "message": "Open", "elicitationId": "1"})).await });
        let id = loop {
            if let Some(id) = waiting_for("e-url") {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        respond(&id, "accept", json!({"x": 1})).unwrap();
        assert_eq!(asked.await.unwrap(), json!({"action": "accept"}));
    }

    #[tokio::test]
    async fn a_server_asks_one_thing_at_a_time() {
        let first = tokio::spawn(async { ask("e-busy", &form()).await });
        let id = loop {
            if let Some(id) = waiting_for("e-busy") {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        // while the first waits, more from the same server are declined without a dialog
        assert_eq!(ask_within("e-busy", &form(), Duration::from_secs(60)).await, json!({"action": "decline"}));
        assert_eq!(WAITING.lock().values().filter(|(c, _)| c["server"] == "e-busy").count(), 1);
        respond(&id, "cancel", Value::Null).unwrap();
        assert_eq!(first.await.unwrap(), json!({"action": "cancel"}));
    }
}

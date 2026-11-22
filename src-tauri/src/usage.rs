//! The Claude plan's usage limits (the 5-hour session and the weekly windows) as the account engine's CLI
//! reports them: its `rate_limit_event` lines, and the limit a refused request names. Kept in memory only:
//! known after the first answer on the account in this run of the app.

use crate::state::AppState;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Runtime};

/// The limit information a CLI line carries: a `rate_limit_event`'s, or the one a refused request names
/// (`api_error_params`).
pub fn info_of(ev: &Value) -> Option<&Value> {
    let info = if ev["type"] == "rate_limit_event" { &ev["rate_limit_info"] } else { &ev["api_error_params"]["rate_limit_info"] };
    info.is_object().then_some(info)
}

/// Keeps what `info` says and tells the window (`usage-limits-changed`, the `usage_limits` payload).
pub fn record<R: Runtime>(app: &AppHandle<R>, st: &AppState, info: &Value) {
    let next = {
        let mut cur = st.usage_limits.lock();
        *cur = merged(&cur, info, crate::util::now_ms());
        cur.clone()
    };
    let _ = app.emit("usage-limits-changed", &next);
}

fn past_warning(status: &Value) -> bool {
    matches!(status.as_str(), Some("allowed_warning" | "rejected"))
}

/// One window as the CLI reports it (utilisation as a fraction, reset in epoch seconds) in the payload's terms.
fn window(key: &str, w: &Value) -> Value {
    json!({
        "key": key,
        "utilization": w["utilization"].as_f64(),
        "resets_at": w["resetsAt"].as_i64().map(|s| s.saturating_mul(1000)),
        "status": w["status"].as_str(),
        "threshold": w["surpassedThreshold"].as_f64(),
    })
}

/// The payload after `info`: the windows it reports (`limits`, the 5-hour one first, then the weekly ones)
/// replace the ones known, the others stay until they reset. `status` is the server's for the limit that
/// applies now (`limiting`): `exceeded` when it refuses requests, else `near` when that or any window is past
/// a warning threshold or at its limit.
fn merged(prev: &Value, info: &Value, now: i64) -> Value {
    let mut windows: Map<String, Value> = prev["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["resets_at"].as_i64().is_none_or(|t| t > now))
        .filter_map(|w| Some((w["key"].as_str()?.to_string(), w.clone())))
        .collect();
    for (k, w) in info["unifiedWindows"].as_object().into_iter().flatten().filter(|(_, w)| w.is_object()) {
        windows.insert(k.clone(), window(k, w));
    }
    // the top-level fields describe the limiting window (overage is shown on its own)
    let limiting = info["rateLimitType"].as_str();
    if let Some(k) = limiting.filter(|k| *k != "overage") {
        let mut w = window(k, info);
        w["status"] = info["status"].clone();
        if w["utilization"].is_null() {
            w["utilization"] = windows.get(k).map_or(Value::Null, |old| old["utilization"].clone());
        }
        windows.insert(k.to_string(), w);
    }
    let exceeded = info["status"] == "rejected";
    let near = !exceeded && (past_warning(&info["status"]) || windows.values().any(|w| past_warning(&w["status"])));
    let mut limits: Vec<Value> = windows.into_values().collect();
    limits.sort_by_key(|w| {
        let k = w["key"].as_str().unwrap_or_default().to_string();
        (["five_hour", "seven_day"].iter().position(|f| *f == k).unwrap_or(2), k)
    });
    json!({
        "status": info["status"],
        "limiting": limiting,
        "resets_at": info["resetsAt"].as_i64().map(|s| s.saturating_mul(1000)),
        "near": near,
        "exceeded": exceeded,
        "limits": limits,
        "overage": {
            "status": info["overageStatus"],
            "enabled": info["overageEnabled"],
            "in_use": info["isUsingOverage"],
            "disabled_reason": info["overageDisabledReason"],
            "resets_at": info["overageResetsAt"].as_i64().map(|s| s.saturating_mul(1000)),
        },
        "updated_at": now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line the installed CLI (2.1.326) printed for a Haiku turn on a Pro plan.
    const SEEN: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1795536000,"rateLimitType":"seven_day","utilization":0.76,"overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false,"surpassedThreshold":0.75,"unifiedWindows":{"seven_day":{"utilization":0.76,"resetsAt":1795536000,"status":"allowed_warning","surpassedThreshold":0.75}},"overageEnabled":false},"uuid":"0a48d519-a76f-49f4-9925-19a5ff067eb8","session_id":"e9fb3115-2ad5-4537-98d7-33ddfe121db3"}"#;
    const NOW: i64 = 1_795_000_000_000;

    #[test]
    fn a_weekly_limit_near_its_end_warns() {
        let ev: Value = serde_json::from_str(SEEN).unwrap();
        let p = merged(&Value::Null, info_of(&ev).unwrap(), NOW);
        assert_eq!(
            p,
            json!({
                "status": "allowed_warning", "limiting": "seven_day", "resets_at": 1_795_536_000_000i64, "near": true, "exceeded": false,
                "limits": [{"key": "seven_day", "utilization": 0.76, "resets_at": 1_795_536_000_000i64, "status": "allowed_warning", "threshold": 0.75}],
                "overage": {"status": "rejected", "enabled": false, "in_use": false, "disabled_reason": "org_level_disabled", "resets_at": null},
                "updated_at": NOW,
            })
        );
    }

    fn keys(p: &Value) -> Vec<&str> {
        p["limits"].as_array().unwrap().iter().map(|w| w["key"].as_str().unwrap()).collect()
    }

    fn limit<'a>(p: &'a Value, key: &str) -> &'a Value {
        p["limits"].as_array().unwrap().iter().find(|w| w["key"] == key).unwrap()
    }

    #[test]
    fn windows_add_up_and_go_when_they_reset() {
        let five = json!({"status": "allowed", "rateLimitType": "five_hour", "utilization": 0.2, "resetsAt": 1_795_010_000, "unifiedWindows": {
            "seven_day_fable": {"utilization": 0.9, "resetsAt": 1_795_536_000, "status": "allowed_warning"},
            "seven_day": {"utilization": 0.4, "resetsAt": 1_795_536_000},
            "five_hour": {"utilization": 0.2, "resetsAt": 1_795_010_000, "status": "allowed"},
            "junk": 3,
        }});
        let p = merged(&Value::Null, &five, NOW);
        // the 5-hour window first, then the weekly ones
        assert_eq!(keys(&p), ["five_hour", "seven_day", "seven_day_fable"]);
        // a window of a kind not seen before still warns; the limiting one is fine
        assert_eq!((p["status"].as_str(), p["near"].as_bool(), p["exceeded"].as_bool(), limit(&p, "seven_day")["status"].as_str()), (Some("allowed"), Some(true), Some(false), None));
        // a later event with only the top-level fields (an older CLI) updates that window and keeps the rest
        let rejected = json!({"status": "rejected", "rateLimitType": "five_hour", "resetsAt": 1_795_010_000});
        let p2 = merged(&p, &rejected, NOW + 1);
        assert_eq!((p2["exceeded"].as_bool(), p2["near"].as_bool()), (Some(true), Some(false)));
        assert_eq!((limit(&p2, "five_hour")["status"].as_str(), limit(&p2, "five_hour")["utilization"].as_f64()), (Some("rejected"), Some(0.2)));
        assert_eq!(limit(&p2, "seven_day")["utilization"].as_f64(), Some(0.4));
        // past its reset a window is no longer shown
        let p3 = merged(&p2, &json!({"status": "allowed"}), 1_795_010_000_001);
        assert_eq!(keys(&p3), ["seven_day", "seven_day_fable"]);
        // a window at its limit while requests still go through is near, not exceeded
        let p4 = merged(&Value::Null, &json!({"status": "allowed", "unifiedWindows": {"seven_day_opus": {"utilization": 1.0, "resetsAt": 1_795_536_000, "status": "rejected"}}}), NOW);
        assert_eq!((p4["near"].as_bool(), p4["exceeded"].as_bool()), (Some(true), Some(false)));
        // the overage limit isn't a window
        let p5 = merged(&Value::Null, &json!({"status": "allowed", "rateLimitType": "overage", "overageStatus": "allowed", "isUsingOverage": true}), NOW);
        assert_eq!((&p5["limits"], p5["overage"]["in_use"].as_bool(), p5["near"].as_bool()), (&json!([]), Some(true), Some(false)));
    }

    #[test]
    fn a_report_is_kept_and_sent_to_the_window() {
        use tauri::Listener;
        let dir = std::env::temp_dir().join(format!("cr-usage-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"));
        assert!(st.usage_limits.lock().is_null());
        let app = tauri::test::mock_app();
        let got = std::sync::Arc::new(parking_lot::Mutex::new(vec![]));
        let sink = got.clone();
        app.listen_any("usage-limits-changed", move |e| sink.lock().push(serde_json::from_str::<Value>(e.payload()).unwrap()));
        let ev: Value = serde_json::from_str(SEEN).unwrap();
        record(app.handle(), &st, info_of(&ev).unwrap());
        let kept = st.usage_limits.lock().clone();
        assert_eq!((kept["limiting"].as_str(), kept["near"].as_bool()), (Some("seven_day"), Some(true)));
        assert_eq!(*got.lock(), vec![kept]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_refused_requests_limit_is_read_too() {
        let refused = json!({"type": "assistant", "api_error": "usage_limit_reached", "api_error_params": {"rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour"}}});
        assert_eq!(info_of(&refused).unwrap()["status"], "rejected");
        for other in [json!({"type": "assistant", "message": {}}), json!({"type": "rate_limit_event"}), json!({"type": "rate_limit_event", "rate_limit_info": "x"})] {
            assert!(info_of(&other).is_none(), "{other}");
        }
    }

    #[test]
    fn garbage_in_a_limit_line_is_read_as_unknown() {
        let junk = json!({"status": 7, "rateLimitType": ["x"], "resetsAt": i64::MAX, "utilization": "lots",
            "unifiedWindows": {"five_hour": {"resetsAt": i64::MIN, "utilization": {}}, "odd": 3, "seven_day": null}});
        let p = merged(&json!({"limits": "not a list"}), &junk, NOW);
        assert_eq!(p["resets_at"], json!(i64::MAX));
        assert_eq!(p["limits"], json!([{"key": "five_hour", "utilization": null, "resets_at": i64::MIN, "status": null, "threshold": null}]));
        assert_eq!((p["near"].as_bool(), p["exceeded"].as_bool()), (Some(false), Some(false)));
        // a window that has reset since drops out on the next line
        assert_eq!(merged(&p, &json!({}), NOW)["limits"], json!([]));
    }
}

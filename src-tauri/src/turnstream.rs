//! Compact `turn` events while a reply streams.
//!
//! The engine re-sends the whole rendered turn about 20 times a second. In a
//! long agentic turn almost all of it (finished text, tool calls with their
//! outputs, artifacts) is the same as in the previous event, yet every event
//! was serialised, evaluated as a script in the WebView and parsed again.
//!
//! Here each conversation's stream is a keyframe (the full turn, as before)
//! followed by deltas: the same shape, with every element of `parts` and of
//! `artifacts` that equals the previous event's written as `null`, and the
//! unchanged `msg_ids`, `siblings` and `meta` written as `null` and named in
//! `same`. Payloads:
//!
//! - keyframe: `{conversation_id, turn, seq}`
//! - delta: `{conversation_id, turn, seq, base, same}`, where `base` is the
//!   `seq` of the event it was computed against
//!
//! `seq` grows across all conversations and never repeats while the app
//! runs, so a delta can only be expanded with the exact event it was made
//! from. `ui/js/turnstream.js` expands it into the full turn and drops the
//! deltas it can't expand (after a missed event, a reloaded page) until the
//! next keyframe. Keyframes go out at least every [`KEYFRAME_EVERY`], for a
//! new turn, for a non-streaming turn and whenever nothing could be left out.

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};

/// Longest a reader that missed an event waits for the next full turn.
pub const KEYFRAME_EVERY: Duration = Duration::from_millis(1000);
/// A conversation's state is dropped this long after its last event (a
/// safety net for `end`; a missing state only means the next event is a
/// keyframe).
const IDLE_DROP: Duration = Duration::from_secs(15 * 60);
/// The lists whose unchanged elements are left out of a delta.
const ELIDED: [&str; 2] = ["parts", "artifacts"];
/// The values left out of a delta when they didn't change.
const KEPT: [&str; 3] = ["msg_ids", "siblings", "meta"];

/// Emit `turn` for `conv_id`: a drop-in for `engine::emit_turn`.
pub fn emit_turn<R: Runtime>(app: &AppHandle<R>, conv_id: &str, turn: &Value) {
    // serialised under the lock so `seq` follows the order of the events;
    // emitted outside it (`emit` can wait for the main thread)
    let payload = ENCODER.lock().unwrap_or_else(|e| e.into_inner()).encode(conv_id, turn, Instant::now());
    let _ = app.emit_str("turn", payload);
}

/// The turn running in `conv_id` ended: forget what its readers have.
pub fn end(conv_id: &str) {
    ENCODER.lock().unwrap_or_else(|e| e.into_inner()).end(conv_id);
}

static ENCODER: LazyLock<Mutex<Encoder>> = LazyLock::new(|| Mutex::new(Encoder::default()));

/// What the readers of one conversation's stream last received.
struct Sent {
    seq: u64,
    turn_id: Value,
    /// the full elements of the `ELIDED` lists, as last sent
    lists: [Vec<Value>; 2],
    /// the `KEPT` values, as last sent
    kept: [Value; 3],
    keyframe_at: Instant,
    at: Instant,
}

/// What a delta leaves out.
struct Same {
    /// per element of each `ELIDED` list
    lists: [Vec<bool>; 2],
    /// per `KEPT` value
    kept: [bool; 3],
}

#[derive(Default)]
struct Encoder {
    seq: u64,
    convs: HashMap<String, Sent>,
    swept_at: Option<Instant>,
}

impl Encoder {
    fn end(&mut self, conv_id: &str) {
        self.convs.remove(conv_id);
    }

    /// The payload of the next `turn` event of `conv_id`, as JSON.
    fn encode(&mut self, conv_id: &str, turn: &Value, now: Instant) -> String {
        self.sweep(now);
        self.seq += 1;
        let seq = self.seq;
        let lists = ELIDED.map(|k| turn.get(k).and_then(Value::as_array));
        let streaming = turn["streaming"].as_bool() == Some(true);
        let base = self.convs.get(conv_id).filter(|s| {
            streaming
                && s.turn_id == turn["id"]
                && now.duration_since(s.keyframe_at) < KEYFRAME_EVERY
                && lists.iter().all(|l| l.is_some())
        });
        // left out when it equals what the reader has
        let same = base
            .map(|s| Same {
                lists: [0, 1].map(|i| {
                    let new = lists[i].map(Vec::as_slice).unwrap_or(&[]);
                    new.iter().enumerate().map(|(j, v)| s.lists[i].get(j) == Some(v)).collect()
                }),
                kept: [0, 1, 2].map(|i| turn.get(KEPT[i]).is_some_and(|v| !v.is_null() && *v == s.kept[i])),
            })
            .filter(|m| m.lists.iter().flatten().any(|&b| b));
        let base_seq = same.as_ref().and(base.map(|s| s.seq));
        let kept: Vec<&str> = same.as_ref().map(|m| (0..3).filter(|&i| m.kept[i]).map(|i| KEPT[i]).collect()).unwrap_or_default();
        let json = serde_json::to_string(&Payload {
            conversation_id: conv_id,
            turn: TurnOut { turn, same: same.as_ref() },
            seq,
            base: base_seq,
            same: kept,
        })
        .unwrap_or_default();

        if !streaming || !turn.is_object() {
            self.convs.remove(conv_id);
            return json;
        }
        let keyframe = base_seq.is_none();
        let s = self.convs.entry(conv_id.to_string()).or_insert_with(|| Sent {
            seq,
            turn_id: Value::Null,
            lists: Default::default(),
            kept: Default::default(),
            keyframe_at: now,
            at: now,
        });
        s.seq = seq;
        s.at = now;
        if keyframe {
            s.keyframe_at = now;
            s.turn_id = turn["id"].clone();
        }
        // keep the reader's copy in step, cloning only what changed
        for (i, l) in lists.iter().enumerate() {
            let new = l.map(Vec::as_slice).unwrap_or(&[]);
            let mine = &mut s.lists[i];
            mine.truncate(new.len());
            for (j, v) in new.iter().enumerate() {
                match mine.get_mut(j) {
                    Some(m) if m == v => {}
                    Some(m) => *m = v.clone(),
                    None => mine.push(v.clone()),
                }
            }
        }
        for (i, k) in KEPT.iter().enumerate() {
            let v = turn.get(*k).unwrap_or(&Value::Null);
            if s.kept[i] != *v {
                s.kept[i] = v.clone();
            }
        }
        json
    }

    fn sweep(&mut self, now: Instant) {
        if self.swept_at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(60)) {
            return;
        }
        self.swept_at = Some(now);
        self.convs.retain(|_, s| now.duration_since(s.at) < IDLE_DROP);
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    conversation_id: &'a str,
    turn: TurnOut<'a>,
    seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    same: Vec<&'a str>,
}

/// A turn, with what `same` marks written as `null`.
struct TurnOut<'a> {
    turn: &'a Value,
    same: Option<&'a Same>,
}

impl Serialize for TurnOut<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (Some(obj), Some(same)) = (self.turn.as_object(), self.same) else {
            return self.turn.serialize(s);
        };
        let mut m = s.serialize_map(Some(obj.len()))?;
        for (k, v) in obj {
            if let Some(i) = ELIDED.iter().position(|e| e == k) {
                m.serialize_entry(k, &Elided(v, &same.lists[i]))?;
            } else if KEPT.iter().position(|e| e == k).is_some_and(|i| same.kept[i]) {
                m.serialize_entry(k, &())?;
            } else {
                m.serialize_entry(k, v)?;
            }
        }
        m.end()
    }
}

struct Elided<'a>(&'a Value, &'a [bool]);

impl Serialize for Elided<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Some(items) = self.0.as_array() else {
            return self.0.serialize(s);
        };
        let mut seq = s.serialize_seq(Some(items.len()))?;
        for (i, v) in items.iter().enumerate() {
            if self.1.get(i) == Some(&true) {
                seq.serialize_element(&())?;
            } else {
                seq.serialize_element(v)?;
            }
        }
        seq.end()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What `ui/js/turnstream.js` does, for checking the encoder.
    #[derive(Default)]
    struct Reader {
        last: HashMap<String, (u64, Value)>,
    }

    impl Reader {
        /// The full payload handed to the UI, or None for a dropped delta.
        fn read(&mut self, json: &str) -> Option<Value> {
            let mut p: Value = serde_json::from_str(json).unwrap();
            let conv = p["conversation_id"].as_str().unwrap().to_string();
            let seq = p["seq"].as_u64().unwrap();
            if let Some(base) = p.get("base").and_then(Value::as_u64) {
                let (have, prev) = self.last.get(&conv)?;
                if *have != base {
                    return None;
                }
                for k in ELIDED {
                    let items = p["turn"][k].as_array_mut().unwrap();
                    for (i, v) in items.iter_mut().enumerate() {
                        if v.is_null() {
                            *v = prev[k][i].clone();
                        }
                    }
                }
                for k in p["same"].as_array().cloned().unwrap_or_default() {
                    let k = k.as_str().unwrap();
                    assert!(p["turn"][k].is_null());
                    p["turn"][k] = prev[k].clone();
                }
            }
            let turn = p["turn"].clone();
            if turn["streaming"] == true {
                self.last.insert(conv, (seq, turn.clone()));
            } else {
                self.last.remove(&conv);
            }
            let mut out = p;
            let o = out.as_object_mut().unwrap();
            o.remove("seq");
            o.remove("base");
            o.remove("same");
            Some(out)
        }
    }

    fn turn(id: &str, parts: Vec<Value>, streaming: bool) -> Value {
        json!({"id": id, "role": "assistant", "msg_ids": [id], "parts": parts,
               "artifacts": [], "streaming": streaming, "meta": {"usage": {}}, "siblings": null})
    }

    fn text(s: &str) -> Value {
        json!({"kind": "text", "html": format!("<p>{s}</p>"), "raw": s, "sources": []})
    }

    fn tool(id: &str, status: &str, out: &str) -> Value {
        json!({"kind": "tool", "id": id, "name": "Bash", "input": {"command": "ls"}, "status": status,
               "result": if out.is_empty() { Value::Null } else { json!({"text": out}) }})
    }

    fn full(conv: &str, t: &Value) -> Value {
        json!({"conversation_id": conv, "turn": t})
    }

    #[test]
    fn deltas_expand_to_the_full_turn() {
        let mut e = Encoder::default();
        let mut r = Reader::default();
        let t0 = Instant::now();
        let big = "x".repeat(5000);
        let mut turns = vec![];
        for i in 0..30 {
            let mut parts = vec![text("Looking"), tool("t1", "done", &big)];
            if i > 10 {
                parts.push(tool("t2", if i > 20 { "done" } else { "running" }, if i > 20 { "ok" } else { "" }));
            }
            parts.push(text(&"streamed ".repeat(i + 1)));
            turns.push(turn("a1", parts, true));
        }
        let mut keys = vec![];
        for (i, t) in turns.iter().enumerate() {
            let now = t0 + Duration::from_millis(45 * i as u64);
            let json = e.encode("c1", t, now);
            let key = !json.contains("\"base\"");
            // deltas leave out the 5 kB tool output
            assert!(if key { json.len() > 5000 } else { json.len() < 1500 }, "event {i}: {}", json.len());
            if key {
                keys.push(i);
            }
            assert_eq!(r.read(&json).unwrap(), full("c1", t), "event {i}");
        }
        // a keyframe at least every second
        assert_eq!(keys, [0, 23], "{keys:?}");
    }

    #[test]
    fn a_missed_event_drops_deltas_until_the_next_keyframe() {
        let mut e = Encoder::default();
        let mut r = Reader::default();
        let t0 = Instant::now();
        let mut got = vec![];
        for i in 0..40u64 {
            let t = turn("a1", vec![text("same"), text(&format!("n{i}"))], true);
            let json = e.encode("c1", &t, t0 + Duration::from_millis(100 * i));
            if i == 3 {
                continue; // lost on the way
            }
            if let Some(p) = r.read(&json) {
                assert_eq!(p, full("c1", &t), "event {i}");
                got.push(i);
            }
        }
        assert!(got.starts_with(&[0, 1, 2]), "{got:?}");
        // nothing between the lost event and the next keyframe (≤ 1 s later)
        let next = got[3];
        assert!((10..=14).contains(&next), "{got:?}");
        assert_eq!(got.len() as u64, 40 - next + 3);
    }

    #[test]
    fn a_reader_that_joins_late_waits_for_a_keyframe() {
        let mut e = Encoder::default();
        let t0 = Instant::now();
        let mut r = Reader::default();
        let mut first = None;
        for i in 0..30u64 {
            let t = turn("a1", vec![text("a"), text(&format!("b{i}"))], true);
            let json = e.encode("c1", &t, t0 + Duration::from_millis(45 * i));
            if i < 5 {
                continue; // the page wasn't listening yet
            }
            if let Some(p) = r.read(&json) {
                first.get_or_insert(i);
                assert_eq!(p, full("c1", &t));
            }
        }
        let first = first.unwrap();
        assert!(first > 5 && 45 * first <= 1000 + 45 * 2, "{first}");
    }

    #[test]
    fn conversations_and_turns_are_kept_apart() {
        let mut e = Encoder::default();
        let mut r = Reader::default();
        let t0 = Instant::now();
        let mut n = 0u64;
        let mut step = |conv: &str, t: &Value, e: &mut Encoder, r: &mut Reader| {
            n += 1;
            let json = e.encode(conv, t, t0 + Duration::from_millis(10 * n));
            assert_eq!(r.read(&json).unwrap(), full(conv, t), "{conv} {n}");
            json
        };
        let a = |i: usize| turn("a1", vec![text("shared"), text(&i.to_string())], true);
        let b = |i: usize| turn("b1", vec![text("shared"), text(&format!("b{i}"))], true);
        for i in 0..5 {
            step("c1", &a(i), &mut e, &mut r);
            step("c2", &b(i), &mut e, &mut r);
        }
        // a new turn in c1 (same parts at first) starts with a keyframe
        let json = step("c1", &turn("a2", vec![text("shared"), text("4")], true), &mut e, &mut r);
        assert!(!json.contains("\"base\""), "{json}");
        // the final, non-streaming event is complete and ends the state
        let done = turn("b1", vec![text("shared"), text("b4"), text("end")], false);
        let json = step("c2", &done, &mut e, &mut r);
        assert!(!json.contains("\"base\"") && !e.convs.contains_key("c2"), "{json}");
        let json = step("c2", &b(9), &mut e, &mut r);
        assert!(!json.contains("\"base\""), "{json}");
        // end() drops a conversation's state
        e.end("c1");
        let json = step("c1", &a(7), &mut e, &mut r);
        assert!(!json.contains("\"base\""), "{json}");
    }

    #[test]
    fn shrinking_and_reordered_lists() {
        let mut e = Encoder::default();
        let mut r = Reader::default();
        let t0 = Instant::now();
        let seqs = [
            vec![text("a"), text("b"), text("c")],
            vec![text("a"), text("b")],
            vec![json!({"kind": "notice", "code": "x"}), text("a"), text("b")],
            vec![json!({"kind": "notice", "code": "x"}), text("a"), text("b"), text("c")],
            vec![],
            vec![text("z")],
        ];
        for (i, parts) in seqs.into_iter().enumerate() {
            let mut t = turn("a1", parts, true);
            t["artifacts"] = json!([{"identifier": "x", "content": "y".repeat(i)}, {"identifier": "k", "content": "same"}]);
            t["msg_ids"] = json!((0..i / 2).collect::<Vec<_>>());
            t["meta"] = json!({"n": i / 3});
            t["siblings"] = if i == 3 { Value::Null } else { json!({"count": 1 + i / 4}) };
            let json = e.encode("c1", &t, t0 + Duration::from_millis(i as u64));
            assert_eq!(r.read(&json).unwrap(), full("c1", &t), "event {i}");
        }
    }

    #[test]
    fn odd_turns_are_sent_whole() {
        let mut e = Encoder::default();
        let mut r = Reader::default();
        let t0 = Instant::now();
        for (i, t) in [json!({"id": "a", "streaming": true}), json!(null), json!({"id": "a", "streaming": true, "parts": "x"})]
            .iter()
            .enumerate()
        {
            for k in 0..2 {
                let json = e.encode("c1", t, t0 + Duration::from_millis((i * 2 + k) as u64));
                assert!(!json.contains("\"base\""), "{json}");
                let p: Value = serde_json::from_str(&json).unwrap();
                assert_eq!(p["turn"], *t);
            }
        }
        let _ = &mut r;
    }

    // ---- measurement: an agentic turn streamed the way the engine does it

    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    const WORDS: [&str; 16] = ["the", "file", "config", "parse", "error", "value", "returns", "state", "update", "check",
        "handler", "request", "token", "list", "build", "path"];

    fn prose(r: &mut Rng, min: usize, extra: usize) -> String {
        let words = min + r.next(extra);
        (0..words).map(|i| if i % 13 == 12 { format!("{}.", WORDS[r.next(16)]) } else { WORDS[r.next(16)].to_string() }).collect::<Vec<_>>().join(" ")
    }

    fn code(r: &mut Rng, min: usize, extra: usize) -> String {
        let lines = min + r.next(extra);
        (0..lines)
            .map(|_| match r.next(5) {
                0 => format!("    let {} = {}(&{}, {});", WORDS[r.next(16)], WORDS[r.next(16)], WORDS[r.next(16)], r.next(1000)),
                1 => format!("    if {}.is_empty() {{ return Err(Error::{}); }}", WORDS[r.next(16)], WORDS[r.next(16)]),
                2 => format!("pub fn {}_{}(x: &str) -> Result<String> {{", WORDS[r.next(16)], WORDS[r.next(16)]),
                3 => "    }".to_string(),
                _ => format!("    // {}", prose(r, 8, 1)),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn numbered(r: &mut Rng, min: usize, extra: usize) -> String {
        code(r, min, extra).lines().enumerate().map(|(i, l)| format!("{:>6}\u{2192}{l}", i + 1)).collect::<Vec<_>>().join("\n")
    }

    fn msg(id: String, role: &str, content: Value) -> crate::db::Message {
        crate::db::Message { id, conversation_id: "c".into(), parent_id: None, role: role.into(), content, meta: json!({}), created_at: 0 }
    }

    /// One round: (text, tool name, input, result)
    fn round(r: &mut Rng, i: usize) -> (String, &'static str, Value, String) {
        let mut text = prose(r, 30, 90);
        if i % 4 == 1 {
            text.push_str(&format!("\n\n```rust\n{}\n```\n\n{}", code(r, 15, 25), prose(r, 20, 1)));
        }
        let path = format!("src/{}/{}.rs", WORDS[r.next(16)], WORDS[r.next(16)]);
        match i % 6 {
            0 | 3 => (text, "Read", json!({"file_path": path}), numbered(r, 80, 220)),
            1 => (text, "Grep", json!({"pattern": WORDS[r.next(16)], "path": "src"}), numbered(r, 10, 30)),
            2 | 5 => (
                text,
                "Edit",
                json!({"file_path": path, "old_string": code(r, 5, 15), "new_string": code(r, 5, 20)}),
                format!("The file {path} has been updated."),
            ),
            _ => (text, "Bash", json!({"command": "cargo test 2>&1 | tail -50"}), code(r, 20, 60)),
        }
    }

    /// Stream a turn of `rounds` tool calls, `chars` characters per event
    /// (~45 ms of output), and return each event's turn.
    fn agentic_turn(rounds: usize, chars: usize, mut each: impl FnMut(&Value)) {
        use crate::render::{assistant_turn, ArtifactTracker, RenderCtx};
        let mut r = Rng(7);
        let mut done: Vec<crate::db::Message> = vec![];
        let mcp = HashMap::new();
        for i in 0..rounds {
            let (text, name, input, result) = round(&mut r, i);
            let tid = format!("toolu_{i:04}");
            let input_s = input.to_string();
            let total = text.len() + input_s.len();
            let mut at = 0;
            loop {
                at = (at + chars).min(total);
                let shown: String = text.chars().take(at).collect();
                let mut content = vec![json!({"type": "text", "text": shown})];
                let mut streaming_tools = std::collections::HashSet::new();
                if at > text.len() {
                    let partial = &input_s[..(at - text.len()).min(input_s.len())];
                    let inp = crate::render::close_partial_json(partial).unwrap_or_else(|| json!({}));
                    content.push(json!({"type": "tool_use", "id": tid, "name": name, "input": if at == total { input.clone() } else { inp }}));
                    if at < total {
                        streaming_tools.insert(tid.clone());
                    }
                }
                let cur = msg(format!("a{i}"), "assistant", json!(content));
                let mut msgs: Vec<&crate::db::Message> = done.iter().collect();
                msgs.push(&cur);
                let mut live = HashMap::new();
                if at == total {
                    live.insert(tid.clone(), "running".to_string());
                }
                let ctx = RenderCtx { math: false, live_tools: &live, streaming_tools: &streaming_tools, mcp_names: &mcp, streaming_msg: Some(&cur.id) };
                let mut t = assistant_turn(&msgs, &ctx, &mut ArtifactTracker::default());
                t["id"] = json!("a0");
                t["siblings"] = json!({"count": 1, "index": 0, "ids": ["a0"]});
                each(&t);
                if at == total {
                    done.push(cur);
                    break;
                }
            }
            done.push(msg(format!("u{i}"), "user", json!([{"type": "tool_result", "tool_use_id": tid, "content": result}])));
        }
    }

    /// `cargo test -p clauderust --lib turnstream::tests::measure -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure() {
        for (rounds, chars) in [(10, 12), (40, 12)] {
            let mut e = Encoder::default();
            let mut rd = Reader::default();
            let t0 = Instant::now();
            let (mut n, mut full_total, mut delta_total, mut full_last, mut delta_max) = (0u64, 0usize, 0usize, 0usize, 0usize);
            let (mut t_full, mut t_delta) = (Duration::ZERO, Duration::ZERO);
            let mut sizes = vec![];
            agentic_turn(rounds, chars, |t| {
                let s = Instant::now();
                let f = serde_json::to_string(&json!({"conversation_id": "c1", "turn": t})).unwrap();
                t_full += s.elapsed();
                let s = Instant::now();
                let d = e.encode("c1", t, t0 + Duration::from_millis(45 * n));
                t_delta += s.elapsed();
                n += 1;
                full_total += f.len();
                delta_total += d.len();
                full_last = f.len();
                delta_max = delta_max.max(if d.contains("\"base\"") { d.len() } else { 0 });
                sizes.push((f.len(), d.len()));
                assert_eq!(rd.read(&d).unwrap(), full("c1", t));
            });
            let mut ds: Vec<usize> = sizes.iter().filter(|(f, d)| f != d).map(|x| x.1).collect();
            ds.sort();
            println!(
                "rounds {rounds}: {n} events; full: total {full_total} B, last {full_last} B, mean {} B, serialize {:?}; \
                 compact: total {delta_total} B ({:.1}%), mean {} B, median delta {} B, max delta {delta_max} B, encode {:?}",
                full_total / n as usize,
                t_full,
                100.0 * delta_total as f64 / full_total as f64,
                delta_total / n as usize,
                ds.get(ds.len() / 2).copied().unwrap_or(0),
                t_delta,
            );
        }
    }

    /// Events for checking `ui/js/turnstream.js` against the encoder: each
    /// delivered payload with the full payload the UI must get (null where
    /// it can't be expanded). Two conversations interleaved, a lost event,
    /// a new turn, a non-streaming turn and `end`.
    /// `TURNSTREAM_FIXTURE=<file> cargo test -p clauderust --lib turnstream::tests::fixture -- --ignored`
    #[test]
    #[ignore]
    fn fixture() {
        let Some(path) = std::env::var_os("TURNSTREAM_FIXTURE") else { return };
        let mut e = Encoder::default();
        let mut r = Reader::default();
        let t0 = Instant::now();
        let mut out = vec![];
        let mut n = 0u64;
        let mut emit = |conv: &str, t: &Value, lost: bool, e: &mut Encoder, r: &mut Reader, out: &mut Vec<Value>| {
            n += 1;
            let json = e.encode(conv, t, t0 + Duration::from_millis(45 * n));
            if lost {
                return;
            }
            let got = r.read(&json);
            if let Some(g) = &got {
                assert_eq!(*g, full(conv, t));
            }
            out.push(json!({"payload": json, "full": got}));
        };
        let mut a = vec![];
        agentic_turn(6, 40, |t| a.push(t.clone()));
        let mut b = vec![];
        agentic_turn(3, 25, |t| b.push(t.clone()));
        for (i, t) in a.iter().enumerate() {
            emit("ca", t, matches!(i, 30 | 52 | 71), &mut e, &mut r, &mut out);
            if let Some(tb) = b.get(i) {
                let mut tb = tb.clone();
                tb["id"] = json!("b0");
                emit("cb", &tb, false, &mut e, &mut r, &mut out);
            }
        }
        let mut last = a.last().unwrap().clone();
        last["streaming"] = json!(false);
        emit("ca", &last, false, &mut e, &mut r, &mut out);
        e.end("cb");
        let mut again = b[5].clone();
        again["id"] = json!("b1");
        emit("cb", &again, false, &mut e, &mut r, &mut out);
        emit("cb", &b[6], false, &mut e, &mut r, &mut out);
        std::fs::write(path, serde_json::to_string(&out).unwrap()).unwrap();
    }

    #[test]
    fn idle_conversations_are_forgotten() {
        let mut e = Encoder::default();
        let t0 = Instant::now();
        e.encode("c1", &turn("a1", vec![text("a")], true), t0);
        e.encode("c2", &turn("b1", vec![text("a")], true), t0 + IDLE_DROP - Duration::from_secs(1));
        assert!(e.convs.contains_key("c1"));
        e.encode("c2", &turn("b1", vec![text("a")], true), t0 + IDLE_DROP + Duration::from_secs(61));
        assert!(!e.convs.contains_key("c1") && e.convs.contains_key("c2"));
    }
}

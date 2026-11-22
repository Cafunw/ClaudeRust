//! Compact `turn` events while a reply streams.
//!
//! The engine re-sends the whole rendered turn up to 10 times a second. In a
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
//! A changed element of `parts` or `artifacts` that kept its fields is written
//! as an array of edits of the reader's copy instead, applied in order. The
//! page says which it expands with [`turn_stream_protocol`]:
//!
//! - protocol 2: `[key, kept, appended]`, a string that is the first `kept`
//!   UTF-16 units of the old one followed by `appended`
//! - protocol 3 adds the same for an array (its first `kept` items, then the
//!   `appended` ones), `[key, at, removed, inserted]` (a string with `removed`
//!   units at `at` replaced by `inserted`), `[key, value]` (the field is
//!   `value`) and `[key, null, edits]` (an object edited in turn, such as a
//!   tool's input as it streams in). A block that closes, whose HTML changes
//!   near its start, then costs what changed instead of the whole block
//!   again; a tool that ends, its status and result.
//!
//! `seq` grows across all conversations and never repeats while the app
//! runs, so a delta can only be expanded with the exact event it was made
//! from. `ui/js/turnstream.js` expands it into the full turn. A keyframe goes
//! out for a new turn, for a non-streaming turn, when nothing could be left
//! out, and when a reader has nothing to expand deltas with: a page that
//! starts listening ([`turn_stream_protocol`]) or one that missed an event
//! ([`turn_stream_resync`]). Never on a timer: what a reply has finished is
//! sent once, so its events cost about its size, not its size squared.

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};

/// Where an old and a new string agree again after they differ (an attribute
/// gone, a line highlighted anew): `RESYNC` equal bytes, searched for up to
/// `SEARCH` bytes on from the difference in each.
const RESYNC: usize = 32;
const SEARCH: usize = 64;
/// Splices of one string at most; the rest of it is then sent as it is.
const MAX_SPLICES: usize = 8;
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

/// A page starts listening: its decoder says which payloads it expands.
#[tauri::command]
pub fn turn_stream_protocol(version: u32) {
    ENCODER.lock().unwrap_or_else(|e| e.into_inner()).announce(version);
}

/// The page can't expand the deltas of `conversation_id` (it missed an
/// event): the next event of it is a keyframe.
#[tauri::command]
pub fn turn_stream_resync(conversation_id: String) {
    end(&conversation_id);
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
    at: Instant,
}

/// How a delta writes one element of an `ELIDED` list.
enum Elem<'a> {
    /// as the reader has it: `null`
    Same,
    /// edits of the reader's copy
    Edits(Vec<Edit<'a>>),
    Whole,
}

/// One edit of a field of the reader's copy of an element. Positions and
/// lengths count UTF-16 units (a JS string's length), whole characters only.
enum Edit<'a> {
    /// `[key, kept, appended]`
    Tail(&'a str, usize, Appended<'a>),
    /// `[key, at, removed, inserted]`, on the string as edited so far
    Splice(&'a str, usize, usize, &'a str),
    /// `[key, value]`
    Set(&'a str, &'a Value),
    /// `[key, null, edits]`: edits of an object
    Inner(&'a str, Vec<Edit<'a>>),
}

enum Appended<'a> {
    Str(&'a str),
    Items(&'a [Value]),
}

/// What a delta leaves out.
struct Same<'a> {
    /// per element of each `ELIDED` list
    lists: [Vec<Elem<'a>>; 2],
    /// per `KEPT` value
    kept: [bool; 3],
}

#[derive(Default)]
struct Encoder {
    seq: u64,
    convs: HashMap<String, Sent>,
    swept_at: Option<Instant>,
    /// the reader's protocol: 2 adds string tails, 3 the other edits
    level: u32,
}

/// `new` as edits of `old`, an element with the same fields, or None when it
/// goes whole: a field added or removed, or (protocol 2) one that isn't a string.
fn edits<'a>(old: &Value, new: &'a Value, level: u32) -> Option<Vec<Edit<'a>>> {
    let (old, new) = (old.as_object()?, new.as_object()?);
    if old.len() != new.len() {
        return None;
    }
    let mut out = vec![];
    for (k, v) in new {
        match (old.get(k)?, v) {
            (Value::String(a), Value::String(b)) => string_edits(k, a, b, level >= 3, &mut out),
            (o, _) if o == v => {}
            (Value::Array(a), Value::Array(b)) if level >= 3 => {
                let kept = a.iter().zip(b).take_while(|(x, y)| x == y).count();
                out.push(Edit::Tail(k, kept, Appended::Items(&b[kept..])));
            }
            // a tool's input as it streams in (a file being written)
            (o @ Value::Object(_), Value::Object(_)) if level >= 3 => out.push(edits(o, v, level).map_or(Edit::Set(k, v), |e| Edit::Inner(k, e))),
            _ if level >= 3 => out.push(Edit::Set(k, v)),
            _ => return None,
        }
    }
    Some(out)
}

/// `new` as edits of `old` (none when they're equal): what they start with
/// kept, then up to `MAX_SPLICES` places where they differ and agree again
/// soon after (`splice`), then the rest of `new`.
fn string_edits<'a>(key: &'a str, old: &str, new: &'a str, splice: bool, out: &mut Vec<Edit<'a>>) {
    let (mut i, mut j, mut units, mut splices) = (0, 0, 0, 0);
    loop {
        let mut run = common(&old.as_bytes()[i..], &new.as_bytes()[j..]);
        // equal bytes so far: a character boundary in one is one in the other
        while !new.is_char_boundary(j + run) {
            run -= 1;
        }
        units += utf16_len(&new[j..j + run]);
        (i, j) = (i + run, j + run);
        if i == old.len() && j == new.len() {
            return;
        }
        let found = if splice && splices < MAX_SPLICES { resync(old, new, i, j) } else { None };
        let Some((d, s)) = found else {
            out.push(Edit::Tail(key, units, Appended::Str(&new[j..])));
            return;
        };
        out.push(Edit::Splice(key, units, utf16_len(&old[i..i + d]), &new[j..j + s]));
        units += utf16_len(&new[j..j + s]);
        (i, j, splices) = (i + d, j + s, splices + 1);
    }
}

/// How many bytes `a` and `b` start with in common.
fn common(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut k = 0;
    // whole chunks first: a slice comparison is much faster than byte by byte
    while k + 64 <= n && a[k..k + 64] == b[k..k + 64] {
        k += 64;
    }
    k + a[k..n].iter().zip(&b[k..n]).take_while(|(x, y)| x == y).count()
}

/// The nearest place after `old[i..]` and `new[j..]` differ where they agree
/// for `RESYNC` bytes: how far on it is in each.
fn resync(old: &str, new: &str, i: usize, j: usize) -> Option<(usize, usize)> {
    let (a, b) = (&old.as_bytes()[i..], &new.as_bytes()[j..]);
    if a.len() < RESYNC || b.len() < RESYNC {
        return None;
    }
    (1..=2 * SEARCH).find_map(|t| {
        (t.saturating_sub(SEARCH)..=t.min(SEARCH)).map(|d| (d, t - d)).find(|&(d, s)| {
            a.len() >= d + RESYNC
                && b.len() >= s + RESYNC
                && a[d..d + RESYNC] == b[s..s + RESYNC]
                && old.is_char_boundary(i + d)
                && new.is_char_boundary(j + s)
        })
    })
}

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().map(char::len_utf16).sum()
    }
}

impl Encoder {
    fn end(&mut self, conv_id: &str) {
        self.convs.remove(conv_id);
    }

    /// A page starts listening: it has no stream to expand deltas with.
    fn announce(&mut self, level: u32) {
        self.level = level;
        self.convs.clear();
    }

    /// The payload of the next `turn` event of `conv_id`, as JSON.
    fn encode(&mut self, conv_id: &str, turn: &Value, now: Instant) -> String {
        self.sweep(now);
        self.seq += 1;
        let seq = self.seq;
        let lists = ELIDED.map(|k| turn.get(k).and_then(Value::as_array));
        let streaming = turn["streaming"].as_bool() == Some(true);
        let base = self.convs.get(conv_id).filter(|s| streaming && s.turn_id == turn["id"] && lists.iter().all(|l| l.is_some()));
        // left out when it equals what the reader has
        let same = base
            .map(|s| Same {
                lists: [0, 1].map(|i| {
                    let new = lists[i].map(Vec::as_slice).unwrap_or(&[]);
                    new.iter()
                        .enumerate()
                        .map(|(j, v)| match s.lists[i].get(j) {
                            Some(old) if old == v => Elem::Same,
                            Some(old) if self.level >= 2 => edits(old, v, self.level).map_or(Elem::Whole, Elem::Edits),
                            _ => Elem::Whole,
                        })
                        .collect()
                }),
                kept: [0, 1, 2].map(|i| turn.get(KEPT[i]).is_some_and(|v| !v.is_null() && *v == s.kept[i])),
            })
            .filter(|m| m.lists.iter().flatten().any(|e| !matches!(e, Elem::Whole)));
        let base_seq = same.as_ref().and(base.map(|s| s.seq));
        let kept: Vec<&str> = same.as_ref().map(|m| (0..3).filter(|&i| m.kept[i]).map(|i| KEPT[i]).collect()).unwrap_or_default();
        let json = serde_json::to_string(&Payload {
            conversation_id: conv_id,
            turn: TurnOut { turn, same: same.as_ref() },
            seq,
            base: base_seq,
            same: kept,
        })
        // can't fail (string keys only); "null" keeps the event's script valid
        .unwrap_or_else(|_| "null".into());

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
            at: now,
        });
        s.seq = seq;
        s.at = now;
        if keyframe {
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
    same: Option<&'a Same<'a>>,
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

struct Elided<'a>(&'a Value, &'a [Elem<'a>]);

impl Serialize for Elided<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Some(items) = self.0.as_array() else {
            return self.0.serialize(s);
        };
        let mut seq = s.serialize_seq(Some(items.len()))?;
        for (i, v) in items.iter().enumerate() {
            match self.1.get(i) {
                Some(Elem::Same) => seq.serialize_element(&())?,
                Some(Elem::Edits(e)) => seq.serialize_element(e)?,
                _ => seq.serialize_element(v)?,
            }
        }
        seq.end()
    }
}

impl Serialize for Edit<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Edit::Tail(k, kept, Appended::Str(a)) => (k, kept, a).serialize(s),
            Edit::Tail(k, kept, Appended::Items(a)) => (k, kept, a).serialize(s),
            Edit::Splice(k, at, removed, inserted) => (k, at, removed, inserted).serialize(s),
            Edit::Set(k, v) => (k, v).serialize(s),
            Edit::Inner(k, e) => (k, (), e).serialize(s),
        }
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
                        } else if let Some(ops) = v.as_array() {
                            let mut el = prev[k][i].clone();
                            for op in ops {
                                apply(&mut el, op.as_array().unwrap());
                            }
                            *v = el;
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

    /// One edit of an element, as `ui/js/turnstream_delta.js` applies it.
    fn apply(el: &mut Value, op: &[Value]) {
        let k = op[0].as_str().unwrap();
        let n = |i: usize| op[i].as_u64().unwrap() as usize;
        let units = |v: &Value| v.as_str().unwrap().encode_utf16().collect::<Vec<u16>>();
        el[k] = match (op.len(), &el[k]) {
            (2, _) => op[1].clone(),
            (3, inner) if op[1].is_null() => {
                let mut inner = inner.clone();
                for o in op[2].as_array().unwrap() {
                    apply(&mut inner, o.as_array().unwrap());
                }
                inner
            }
            (3, Value::Array(a)) => json!([&a[..n(1)], op[2].as_array().unwrap().as_slice()].concat()),
            (3, old) => json!(String::from_utf16(&units(old)[..n(1)]).unwrap() + op[2].as_str().unwrap()),
            (4, old) => {
                let u = units(old);
                let (head, tail) = (String::from_utf16(&u[..n(1)]).unwrap(), String::from_utf16(&u[n(1) + n(2)..]).unwrap());
                json!(head + op[3].as_str().unwrap() + &tail)
            }
            _ => panic!("{op:?}"),
        };
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

    fn encoder(level: u32) -> Encoder {
        Encoder { level, ..Default::default() }
    }

    #[test]
    fn deltas_expand_to_the_full_turn() {
        for level in 1..=3 {
            let mut e = encoder(level);
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
            // only the first: never again on a timer
            assert_eq!(keys, [0], "{keys:?}");
        }
    }

    /// `edits` as the JSON a delta carries.
    fn edits_json(old: Value, new: Value, level: u32) -> Value {
        serde_json::to_value(edits(&old, &new, level)).unwrap()
    }

    #[test]
    fn a_growing_part_is_sent_as_its_new_end() {
        let mut e = encoder(2);
        let mut r = Reader::default();
        let t0 = Instant::now();
        // multi-unit characters before and at the cut, html that changes before its end
        let chunks = ["Ça ", "😀 ", "ğüş ", "**bold** ", "😁", "漢字 ", "done. "];
        let mut raw = String::new();
        let mut thought = String::new();
        for i in 0..14u64 {
            let c = chunks[i as usize % chunks.len()];
            if i < 7 { thought.push_str(c) } else { raw.push_str(c) }
            let mut parts = vec![json!({"kind": "thinking", "raw": thought, "html": format!("<p>{thought}</p>"), "redacted": false, "streaming": i < 7})];
            if i >= 7 {
                parts.push(text(&raw));
            }
            let t = turn("a1", parts, true);
            let json = e.encode("c1", &t, t0 + Duration::from_millis(45 * i));
            assert_eq!(r.read(&json).unwrap(), full("c1", &t), "event {i}: {json}");
            let p: Value = serde_json::from_str(&json).unwrap();
            if i > 0 && i != 7 {
                let grown = if i < 7 { 0 } else { 1 };
                let ops = p["turn"]["parts"][grown].as_array().unwrap_or_else(|| panic!("event {i}: {json}"));
                // only the new end, never the whole string again
                assert!(ops.iter().all(|op| op[2].as_str().unwrap().chars().count() <= c.chars().count() + 4), "event {i}: {json}");
            }
        }
        // the thinking part stops streaming at event 7: in protocol 2 a non-string change is sent whole
        assert_eq!(edits_json(json!({"a": "x", "b": true}), json!({"a": "xy", "b": false}), 2), Value::Null);
        assert_eq!(edits_json(json!({"a": "x"}), json!({"a": "x", "b": "y"}), 3), Value::Null);
        assert_eq!(edits_json(json!({"a": "x", "c": "z"}), json!({"a": "x", "b": "z"}), 3), Value::Null);
        // U+1F600 and U+1F601 share their first UTF-16 unit: the cut stays before the whole character
        assert_eq!(edits_json(json!({"a": "a😀b"}), json!({"a": "a😁b"}), 2), json!([["a", 1, "😁b"]]));
        assert_eq!(edits_json(json!({"a": "ğ😀b", "n": 1}), json!({"a": "ğ😀bc", "n": 1}), 2), json!([["a", 4, "c"]]));
    }

    #[test]
    fn protocol_3_edits_what_changed_inside_a_field() {
        // a code block closes: the attribute near its start goes, its last line is highlighted and text follows
        let body: String = (0..40).map(|i| format!("<span class=\"hl-k\">let</span> v{i} = {i};\n")).collect();
        let open = format!("<p>Intro ğ</p><div class=\"code-block\" data-lang=\"rust\" data-open><pre><code>{body}let las</code></pre></div>");
        let closed = format!("<p>Intro ğ</p><div class=\"code-block\" data-lang=\"rust\"><pre><code>{body}<span class=\"hl-k\">let</span> last;\n</code></pre></div><p>Then</p>");
        let ops = edits_json(json!({"html": open, "n": 1}), json!({"html": closed, "n": 1}), 3);
        let mut el = json!({"html": open, "n": 1});
        for op in ops.as_array().unwrap() {
            apply(&mut el, op.as_array().unwrap());
        }
        assert_eq!(el, json!({"html": closed, "n": 1}));
        let sent = ops.to_string().len();
        assert!(sent < 200, "{sent} bytes for a {} byte string: {ops}", closed.len());
        // where " data-open" is, in UTF-16 units (ğ is two bytes, one unit)
        let at = open.find(" data-open").unwrap() - 1;
        assert_eq!(ops[0], json!(["html", at, 10, ""]), "{ops}");
        // protocol 2 sends everything after the attribute again
        assert!(edits_json(json!({"html": open}), json!({"html": closed}), 2).to_string().len() > body.len());

        // a list that grows (the text's sources) and any other field that changes (a tool ends)
        let src = |n: usize| (0..n).map(|i| json!({"url": format!("https://e.com/{i}")})).collect::<Vec<_>>();
        assert_eq!(edits_json(json!({"sources": src(2)}), json!({"sources": src(3)}), 3), json!([["sources", 2, [src(3)[2]]]]));
        assert_eq!(edits_json(json!({"sources": src(2)}), json!({"sources": src(3)}), 2), Value::Null);
        let ops = edits_json(json!({"status": "running", "result": null, "input": {"command": "ls"}}), json!({"status": "done", "result": {"text": "ok"}, "input": {"command": "ls"}}), 3);
        assert_eq!(ops, json!([["result", {"text": "ok"}], ["status", 0, "done"]]));
        // a file being written: its content grows inside the tool's input
        let input = |c: &str| json!({"input": {"file_path": "a.rs", "content": c}});
        assert_eq!(edits_json(input("fn a"), input("fn a() {}"), 3), json!([["input", null, [["content", 4, "() {}"]]]]));
        assert_eq!(edits_json(json!({"input": {"file_path": "a.rs"}}), input("fn"), 3), json!([["input", {"file_path": "a.rs", "content": "fn"}]]));

        // strings that differ in many places: a few splices, then the rest as it is
        let old: String = (0..200).map(|i| format!("line {i:04} of a long text that goes on\n")).collect();
        let new = old.replace("0 of", "0 OF");
        let ops = edits_json(json!({"s": old}), json!({"s": new}), 3);
        let mut el = json!({"s": old});
        for op in ops.as_array().unwrap() {
            apply(&mut el, op.as_array().unwrap());
        }
        assert_eq!(el["s"], json!(new));
        assert_eq!(ops.as_array().unwrap().len(), MAX_SPLICES + 1, "{ops}");
    }

    #[test]
    fn protocol_1_readers_never_get_edits() {
        let mut e = Encoder::default();
        let t0 = Instant::now();
        for i in 0..10u64 {
            let t = turn("a1", vec![text("same"), text(&"grows ".repeat(i as usize + 1))], true);
            let p: Value = serde_json::from_str(&e.encode("c1", &t, t0 + Duration::from_millis(45 * i))).unwrap();
            assert!(p["turn"]["parts"].as_array().unwrap().iter().all(|v| v.is_null() || v.is_object()), "{p}");
        }
    }

    #[test]
    fn a_reader_that_missed_an_event_asks_and_gets_the_next_whole() {
        let mut e = encoder(3);
        let mut r = Reader::default();
        let t0 = Instant::now();
        let mut got = vec![];
        for i in 0..40u64 {
            let t = turn("a1", vec![text("same"), text(&format!("n{i}"))], true);
            let json = e.encode("c1", &t, t0 + Duration::from_millis(100 * i));
            if i == 3 {
                continue; // lost on the way
            }
            match r.read(&json) {
                Some(p) => {
                    assert_eq!(p, full("c1", &t), "event {i}");
                    got.push(i);
                }
                // what ui/js/turnstream.js does with a delta it can't expand
                None => e.end("c1"),
            }
        }
        // only the lost event and the one after it are missing
        assert_eq!(got, (0..40).filter(|i| ![3, 4].contains(i)).collect::<Vec<_>>());
    }

    #[test]
    fn a_page_that_starts_listening_gets_the_next_event_whole() {
        let mut e = encoder(2);
        let t0 = Instant::now();
        let mut r = Reader::default();
        let mut got = vec![];
        for i in 0..10u64 {
            if i == 5 {
                e.announce(3);
            }
            let t = turn("a1", vec![text("a"), text(&format!("b{i}"))], true);
            let json = e.encode("c1", &t, t0 + Duration::from_millis(100 * i));
            if i < 5 {
                continue; // the page wasn't listening yet
            }
            assert_eq!(r.read(&json).unwrap(), full("c1", &t));
            got.push(json.contains("\"base\""));
        }
        assert_eq!(got, [false, true, true, true, true]);
        assert_eq!(e.level, 3);
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
            let mut e = encoder(3);
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
            match &got {
                Some(g) => assert_eq!(*g, full(conv, t)),
                None => e.end(conv),
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

    /// A 5,000-character reply (alone, then after 1,500 characters of thinking)
    /// streamed 15 characters per 45 ms event, rendered as the engine does, in each protocol.
    /// `cargo test --no-default-features --lib turnstream::tests::measure_reply -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_reply() {
        use crate::render::{assistant_turn, ArtifactTracker, RenderCtx};
        let mut r = Rng(7);
        let mut reply = String::new();
        for i in 0.. {
            if reply.len() >= 6500 {
                break;
            }
            reply.push_str(&prose(&mut r, 30, 90));
            reply.push_str(if i % 3 == 1 { "\n\n```rust\n" } else { "\n\n" });
            if i % 3 == 1 {
                reply.push_str(&code(&mut r, 8, 12));
                reply.push_str("\n```\n\n");
            }
        }
        let (live, streaming_tools, mcp) = (HashMap::new(), std::collections::HashSet::new(), HashMap::new());
        for thought in [0, 1500] {
            let (thinking, answer): (String, String) = (reply.chars().take(thought).collect(), reply.chars().skip(thought).take(5000).collect());
            let end = thought + 5000;
            let mut turns = vec![];
            for (i, at) in (15..end).step_by(15).chain([end]).enumerate() {
                let mut content = vec![];
                if thought > 0 {
                    content.push(json!({"type": "thinking", "thinking": thinking.chars().take(at).collect::<String>()}));
                }
                if at > thought {
                    content.push(json!({"type": "text", "text": answer.chars().take(at - thought).collect::<String>()}));
                }
                let m = msg("a0".into(), "assistant", json!(content));
                let ctx = RenderCtx { math: false, live_tools: &live, streaming_tools: &streaming_tools, mcp_names: &mcp, streaming_msg: (at < end).then_some("a0") };
                turns.push((i, assistant_turn(&[&m], &ctx, &mut ArtifactTracker::default())));
            }
            let t0 = Instant::now();
            let full_bytes: usize = turns.iter().map(|(_, t)| serde_json::to_string(&full("c1", t)).unwrap().len()).sum();
            println!("{thought} thinking + 5000 reply characters, {} events; whole turn every event: {full_bytes} B", turns.len());
            for level in 1..=3 {
                let (mut e, mut rd) = (encoder(level), Reader::default());
                let (mut total, mut keys, mut key_bytes) = (0, 0, 0);
                for (i, t) in &turns {
                    let json = e.encode("c1", t, t0 + Duration::from_millis(45 * *i as u64));
                    assert_eq!(rd.read(&json).unwrap(), full("c1", t));
                    total += json.len();
                    if !json.contains("\"base\"") {
                        keys += 1;
                        key_bytes += json.len();
                    }
                }
                println!("  protocol {level}: {total} B ({:.1}%), {keys} keyframes {key_bytes} B", 100.0 * total as f64 / full_bytes as f64);
            }
        }
    }

    /// The reply of the evaluator's long-stream mock: `sections` times a
    /// heading, a paragraph, a 25-line Rust block and a 5-row table (30: 77,900 characters).
    fn long_reply(sections: usize) -> String {
        let mut out = String::new();
        for s in 0..sections {
            out.push_str(&format!("## Section {s}\n\nThis paragraph explains part {s} with **bold**, `inline code` and a link https://example.com/{s}.\n\n"));
            out.push_str("```rust\n");
            for i in 0..25 {
                out.push_str(&format!("fn f{s}_{i}(x: u64) -> u64 {{ let v: Vec<u64> = (0..x).map(|n| n * {i}).collect(); v.iter().sum() }}\n"));
            }
            out.push_str("```\n\n| a | b | c |\n|---|---|---|\n");
            for i in 0..5 {
                out.push_str(&format!("| {i} | {} | {} |\n", i * i, i * 3));
            }
            out.push('\n');
        }
        out
    }

    /// `text` streamed as the mock sends it (40 characters every 10.8 ms) and
    /// rendered as the engine does at each emit (at most every `every`): each
    /// event's turn with its time.
    fn stream_reply(text: &str, every: Duration, mut each: impl FnMut(&Value, Duration)) {
        use crate::render::{assistant_turn, ArtifactTracker, RenderCtx};
        let (live, streaming_tools, mcp) = (HashMap::new(), std::collections::HashSet::new(), HashMap::new());
        let chunk = Duration::from_micros(10_800);
        let (mut at, mut now, mut last) = (0, Duration::ZERO, None::<Duration>);
        while at < text.len() {
            at = (at + 40).min(text.len());
            now += chunk;
            if last.is_some_and(|l| now - l < every) && at < text.len() {
                continue;
            }
            last = Some(now);
            let m = msg("a0".into(), "assistant", json!([{"type": "text", "text": &text[..at]}]));
            let ctx = RenderCtx { math: false, live_tools: &live, streaming_tools: &streaming_tools, mcp_names: &mcp, streaming_msg: Some("a0") };
            let mut t = assistant_turn(&[&m], &ctx, &mut ArtifactTracker::default());
            t["siblings"] = json!({"count": 1, "index": 0, "ids": ["a0"]});
            each(&t, now);
        }
    }

    /// What streaming `text` costs: events, bytes, keyframes and their bytes,
    /// the largest delta. `keyframe_every` is what the encoder did before
    /// protocol 3: a keyframe at least every second.
    fn stream_cost(text: &str, level: u32, every: Duration, keyframe_every: Option<Duration>) -> [usize; 5] {
        let (mut e, mut rd) = (encoder(level), Reader::default());
        let t0 = Instant::now();
        let mut key_at = Duration::ZERO;
        let mut cost = [0; 5];
        stream_reply(text, every, |t, at| {
            if keyframe_every.is_some_and(|k| at - key_at >= k) {
                e.end("c1");
            }
            let json = e.encode("c1", t, t0 + at);
            assert_eq!(rd.read(&json).unwrap(), full("c1", t));
            cost[0] += 1;
            cost[1] += json.len();
            if json.contains("\"base\"") {
                cost[4] = cost[4].max(json.len());
            } else {
                (cost[2], cost[3], key_at) = (cost[2] + 1, cost[3] + json.len(), at);
            }
        });
        cost
    }

    #[test]
    fn a_long_reply_costs_about_its_size() {
        let text = long_reply(4);
        let [events, bytes, keys, _, largest] = stream_cost(&text, 3, crate::engine::EMIT_EVERY, None);
        assert_eq!(keys, 1, "only the first event is whole");
        assert!(bytes < 10 * text.len(), "{bytes} bytes for {} characters", text.len());
        assert!(largest < 4096, "a delta of {largest} bytes");
        // the mock's pace: 40 characters every 10.8 ms
        let secs = text.len() as f64 / 40.0 * 0.0108;
        assert!(events as f64 <= secs * 10.0 + 2.0, "{events} events in {secs:.1} s");
    }

    /// The evaluator's 77,900-character, 30-code-block reply through the encoder,
    /// as it was (protocol 2, a keyframe every second, an event every 45 ms) and is.
    /// `cargo test --no-default-features --lib turnstream::tests::measure_long_reply -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_long_reply() {
        let text = long_reply(30);
        let was = (2, Duration::from_millis(45), Some(Duration::from_secs(1)));
        for (name, (level, every, keyframes)) in [("before", was), ("after", (3, crate::engine::EMIT_EVERY, None))] {
            let [n, total, keys, key_bytes, largest] = stream_cost(&text, level, every, keyframes);
            println!(
                "{name}: {} characters, protocol {level}, an event every {every:?} at most: {n} events, {total} B ({:.1}x the text), \
                 {keys} keyframes {key_bytes} B, largest delta {largest} B",
                text.len(),
                total as f64 / text.len() as f64
            );
        }
    }

    /// Payloads for `ui/test/turnstream_delta.test.mjs`, each with the full
    /// payload the decoder must make of it (null: it must drop it): growing
    /// thinking and text with multi-unit characters, an attribute that goes
    /// near the start of the text, sources that grow, a tool that ends, two
    /// conversations, a lost event (the reader then asks for a keyframe), the
    /// end of a turn and `end`.
    fn delta_fixture() -> Vec<Value> {
        let mut e = encoder(3);
        let mut r = Reader::default();
        let t0 = Instant::now();
        let mut out = vec![];
        let mut n = 0u64;
        let mut emit = |conv: &str, t: &Value, lost: bool, e: &mut Encoder, r: &mut Reader, out: &mut Vec<Value>| {
            n += 1;
            let json = e.encode(conv, t, t0 + Duration::from_millis(100 * n));
            if lost {
                return;
            }
            let got = r.read(&json);
            match &got {
                Some(g) => assert_eq!(*g, full(conv, t)),
                None => e.end(conv),
            }
            out.push(json!({"payload": json, "full": got}));
        };
        let words = ["Ça ", "😀 ", "ğüş ", "<b>&amp;</b> ", "😁", "漢字 ", "\"q\"\n"];
        let (mut grown, mut thought, mut other) = (String::new(), String::new(), String::new());
        for i in 0..22usize {
            grown.push_str(words[i % words.len()]);
            if i < 6 {
                thought = grown.clone();
            }
            let mut parts = vec![json!({"kind": "thinking", "raw": thought, "html": format!("<p>{thought}</p>"), "redacted": false, "streaming": i < 6})];
            if i >= 6 {
                let said = &grown[thought.len()..];
                let mut t = text(said);
                t["html"] = json!(format!("<div class=\"answer\"{}><p>{said}</p></div>", if i < 16 { " data-open" } else { "" }));
                parts.push(t);
            }
            emit("ca", &turn("a1", parts, true), i == 10, &mut e, &mut r, &mut out);
            if i % 2 == 0 {
                other.push_str(words[words.len() - 1 - i % words.len()]);
                let mut said = text(&other);
                said["sources"] = json!((0..i / 4).map(|k| json!({"url": format!("https://e.com/{k}"), "domain": "e.com"})).collect::<Vec<_>>());
                let done = i >= 12;
                let mut call = tool("t1", if done { "done" } else { "running" }, if done { "ok" } else { "" });
                call["input"]["command"] = json!(format!("ls {}", "ğ".repeat(i.min(12))));
                emit("cb", &turn("b1", vec![text("fixed"), call, said], true), false, &mut e, &mut r, &mut out);
            }
        }
        emit("ca", &turn("a1", vec![text(&grown)], false), false, &mut e, &mut r, &mut out);
        e.end("cb");
        emit("cb", &turn("b1", vec![text("fixed"), text(&other)], true), false, &mut e, &mut r, &mut out);
        out
    }

    /// The JS test reads what the encoder writes today.
    /// `WRITE_FIXTURES=1 cargo test --no-default-features --lib turnstream` rewrites it.
    #[test]
    fn the_js_fixture_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/test/turnstream_delta.fixture.json");
        let events = delta_fixture();
        if std::env::var_os("WRITE_FIXTURES").is_some() {
            let lines: Vec<String> = events.iter().map(Value::to_string).collect();
            std::fs::write(&path, format!("[\n{}\n]\n", lines.join(",\n"))).unwrap();
        }
        let out = Value::Array(events);
        let disk: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(disk == out, "{} is stale: run with WRITE_FIXTURES=1", path.display());
        // it covers every kind of edit, dropped deltas and keyframes
        let payloads: Vec<&str> = out.as_array().unwrap().iter().map(|v| v["payload"].as_str().unwrap()).collect();
        let ops: Vec<Value> = payloads
            .iter()
            .flat_map(|p| serde_json::from_str::<Value>(p).unwrap()["turn"]["parts"].as_array().cloned().unwrap_or_default())
            .filter(|el| el.as_array().is_some_and(|ops| ops.iter().all(Value::is_array)))
            .flat_map(|el| el.as_array().unwrap().clone())
            .collect();
        let shape = |op: &Value| match op.as_array().unwrap().as_slice() {
            [_, _] => "set",
            [_, Value::Null, _] => "inner",
            [_, _, Value::Array(_)] => "items",
            [_, _, _] => "tail",
            _ => "splice",
        };
        for want in ["set", "inner", "tail", "items", "splice"] {
            assert!(ops.iter().any(|op| shape(op) == want), "no {want} edit");
        }
        assert!(out.as_array().unwrap().iter().any(|v| v["full"].is_null()));
        assert!(payloads.iter().filter(|p| !p.contains("\"base\"")).count() >= 4);
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

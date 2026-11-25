//! Builds the UI view of a conversation: turns for the current branch, with
//! branch (sibling) navigation info and any live streaming state overlaid.

use crate::artifacts::{self, Segment};
use crate::db::{Message, NodeInfo};
use crate::error::AppResult;
use crate::render::{self, ArtifactTracker, RenderCtx};
use crate::state::AppState;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub fn siblings_of(nodes: &[NodeInfo], msg_id: &str) -> Value {
    let Some(me) = nodes.iter().find(|n| n.id == msg_id) else {
        return json!({"index": 0, "count": 1, "ids": [msg_id]});
    };
    let is_user_turn = me.role == "user" && !me.is_tool_result;
    let sibs: Vec<&NodeInfo> = nodes
        .iter()
        .filter(|n| {
            n.parent_id == me.parent_id
                && n.role == me.role
                && (if is_user_turn { !n.is_tool_result } else { true })
        })
        .collect();
    let ids: Vec<String> = sibs.iter().map(|n| n.id.clone()).collect();
    let index = ids.iter().position(|i| i == msg_id).unwrap_or(0);
    json!({"index": index, "count": ids.len(), "ids": ids})
}

/// The nodes of a conversation by id and by (parent, role): the siblings of
/// every turn of a view (what `siblings_of` gives) in one pass over the
/// nodes, not one per turn (a long conversation's view was quadratic).
struct Siblings<'a> {
    by_id: HashMap<&'a str, &'a NodeInfo>,
    by_parent: HashMap<(Option<&'a str>, &'a str), Vec<&'a NodeInfo>>,
}

impl<'a> Siblings<'a> {
    fn new(nodes: &'a [NodeInfo]) -> Self {
        let mut by_id = HashMap::with_capacity(nodes.len());
        let mut by_parent: HashMap<(Option<&str>, &str), Vec<&NodeInfo>> = HashMap::new();
        for n in nodes {
            // the first node with an id, as a search from the start finds
            by_id.entry(n.id.as_str()).or_insert(n);
            by_parent.entry((n.parent_id.as_deref(), n.role.as_str())).or_default().push(n);
        }
        Siblings { by_id, by_parent }
    }

    fn of(&self, msg_id: &str) -> Value {
        let Some(me) = self.by_id.get(msg_id) else {
            return json!({"index": 0, "count": 1, "ids": [msg_id]});
        };
        let is_user_turn = me.role == "user" && !me.is_tool_result;
        let ids: Vec<String> = self
            .by_parent
            .get(&(me.parent_id.as_deref(), me.role.as_str()))
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|n| if is_user_turn { !n.is_tool_result } else { true })
            .map(|n| n.id.clone())
            .collect();
        let index = ids.iter().position(|i| i == msg_id).unwrap_or(0);
        json!({"index": index, "count": ids.len(), "ids": ids})
    }
}

/// Deepest descendant of `start`, following the most recent child each step.
pub fn latest_leaf(nodes: &[NodeInfo], start: &str) -> String {
    let mut cur = start.to_string();
    loop {
        let next = nodes
            .iter()
            .filter(|n| n.parent_id.as_deref() == Some(cur.as_str()))
            .max_by_key(|n| n.created_at);
        match next {
            Some(n) => cur = n.id.clone(),
            None => return cur,
        }
    }
}

/// The artifacts in the text of assistant messages, in order.
fn artifact_specs<'a>(msgs: impl IntoIterator<Item = &'a Message> + 'a) -> impl Iterator<Item = artifacts::ArtifactSpec> + 'a {
    msgs.into_iter().filter(|m| m.role == "assistant").flat_map(|m| {
        let mut text = String::new();
        for b in m.content.as_array().into_iter().flatten() {
            if b["type"] == "text" {
                text.push_str(b["text"].as_str().unwrap_or(""));
            }
        }
        artifacts::split(&text).into_iter().filter_map(|seg| match seg {
            Segment::Artifact(a) => Some(a),
            Segment::Markdown(_) => None,
        })
    })
}

/// Count artifact versions in messages (used to continue numbering).
pub fn tracker_for(msgs: &[Message]) -> ArtifactTracker {
    let mut t = ArtifactTracker::default();
    for a in artifact_specs(msgs) {
        t.bump(&a.identifier);
    }
    t
}

/// Which turns of the current branch a view carries: the last `limit` (all
/// without one); or, when `since` is still on the branch, those from it on
/// (what the UI shows already, with what came after); or, with `before`, up
/// to `limit` turns just before that one (none when it's gone).
#[derive(Default)]
pub struct Window {
    pub limit: Option<usize>,
    pub since: Option<String>,
    pub before: Option<String>,
}

/// What the UI draws of a page at once (turnwindow.js `PAGE_CHARS`): a page of long turns is cut
/// there, so the reply it parses on its main thread stays small; the turns before come as it scrolls up.
const PAGE_CHARS: usize = 250_000;

/// About how much a turn puts on screen: its rendered text (turnwindow.js `weight`).
fn weight(turn: &Value) -> usize {
    let parts = turn["parts"].as_array().map_or(&[][..], |p| p.as_slice());
    200 + parts.iter().map(|p| p["html"].as_str().or(p["text"].as_str()).map_or(0, str::len)).sum::<usize>()
}

/// Where the part of `turns` to send starts: as many of the last as PAGE_CHARS holds (one at
/// least), and never an answer without its question (turnwindow.js `pageStart`).
fn page_cut(turns: &[Value]) -> usize {
    let mut i = turns.len();
    let mut chars = 0;
    while i > 0 && (i == turns.len() || chars + weight(&turns[i - 1]) <= PAGE_CHARS) {
        i -= 1;
        chars += weight(&turns[i]);
    }
    if i > 0 && turns[i]["role"] == "assistant" && turns[i - 1]["role"] == "user" {
        i -= 1;
    }
    i
}

impl Window {
    fn range(&self, ids: &[&str]) -> std::ops::Range<usize> {
        let pos = |id: &Option<String>| id.as_deref().and_then(|x| ids.iter().position(|i| *i == x));
        let end = if self.before.is_some() { pos(&self.before).unwrap_or(0) } else { ids.len() };
        let start = match pos(&self.since) {
            Some(s) if s <= end => s,
            _ => self.limit.map_or(0, |n| end.saturating_sub(n)),
        };
        start..end
    }
}

pub async fn conversation_view(st: &AppState, conv_id: &str) -> AppResult<Value> {
    conversation_window(st, conv_id, &Window::default()).await
}

/// The view with only the turns of `window` rendered; `more` says whether
/// turns come before them, and `earlier_artifacts` lists those turns'
/// artifacts (the panel shows every version).
pub async fn conversation_window(st: &AppState, conv_id: &str, window: &Window) -> AppResult<Value> {
    let conv = st.db.conv_get(conv_id)?;
    let nodes = st.db.tree_nodes(conv_id)?;
    let mut path = match &conv.current_leaf {
        Some(leaf) => st.db.path_to(leaf)?,
        None => vec![],
    };
    let mut live = st.live.lock().get(conv_id).cloned();
    let empty_tools = HashMap::new();
    let empty_set = HashSet::new();
    if let Some(l) = live.as_mut() {
        if let Some(m) = path.iter_mut().find(|m| m.id == l.msg_id) {
            // moved: the copy taken from `st.live` is only needed here
            m.content = std::mem::take(&mut l.content);
        }
    }
    let mcp_names = st.mcp.name_map().await;
    let math = st.settings.read().latex_enabled;
    let ctx = RenderCtx {
        math,
        live_tools: live.as_ref().map(|l| &l.live_tools).unwrap_or(&empty_tools),
        streaming_tools: live.as_ref().map(|l| &l.streaming_tools).unwrap_or(&empty_set),
        mcp_names: &mcp_names,
        streaming_msg: live.as_ref().map(|l| l.msg_id.as_str()),
    };
    let groups = render::turn_groups(&path);
    let ids: Vec<&str> = groups.iter().map(|g| g[0].id.as_str()).collect();
    let mut range = window.range(&ids);
    // numbering goes on after the turns left out above
    let mut tracker = ArtifactTracker::default();
    for a in artifact_specs(groups[..range.start].iter().flatten().copied()) {
        tracker.next(&a.identifier);
    }
    let mut turns: Vec<Value> = groups[range.clone()].iter().map(|g| render::build_turn(g, &ctx, &mut tracker)).collect();
    // a page (not a refresh from `since`, nor the whole view) is only what the UI draws of it at once
    if window.limit.is_some() && window.since.is_none() {
        let cut = page_cut(&turns);
        turns.drain(..cut);
        range.start += cut;
    }
    let mut numbers = ArtifactTracker::default();
    let earlier: Vec<Value> = artifact_specs(groups[..range.start].iter().flatten().copied())
        .map(|a| {
            let v = numbers.next(&a.identifier);
            render::artifact_json(a, v)
        })
        .collect();
    let more = range.start > 0;
    let siblings = Siblings::new(&nodes);
    for t in turns.iter_mut() {
        let s = siblings.of(t["id"].as_str().unwrap_or(""));
        t["siblings"] = s;
    }
    let running = st.runs.lock().contains_key(conv_id);
    let pending: Vec<Value> = st
        .pending_perms
        .lock()
        .values()
        .filter(|p| p["conversation_id"] == conv_id)
        .cloned()
        .collect();
    // a Claude Code turn idle while a workflow runs: a message goes straight in
    let waiting = st.cc_ctl.lock().get(conv_id).is_some_and(|c| c.waiting);
    Ok(json!({
        "conversation": conv,
        "turns": turns,
        "more": more,
        "earlier_artifacts": earlier,
        "running": running,
        "waiting": waiting,
        "pending_permissions": pending,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_of_long_turns_is_cut_to_what_is_drawn_at_once() {
        let turn = |role: &str, chars: usize| json!({"role": role, "parts": [{"html": "x".repeat(chars)}]});
        let short: Vec<Value> = (0..40).map(|i| turn(if i % 2 == 0 { "user" } else { "assistant" }, 1000)).collect();
        assert_eq!(page_cut(&short), 0, "a page of ordinary turns goes whole");
        // 27 answers of ~90k characters (120 highlighted code blocks): 2 fit, with their questions
        let long: Vec<Value> = (0..54).map(|i| if i % 2 == 0 { turn("user", 50) } else { turn("assistant", 90_000) }).collect();
        assert_eq!(page_cut(&long), 50);
        assert_eq!(long[50]["role"], "user", "an answer comes with its question");
        // one turn larger than a page still goes
        assert_eq!(page_cut(&[turn("assistant", 900_000)]), 0);
        assert_eq!(page_cut(&[]), 0);
        // the text of a part that has no html counts too
        assert_eq!(weight(&json!({"parts": [{"text": "abc"}, {"html": "de"}]})), 205);
    }

    #[test]
    fn indexed_siblings_match_the_search() {
        // a tree with edits (siblings), tool result rows and a regenerated reply
        let spec = [
            ("u1", None, "user", false),
            ("a1", Some("u1"), "assistant", false),
            ("t1", Some("a1"), "user", true),
            ("a2", Some("t1"), "assistant", false),
            ("u2", Some("a2"), "user", false),
            ("u2b", Some("a2"), "user", false),
            ("t2", Some("a2"), "user", true),
            ("a3", Some("u2"), "assistant", false),
            ("a3b", Some("u2"), "assistant", false),
            ("u0", None, "user", false),
        ];
        let nodes: Vec<NodeInfo> = spec
            .iter()
            .enumerate()
            .map(|(i, (id, parent, role, tr))| NodeInfo {
                id: id.to_string(),
                parent_id: parent.map(String::from),
                role: role.to_string(),
                is_tool_result: *tr,
                created_at: i as i64,
            })
            .collect();
        let idx = Siblings::new(&nodes);
        for (id, ..) in spec.iter().chain([("missing", None, "user", false)].iter()) {
            assert_eq!(idx.of(id), siblings_of(&nodes, id), "{id}");
        }
    }

    #[test]
    fn window_ranges() {
        let ids = ["a", "b", "c", "d", "e", "f"];
        let w = |limit: Option<usize>, since: Option<&str>, before: Option<&str>| {
            Window { limit, since: since.map(String::from), before: before.map(String::from) }.range(&ids)
        };
        assert_eq!(w(None, None, None), 0..6, "everything");
        assert_eq!(w(Some(2), None, None), 4..6, "the last turns");
        assert_eq!(w(Some(10), None, None), 0..6);
        assert_eq!(w(Some(2), Some("c"), None), 2..6, "from a turn on, however many");
        assert_eq!(w(Some(2), Some("gone"), None), 4..6, "a turn no longer on the branch: the last ones");
        assert_eq!(w(Some(2), None, Some("e")), 2..4, "the ones before a turn");
        assert_eq!(w(Some(5), None, Some("c")), 0..2);
        assert_eq!(w(Some(2), None, Some("gone")), 0..0, "nothing before a turn that's gone");
        assert_eq!(w(None, Some("e"), Some("c")), 0..2, "since after before: before wins");
    }

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("cr-view-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        AppState::new(crate::db::Db::open_in_memory().unwrap(), dir.clone(), dir.join("mcp.json"))
    }

    /// A chat of a question and an answer per item of `answers`; messages
    /// are `user-<i>` and `assistant-<i>`.
    fn chat(st: &AppState, answers: &[String]) -> String {
        let conv = st.db.conv_create("chat", "claude-opus-5-5", None, None, &json!({})).unwrap();
        let mut parent: Option<String> = None;
        for (i, a) in answers.iter().enumerate() {
            for (role, text) in [("user", format!("Question {i}")), ("assistant", a.clone())] {
                let m = Message {
                    id: format!("{role}-{i}"),
                    conversation_id: conv.id.clone(),
                    parent_id: parent.clone(),
                    role: role.into(),
                    content: json!([{"type": "text", "text": text}]),
                    meta: json!({}),
                    created_at: i as i64,
                };
                st.db.msg_insert(&m).unwrap();
                parent = Some(m.id);
            }
        }
        st.db.conv_set_leaf(&conv.id, parent.as_deref()).unwrap();
        conv.id
    }

    #[tokio::test]
    async fn a_window_is_the_end_of_the_whole_view() {
        let st = test_state();
        let art = |v: &str| format!("Here. <antArtifact identifier=\"page\" type=\"text/html\" title=\"Page\"><p>{v}</p></antArtifact>");
        let id = chat(&st, &[art("one"), "Plain **answer**.".to_string(), "Plain.".to_string(), art("two")]);

        let full = conversation_view(&st, &id).await.unwrap();
        let all = full["turns"].as_array().unwrap();
        assert_eq!((all.len(), full["more"].as_bool()), (8, Some(false)));
        let w = Window { limit: Some(2), ..Default::default() };
        let tail = conversation_window(&st, &id, &w).await.unwrap();
        assert_eq!(tail["turns"].as_array().unwrap()[..], all[6..], "the same turns");
        assert_eq!(tail["turns"][1]["artifacts"][0]["version"], 2, "numbering goes on after the turns left out");
        assert_eq!(tail["more"], true);
        let earlier: Vec<&Value> = all[..6].iter().flat_map(|t| t["artifacts"].as_array().into_iter().flatten()).collect();
        assert_eq!(earlier.len(), 1, "the first version is above the window");
        assert_eq!(tail["earlier_artifacts"].as_array().unwrap().iter().collect::<Vec<_>>(), earlier);
        // scrolling up from the window's first turn, then a reload from a turn on
        let up = Window { limit: Some(3), before: Some("user-3".into()), ..Default::default() };
        assert_eq!(conversation_window(&st, &id, &up).await.unwrap()["turns"].as_array().unwrap()[..], all[3..6]);
        let since = Window { limit: Some(3), since: Some("user-1".into()), ..Default::default() };
        assert_eq!(conversation_window(&st, &id, &since).await.unwrap()["turns"].as_array().unwrap()[..], all[2..]);
    }

    /// Opening a 600-message chat (Markdown answers with a list, a table and
    /// highlighted code), nothing in the render caches: the whole view
    /// against the last 40 turns.
    /// `cargo test --no-default-features --lib bench_window -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn bench_window() {
        let answers = |tag: &str| -> Vec<String> {
            (0..300)
                .map(|i| {
                    let code: String = (0..30).map(|j| format!("    let v{j} = compute({j}, \"{tag}-{i}\").await?;\n")).collect();
                    let rows: String = (0..5).map(|j| format!("| row{j} | {} | text |\n", i * j)).collect();
                    format!("## Section {i}\n\nAn answer with `code`.\n\n- one\n- two\n\n| Name | Value | Note |\n|---|---|---|\n{rows}\n```rust\nasync fn run_{i}() -> Result<()> {{\n{code}}}\n```\n")
                })
                .collect()
        };
        // two chats with different code, so neither finds the other's in the render caches
        let (sa, sb) = (test_state(), test_state());
        let (a, b) = (chat(&sa, &answers("a")), chat(&sb, &answers("b")));
        let t = std::time::Instant::now();
        let full = conversation_view(&sa, &a).await.unwrap();
        let full_ms = t.elapsed();
        let t = std::time::Instant::now();
        let win = conversation_window(&sb, &b, &Window { limit: Some(40), ..Default::default() }).await.unwrap();
        let win_ms = t.elapsed();
        let size = |v: &Value| serde_json::to_string(v).unwrap().len() / 1024;
        eprintln!("600 messages: whole view {full_ms:?}, {} KB; last 40 turns {win_ms:?}, {} KB", size(&full), size(&win));
    }

    /// The siblings of every turn of a long conversation (2000 rows, a
    /// tool call per turn): searching per turn against the index.
    #[test]
    #[ignore]
    fn bench_siblings() {
        let mut nodes = vec![];
        let mut parent: Option<String> = None;
        for i in 0..500 {
            for (role, tr) in [("user", false), ("assistant", false), ("user", true), ("assistant", false)] {
                let id = format!("{role}-{tr}-{i}");
                nodes.push(NodeInfo { id: id.clone(), parent_id: parent.clone(), role: role.into(), is_tool_result: tr, created_at: i });
                parent = Some(id);
            }
        }
        let turns: Vec<String> = nodes.iter().filter(|n| !n.is_tool_result).map(|n| n.id.clone()).step_by(2).collect();
        let t0 = std::time::Instant::now();
        let a: Vec<Value> = turns.iter().map(|id| siblings_of(&nodes, id)).collect();
        let t1 = std::time::Instant::now();
        let idx = Siblings::new(&nodes);
        let b: Vec<Value> = turns.iter().map(|id| idx.of(id)).collect();
        let t2 = std::time::Instant::now();
        assert_eq!(a, b);
        eprintln!("{} nodes, {} turns: search {:?}, index {:?}", nodes.len(), turns.len(), t1 - t0, t2 - t1);
    }
}

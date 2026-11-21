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

/// Count artifact versions in messages (used to continue numbering).
pub fn tracker_for(msgs: &[Message]) -> ArtifactTracker {
    let mut t = ArtifactTracker::default();
    for m in msgs.iter().filter(|m| m.role == "assistant") {
        let mut text = String::new();
        for b in m.content.as_array().into_iter().flatten() {
            if b["type"] == "text" {
                text.push_str(b["text"].as_str().unwrap_or(""));
            }
        }
        for seg in artifacts::split(&text) {
            if let Segment::Artifact(a) = seg {
                t.bump(&a.identifier);
            }
        }
    }
    t
}

pub async fn conversation_view(st: &AppState, conv_id: &str) -> AppResult<Value> {
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
    let mut tracker = ArtifactTracker::default();
    let mut turns = render::build_turns(&path, &ctx, &mut tracker);
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
    Ok(json!({
        "conversation": conv,
        "turns": turns,
        "running": running,
        "pending_permissions": pending,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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

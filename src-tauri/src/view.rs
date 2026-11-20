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
    let live = st.live.lock().get(conv_id).cloned();
    let empty_tools = HashMap::new();
    let empty_set = HashSet::new();
    if let Some(l) = &live {
        if let Some(m) = path.iter_mut().find(|m| m.id == l.msg_id) {
            m.content = l.content.clone();
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
    for t in turns.iter_mut() {
        let id = t["id"].as_str().unwrap_or("").to_string();
        t["siblings"] = siblings_of(&nodes, &id);
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

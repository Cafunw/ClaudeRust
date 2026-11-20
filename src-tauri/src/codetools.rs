//! Native implementations of the coding-agent tools used in Code mode:
//! Bash, Read, Write, Edit, Glob, Grep, TodoWrite (and ExitPlanMode, which
//! the engine handles because it needs the user).

use crate::attachments;
use crate::error::{AppError, AppResult};
use crate::util::truncate_middle;
use globset::{Glob, GlobSetBuilder};
use parking_lot::Mutex;
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

pub const TOOL_NAMES: &[&str] = &["Bash", "BashOutput", "KillShell", "Read", "Write", "Edit", "Glob", "Grep", "TodoWrite", "ExitPlanMode"];

/// Receives live output of a running command (the text so far, tail-capped).
pub type ProgressFn = Arc<dyn Fn(&str) + Send + Sync>;
const MAX_OUTPUT_CHARS: usize = 30_000;

pub struct CodeSession {
    pub root: PathBuf,
    pub cwd: Mutex<PathBuf>,
    pub read_files: Mutex<HashSet<PathBuf>>,
    /// The conversation this session belongs to ("" outside a conversation).
    pub conv_id: String,
    /// Registry for background commands (absent in previews and tests).
    pub shells: Option<Arc<crate::shells::Shells>>,
    /// Content of each file before this session first changed it (`None`:
    /// the file didn't exist). The baseline for diffs outside git.
    pub originals: Mutex<std::collections::HashMap<PathBuf, Option<Vec<u8>>>>,
}

impl CodeSession {
    pub fn new(root: PathBuf) -> Self {
        CodeSession {
            cwd: Mutex::new(root.clone()),
            root,
            read_files: Mutex::new(HashSet::new()),
            conv_id: String::new(),
            shells: None,
            originals: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Record a file's current content before its first change in this session.
    pub fn remember_original(&self, p: &Path) {
        let mut m = self.originals.lock();
        if m.contains_key(p) {
            return;
        }
        let content = match std::fs::metadata(p) {
            Ok(md) if md.is_file() && md.len() <= 20 * 1024 * 1024 => std::fs::read(p).ok(),
            _ => None,
        };
        m.insert(p.to_path_buf(), content);
    }

    pub fn for_conversation(root: PathBuf, conv_id: &str, shells: Arc<crate::shells::Shells>) -> Self {
        let mut s = CodeSession::new(root);
        s.conv_id = conv_id.to_string();
        s.shells = Some(shells);
        s
    }

    fn resolve(&self, p: &str) -> PathBuf {
        let p = p.trim();
        let expanded = if let Some(rest) = p.strip_prefix("~/") {
            dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(p))
        } else {
            PathBuf::from(p)
        };
        if expanded.is_absolute() {
            expanded
        } else {
            self.cwd.lock().join(expanded)
        }
    }
}

pub struct ToolOutput {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolOutput {
    fn text(s: impl Into<String>) -> Self {
        ToolOutput {
            content: vec![json!({"type": "text", "text": s.into()})],
            is_error: false,
        }
    }
    fn err(s: impl Into<String>) -> Self {
        ToolOutput {
            content: vec![json!({"type": "text", "text": s.into()})],
            is_error: true,
        }
    }
}

pub fn definitions(eager: bool) -> Vec<Value> {
    let mut defs = vec![
        json!({
            "name": "Bash",
            "description": "Run a shell command in the project's working directory and return its output. The working directory persists between calls. Use for builds, tests, git and other CLI tools. Do not use it for reading, searching or editing files when Read, Grep, Glob or Edit can do the job. Commands time out after 2 minutes by default (max 10 minutes). Quote paths that contain spaces.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to run."},
                    "description": {"type": "string", "description": "Short description (5-10 words) of what the command does."},
                    "timeout": {"type": "number", "description": "Optional timeout in milliseconds (max 600000)."},
                    "run_in_background": {"type": "boolean", "description": "Run the command in the background and return immediately with a shell id. Use for dev servers, watchers and other long-running commands; read its output later with BashOutput and stop it with KillShell."}
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "BashOutput",
            "description": "Get new output from a background shell started with Bash run_in_background, plus whether it is still running. Returns only output produced since the last call.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "bash_id": {"type": "string", "description": "The id of the background shell."},
                    "filter": {"type": "string", "description": "Optional regular expression; only matching lines are returned."}
                },
                "required": ["bash_id"]
            }
        }),
        json!({
            "name": "KillShell",
            "description": "Stop a background shell (and everything it started) by its id.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "shell_id": {"type": "string", "description": "The id of the background shell to stop."}
                },
                "required": ["shell_id"]
            }
        }),
        json!({
            "name": "Read",
            "description": "Read a file from the local filesystem. Returns up to 2000 lines by default, numbered like `cat -n`; use offset and limit for long files. Also reads images (returned visually) and PDFs. Read a file before editing it.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "Absolute path (or path relative to the working directory)."},
                    "offset": {"type": "number", "description": "1-based line number to start from."},
                    "limit": {"type": "number", "description": "Number of lines to read."}
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "Write",
            "description": "Write a file, replacing it if it exists (read existing files first). Prefer Edit for changes to existing files. Parent directories are created as needed.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "Absolute path of the file to write."},
                    "content": {"type": "string", "description": "Full file contents."}
                },
                "required": ["file_path", "content"]
            }
        }),
        json!({
            "name": "Edit",
            "description": "Replace an exact string in a file. old_string must match the file exactly (including indentation) and be unique unless replace_all is true. Read the file first.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "Absolute path of the file to modify."},
                    "old_string": {"type": "string", "description": "Exact text to replace."},
                    "new_string": {"type": "string", "description": "Replacement text."},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence (default false)."}
                },
                "required": ["file_path", "old_string", "new_string"]
            }
        }),
        json!({
            "name": "Glob",
            "description": "Find files by glob pattern (e.g. \"**/*.rs\", \"src/**/*.{ts,tsx}\"), respecting .gitignore. Returns paths sorted by modification time, newest first.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern."},
                    "path": {"type": "string", "description": "Directory to search (defaults to the working directory)."}
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "Grep",
            "description": "Search file contents with a regular expression (Rust regex syntax), respecting .gitignore. output_mode: \"files_with_matches\" (default, file paths), \"content\" (matching lines), or \"count\".",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression to search for."},
                    "path": {"type": "string", "description": "File or directory to search (defaults to the working directory)."},
                    "glob": {"type": "string", "description": "Only search files matching this glob, e.g. \"*.js\"."},
                    "type": {"type": "string", "description": "File type filter, e.g. rust, js, ts, py, go, java."},
                    "output_mode": {"type": "string", "enum": ["content", "files_with_matches", "count"]},
                    "-i": {"type": "boolean", "description": "Case insensitive."},
                    "-n": {"type": "boolean", "description": "Show line numbers (content mode, default true)."},
                    "-A": {"type": "number", "description": "Lines of context after each match."},
                    "-B": {"type": "number", "description": "Lines of context before each match."},
                    "-C": {"type": "number", "description": "Lines of context around each match."},
                    "multiline": {"type": "boolean", "description": "Let patterns span lines."},
                    "head_limit": {"type": "number", "description": "Limit output to the first N lines or entries."}
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "TodoWrite",
            "description": "Create or update the task list for the current work. Use it for tasks with 3 or more steps. Send the complete list every time. Keep exactly one item in_progress while working and mark items completed as soon as they are done.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string", "description": "Imperative description, e.g. \"Run tests\"."},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                                "activeForm": {"type": "string", "description": "Present continuous form, e.g. \"Running tests\"."}
                            },
                            "required": ["content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }
        }),
        json!({
            "name": "ExitPlanMode",
            "description": "Only in plan mode: present your implementation plan to the user for approval. If they approve, plan mode ends and you can start making changes.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "plan": {"type": "string", "description": "The plan, in concise markdown."}
                },
                "required": ["plan"]
            }
        }),
    ];
    if eager {
        for d in defs.iter_mut() {
            d["eager_input_streaming"] = json!(true);
        }
    }
    defs
}

pub fn is_code_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

pub const PLAN_MCP_DENIED: &str = "Plan mode is active: only read-only tools can run until the user approves a plan (call ExitPlanMode).";

/// Why a command is too risky to run without asking in Auto mode, if it is.
pub fn bash_risk(cmd: &str, s: Option<&CodeSession>) -> Option<&'static str> {
    let lower = cmd.to_ascii_lowercase();
    let padded = format!(" {} ", lower.replace(['\n', ';', '&', '|', '(', ')'], " ; "));
    let has = |w: &str| padded.contains(w);
    const RULES: &[(&str, &str)] = &[
        (" sudo ", "runs as administrator"),
        (" su ", "switches user"),
        (" doas ", "runs as administrator"),
        (" git push", "pushes to a remote"),
        (" git reset --hard", "discards commits or changes"),
        (" git clean ", "deletes untracked files"),
        (" git checkout -- ", "discards changes"),
        (" git restore ", "discards changes"),
        (" git rebase", "rewrites history"),
        (" git branch -d", "deletes a branch"),
        (" git branch -D", "deletes a branch"),
        (" git stash drop", "deletes stashed work"),
        (" git commit --amend", "rewrites history"),
        (" --force", "forces an operation"),
        (" npm publish", "publishes a package"),
        (" cargo publish", "publishes a crate"),
        (" twine upload", "publishes a package"),
        (" docker push", "publishes an image"),
        (" mkfs", "formats a disk"),
        (" dd ", "writes raw data"),
        (" shutdown", "shuts the machine down"),
        (" reboot", "restarts the machine"),
        (" kill -9", "force-kills processes"),
        (" killall ", "kills processes"),
        (" pkill ", "kills processes"),
        (" chmod -r", "changes permissions recursively"),
        (" chown -r", "changes ownership recursively"),
        (" ssh ", "connects to another machine"),
        (" scp ", "copies to another machine"),
        (" rsync ", "syncs files, possibly remotely"),
        (" curl ", "makes network requests"),
        (" wget ", "downloads from the network"),
        (" crontab", "changes scheduled jobs"),
        (" systemctl ", "changes system services"),
        (" drop table", "deletes database data"),
        (" drop database", "deletes a database"),
        (" truncate ", "deletes data"),
        (" eval ", "runs dynamically built code"),
    ];
    for (needle, why) in RULES {
        if has(&needle.to_ascii_lowercase()) {
            return Some(why);
        }
    }
    // rm: recursive or forced deletes, or deletes outside the project
    if let Some(cmds) = split_simple_commands(cmd) {
        for w in &cmds {
            let first = w.first().map(|x| x.as_str()).unwrap_or("");
            if matches!(first, "rm" | "rmdir" | "shred" | "unlink" | "mv") {
                let recursive = w.iter().any(|a| a.starts_with('-') && (a.contains('r') || a.contains('R') || a.contains('f')));
                let outside = match s {
                    Some(sess) => w.iter().skip(1).filter(|a| !a.starts_with('-')).any(|a| !sess.is_inside(&sess.resolve(a))),
                    None => true,
                };
                if outside || (first == "rm" && recursive && w.iter().any(|a| a == "/" || a == "~" || a == "*" || a == "." || a == "..")) {
                    return Some("deletes or moves files outside the project");
                }
            }
        }
    } else if lower.contains("rm ") && (lower.contains(" -rf") || lower.contains(" -fr")) {
        return Some("deletes files recursively");
    }
    // writes through redirection to absolute paths outside the project
    if let Some(sess) = s {
        for part in lower.split('>').skip(1) {
            let target = part.trim_start_matches('>').split_whitespace().next().unwrap_or("");
            if target.starts_with('/') && !target.starts_with("/dev/null") && !target.starts_with("/tmp/") {
                let p = PathBuf::from(target);
                if !sess.is_inside(&p) {
                    return Some("writes a file outside the project");
                }
            }
        }
    }
    None
}

/// Files Auto mode should still ask about even inside the project.
fn sensitive_path(p: &Path) -> bool {
    let s = p.to_string_lossy().to_ascii_lowercase();
    let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    s.contains("/.git/") || s.contains("\\.git\\") || name == ".env" || name.starts_with(".env.") || name.ends_with(".pem") || name.ends_with(".key")
        || name == "id_rsa" || name == "id_ed25519" || name == ".npmrc" || name == ".pypirc" || name == "credentials" || name == "credentials.json"
}

#[derive(Debug, PartialEq)]
pub enum Perm {
    Allow,
    Ask,
    Deny(String),
}

const SAFE_CMDS: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "wc", "echo", "which", "whoami", "date", "file", "stat", "du", "df", "tree",
    "grep", "rg", "uname", "basename", "dirname", "realpath", "sort", "uniq", "diff", "type",
];
const SAFE_GIT: &[&str] = &["status", "diff", "log", "show", "branch", "rev-parse", "ls-files", "blame", "remote", "describe", "shortlog", "tag"];

/// Split a shell command into simple commands (argument vectors). Returns
/// `None` for anything we can't reason about safely: substitutions,
/// redirections, background jobs, ANSI-C quoting, env assignments, or
/// quoting that doesn't parse.
pub fn split_simple_commands(cmd: &str) -> Option<Vec<Vec<String>>> {
    for bad in ["$(", "`", "<(", ">(", "${", "$'", ">", "<", "\\\n"] {
        if cmd.contains(bad) {
            return None;
        }
    }
    let norm = cmd.replace("&&", "\u{1}").replace("||", "\u{1}");
    if norm.contains('&') {
        return None;
    }
    let norm = norm.replace(['|', ';', '\n'], "\u{1}");
    let mut out = vec![];
    for part in norm.split('\u{1}') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let words = shell_words::split(part).ok()?;
        if words.is_empty() {
            continue;
        }
        if words[0].contains('=') {
            return None;
        }
        out.push(words);
    }
    Some(out)
}

fn simple_is_safe(w: &[String]) -> bool {
    let first = w[0].as_str();
    let args = &w[1..];
    match first {
        "git" => {
            let Some(sub) = args.first() else { return false };
            if !SAFE_GIT.contains(&sub.as_str()) {
                return false;
            }
            if args.iter().any(|a| a.starts_with("--output") || a == "--ext-diff" || a.starts_with("--exec") || a.starts_with("-c")) {
                return false;
            }
            let listing: &[&str] = match sub.as_str() {
                "branch" => &["-a", "--all", "-r", "--remotes", "-v", "-vv", "--list", "--show-current", "--no-color", "--merged", "--no-merged"],
                "tag" => &["-l", "--list", "-n"],
                "remote" => &["-v", "--verbose"],
                _ => return true,
            };
            args[1..].iter().all(|a| listing.contains(&a.as_str()))
        }
        "find" => !args.iter().any(|a| {
            matches!(a.as_str(), "-exec" | "-execdir" | "-delete" | "-ok" | "-okdir" | "-fprint" | "-fprint0" | "-fprintf" | "-fls")
        }),
        "rg" => !args.iter().any(|a| a.starts_with("--pre")),
        "sort" => !args.iter().any(|a| a.starts_with("--output") || (a.starts_with("-o")) || (a.starts_with('-') && !a.starts_with("--") && a.contains('o'))),
        "tree" => !args.iter().any(|a| a.starts_with("-o")),
        "date" => !args.iter().any(|a| a == "-s" || a.starts_with("--set")),
        other => SAFE_CMDS.contains(&other),
    }
}

/// True when every argument that names a path stays inside the project.
fn args_inside(w: &[String], s: Option<&CodeSession>) -> bool {
    let Some(s) = s else { return true };
    w[1..].iter().all(|a| {
        let looks_like_path = a.starts_with('/') || a.starts_with('~') || a.contains("..") || (cfg!(windows) && a.contains(':'));
        !looks_like_path || s.is_inside(&s.resolve(a.trim_start_matches('=' )))
    })
}

pub fn is_safe_command(cmd: &str) -> bool {
    is_safe_command_in(cmd, None)
}

fn is_safe_command_in(cmd: &str, s: Option<&CodeSession>) -> bool {
    match split_simple_commands(cmd) {
        Some(parts) => !parts.is_empty() && parts.iter().all(|w| simple_is_safe(w) && args_inside(w, s)),
        None => false,
    }
}

fn prefix_of(w: &[String]) -> String {
    let multi = ["git", "npm", "pnpm", "yarn", "bun", "cargo", "go", "make", "docker", "kubectl", "pip", "uv", "poetry", "dotnet", "mvn", "gradle", "python", "python3", "node", "npx"];
    match w {
        [] => String::new(),
        [a] => a.clone(),
        [a, b, ..] => {
            if multi.contains(&a.as_str()) && !b.starts_with('-') {
                format!("{a} {b}")
            } else {
                a.clone()
            }
        }
    }
}

/// Prefix used for "allow for this session" rules on Bash commands.
pub fn bash_rule_prefix(cmd: &str) -> String {
    split_simple_commands(cmd)
        .and_then(|p| p.into_iter().next())
        .map(|w| prefix_of(&w))
        .unwrap_or_default()
}

fn bash_allowed_by_rules(cmd: &str, rules: &[String], s: Option<&CodeSession>) -> bool {
    let Some(parts) = split_simple_commands(cmd) else { return false };
    !parts.is_empty()
        && parts.iter().all(|w| {
            (simple_is_safe(w) && args_inside(w, s)) || rules.iter().any(|r| r == &format!("Bash({})", prefix_of(w)))
        })
}

fn path_rule_allows(rules: &[String], p: &Path) -> bool {
    let target = canonical_or_self(p);
    rules.iter().any(|r| {
        r.strip_prefix("Path(")
            .and_then(|x| x.strip_suffix(')'))
            .map(|d| target.starts_with(d))
            .unwrap_or(false)
    })
}

fn canonical_or_self(p: &Path) -> PathBuf {
    // canonicalize the nearest existing ancestor and re-append the rest
    let mut cur = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = vec![];
    loop {
        if let Ok(c) = cur.canonicalize() {
            let mut full = c;
            for r in rest.iter().rev() {
                full.push(r);
            }
            return full;
        }
        match (cur.file_name().map(|n| n.to_os_string()), cur.parent().map(|x| x.to_path_buf())) {
            (Some(n), Some(par)) => {
                rest.push(n);
                cur = par;
            }
            _ => return p.to_path_buf(),
        }
    }
}

impl CodeSession {
    /// Is `p` (absolute) inside the session's project folder? Paths whose
    /// unresolved part contains `..` count as outside.
    pub fn is_inside(&self, p: &Path) -> bool {
        if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) && !p.exists() {
            return false;
        }
        let root = canonical_or_self(&self.root);
        canonical_or_self(p).starts_with(root)
    }
}

fn input_path(s: &CodeSession, input: &Value, key: &str) -> PathBuf {
    match input[key].as_str().filter(|p| !p.is_empty()) {
        Some(p) => s.resolve(p),
        None => s.cwd.lock().clone(),
    }
}

pub fn check_permission(mode: &str, name: &str, input: &Value, rules: &[String], s: Option<&CodeSession>) -> Perm {
    let bypass = mode == "bypassPermissions";
    let outside = |key: &str| -> bool {
        match s {
            Some(s) => {
                let p = input_path(s, input, key);
                !s.is_inside(&p) && !path_rule_allows(rules, &p)
            }
            None => false,
        }
    };
    match name {
        "TodoWrite" | "ExitPlanMode" | "BashOutput" | "KillShell" => Perm::Allow,
        "Read" | "Glob" | "Grep" => {
            let key = if name == "Read" { "file_path" } else { "path" };
            if !bypass && outside(key) {
                Perm::Ask
            } else {
                Perm::Allow
            }
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            if mode == "plan" {
                return Perm::Deny("Plan mode is active: files can't be modified until the user approves a plan (call ExitPlanMode).".into());
            }
            if bypass {
                return Perm::Allow;
            }
            let key = if name == "NotebookEdit" { "notebook_path" } else { "file_path" };
            if outside(key) {
                return Perm::Ask;
            }
            if mode == "auto" {
                let sensitive = s.map(|s| sensitive_path(&input_path(s, input, key))).unwrap_or(true);
                return if sensitive { Perm::Ask } else { Perm::Allow };
            }
            if mode == "acceptEdits" || rules.iter().any(|r| r == "Edit") {
                Perm::Allow
            } else {
                Perm::Ask
            }
        }
        "Bash" => {
            let cmd = input["command"].as_str().unwrap_or("");
            if bypass || is_safe_command_in(cmd, s) {
                return Perm::Allow;
            }
            if mode == "plan" {
                return Perm::Deny("Plan mode is active: only read-only commands can run until the user approves a plan (call ExitPlanMode).".into());
            }
            if bash_allowed_by_rules(cmd, rules, s) {
                return Perm::Allow;
            }
            if mode == "auto" && bash_risk(cmd, s).is_none() {
                return Perm::Allow;
            }
            Perm::Ask
        }
        // Claude Code's read-only / delegating tools
        "WebFetch" | "WebSearch" | "LS" | "Task" | "Agent" | "TodoRead" if mode == "auto" || bypass => Perm::Allow,
        _ if bypass => Perm::Allow,
        _ => Perm::Ask,
    }
}

/// Rules to remember when the user picks "allow for this session".
pub fn session_rules(s: &CodeSession, name: &str, input: &Value) -> Vec<String> {
    match name {
        "Write" | "Edit" | "Read" | "Glob" | "Grep" => {
            let key = if matches!(name, "Glob" | "Grep") { "path" } else { "file_path" };
            let p = input_path(s, input, key);
            if s.is_inside(&p) {
                if matches!(name, "Write" | "Edit") {
                    vec!["Edit".into()]
                } else {
                    vec![]
                }
            } else {
                let c = canonical_or_self(&p);
                let dir = if c.is_dir() { c } else { c.parent().map(|x| x.to_path_buf()).unwrap_or(c) };
                vec![format!("Path({})", dir.display())]
            }
        }
        "Bash" => split_simple_commands(input["command"].as_str().unwrap_or(""))
            .unwrap_or_default()
            .iter()
            .filter(|w| !simple_is_safe(w))
            .map(|w| format!("Bash({})", prefix_of(w)))
            .collect(),
        _ => vec![],
    }
}

pub async fn execute(
    s: Arc<CodeSession>,
    name: &str,
    input: &Value,
    cancel: &CancellationToken,
    default_timeout: Duration,
    progress: Option<ProgressFn>,
) -> ToolOutput {
    match name {
        "Bash" => {
            return bash(&s, input, cancel, default_timeout, progress)
                .await
                .unwrap_or_else(|e| ToolOutput::err(e.to_string()))
        }
        "BashOutput" => return bash_output(&s, input),
        "KillShell" => return kill_shell(&s, input),
        _ => {}
    }
    let name2 = name.to_string();
    let input2 = input.clone();
    let c2 = cancel.clone();
    let task = tokio::task::spawn_blocking(move || match name2.as_str() {
        "Read" => read(&s, &input2),
        "Write" => write(&s, &input2),
        "Edit" => edit(&s, &input2),
        "Glob" => glob(&s, &input2, &c2),
        "Grep" => grep(&s, &input2, &c2),
        "TodoWrite" => todo(&input2),
        other => Err(AppError::msg(format!("Unknown tool {other}"))),
    });
    let r = tokio::select! {
        r = task => r.unwrap_or_else(|e| Err(AppError::msg(format!("The tool failed unexpectedly: {e}")))),
        _ = cancel.cancelled() => Ok(ToolOutput::err("The user interrupted this tool call.")),
    };
    r.unwrap_or_else(|e| ToolOutput::err(e.to_string()))
}

fn todo(input: &Value) -> AppResult<ToolOutput> {
    let todos = input["todos"]
        .as_array()
        .ok_or_else(|| AppError::msg("todos must be an array"))?;
    for t in todos {
        let st = t["status"].as_str().unwrap_or("");
        if !matches!(st, "pending" | "in_progress" | "completed") || t["content"].as_str().unwrap_or("").is_empty() {
            return Err(AppError::msg("Each todo needs content and a status of pending, in_progress or completed."));
        }
    }
    Ok(ToolOutput::text(
        "Todos have been updated. Keep using the todo list to track progress and proceed with the current tasks.",
    ))
}

// ---------------------------------------------------------------- Bash

const BASH_OUTPUT_CAP: usize = 2_000_000;

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R, live: Option<&LiveOut>) -> (Vec<u8>, bool) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => return (buf, false),
            Ok(n) => {
                if let Some(l) = live {
                    l.push(&chunk[..n]);
                }
                if buf.len() + n > BASH_OUTPUT_CAP {
                    let room = BASH_OUTPUT_CAP - buf.len();
                    buf.extend_from_slice(&chunk[..room]);
                    return (buf, true);
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        }
    }
}

/// Combined live output of a running command, reported to a `ProgressFn`
/// at most every 120 ms.
struct LiveOut {
    text: Mutex<(Vec<u8>, std::time::Instant)>,
    cb: ProgressFn,
}

const LIVE_TAIL: usize = 24_000;

impl LiveOut {
    fn push(&self, bytes: &[u8]) {
        let mut g = self.text.lock();
        g.0.extend_from_slice(bytes);
        if g.0.len() > LIVE_TAIL * 2 {
            let cut = g.0.len() - LIVE_TAIL;
            g.0.drain(..cut);
        }
        if g.1.elapsed() >= Duration::from_millis(120) {
            g.1 = std::time::Instant::now();
            let tail = if g.0.len() > LIVE_TAIL { &g.0[g.0.len() - LIVE_TAIL..] } else { &g.0[..] };
            let s = String::from_utf8_lossy(tail).to_string();
            drop(g);
            (self.cb)(&s);
        }
    }
}

/// SIGKILL the process group led by `pid` (the shell started with
/// `process_group(0)`), so commands it started die with it. Signals nothing
/// unless `pid` really leads its own group: never the app's own group.
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    signal_process_group(pid, libc::SIGKILL);
}

/// Send `sig` to the process group led by `pid` (see `kill_process_group`).
#[cfg(unix)]
pub(crate) fn signal_process_group(pid: u32, sig: libc::c_int) {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return };
    if pid <= 1 {
        return;
    }
    // SAFETY: plain syscalls on a pid we spawned; no memory is shared.
    unsafe {
        if libc::getpgid(pid) == pid && libc::getpgid(0) != pid {
            libc::kill(-pid, sig);
        }
    }
}

/// Kill a process and everything it started (Windows: the whole tree).
#[cfg(windows)]
pub(crate) fn kill_process_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .creation_flags(0x0800_0000)
        .status();
}

/// The platform shell running `command` in `cwd`: bash/sh on Unix (in its
/// own process group, writing its final directory to the returned file),
/// PowerShell on Windows.
pub(crate) fn shell_command(cwd: &Path, command: &str) -> (tokio::process::Command, Option<PathBuf>) {
    #[cfg(unix)]
    let (mut cmd, pwd_file) = {
        let pwd_file = std::env::temp_dir().join(format!("clauderust-pwd-{}", crate::util::new_id()));
        let shell = if which::which("bash").is_ok() { "bash" } else { "sh" };
        let mut c = tokio::process::Command::new(shell);
        c.arg("-c")
            .arg("cd \"$CR_CWD\" || exit 1\neval \"$CR_CMD\"\n__cr_ec=$?\npwd -P > \"$CR_PWD_FILE\" 2>/dev/null\nexit $__cr_ec")
            .env("CR_CWD", cwd)
            .env("CR_CMD", command)
            .env("CR_PWD_FILE", &pwd_file)
            .env("PATH", crate::shellenv::path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("CLAUDECODE", "1")
            // show output as it happens instead of when buffers fill
            .env("PYTHONUNBUFFERED", "1")
            .process_group(0);
        (c, Some(pwd_file))
    };
    #[cfg(windows)]
    let (mut cmd, pwd_file): (tokio::process::Command, Option<PathBuf>) = {
        let mut c = tokio::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", command])
            .current_dir(cwd)
            .env("PATH", crate::shellenv::path())
            .env("PYTHONUNBUFFERED", "1");
        c.creation_flags(0x0800_0000);
        (c, None)
    };
    crate::display::scrub_child_env(&mut cmd);
    (cmd, pwd_file)
}

fn bash_output(s: &CodeSession, input: &Value) -> ToolOutput {
    let id = input["bash_id"].as_str().or(input["shell_id"].as_str()).unwrap_or("");
    let Some(sh) = s.shells.as_ref().and_then(|r| r.get(id)).filter(|sh| sh.conv_id == s.conv_id) else {
        return ToolOutput::err(format!("No background shell with id {id}."));
    };
    let mut out = sh.take_new();
    if let Some(f) = input["filter"].as_str().filter(|f| !f.is_empty()) {
        match RegexBuilder::new(f).size_limit(1 << 20).build() {
            Ok(re) => out = out.lines().filter(|l| re.is_match(l)).collect::<Vec<_>>().join("\n"),
            Err(e) => return ToolOutput::err(format!("Invalid filter: {e}")),
        }
    }
    let status = sh.status();
    let mut text = format!("<status>{status}</status>");
    if let Some(c) = *sh.exit.lock() {
        text.push_str(&format!("\n<exit_code>{c}</exit_code>"));
    }
    if out.trim().is_empty() {
        text.push_str("\n(no new output)");
    } else {
        text.push_str(&format!("\n<output>\n{}\n</output>", truncate_middle(out.trim_end(), MAX_OUTPUT_CHARS)));
    }
    ToolOutput::text(text)
}

fn kill_shell(s: &CodeSession, input: &Value) -> ToolOutput {
    let id = input["shell_id"].as_str().or(input["bash_id"].as_str()).unwrap_or("");
    let Some(reg) = s.shells.as_ref() else { return ToolOutput::err("Background shells are not available here.") };
    match reg.get(id).filter(|sh| sh.conv_id == s.conv_id) {
        None => ToolOutput::err(format!("No background shell with id {id}.")),
        Some(sh) if !sh.running() => ToolOutput::text(format!("Shell {id} is not running ({}).", sh.status())),
        Some(_) => {
            reg.kill(id);
            ToolOutput::text(format!("Stopped background shell {id}."))
        }
    }
}

async fn bash(
    s: &CodeSession,
    input: &Value,
    cancel: &CancellationToken,
    default_timeout: Duration,
    progress: Option<ProgressFn>,
) -> AppResult<ToolOutput> {
    let command = input["command"].as_str().unwrap_or("").to_string();
    if command.trim().is_empty() {
        return Err(AppError::msg("command is required"));
    }
    if input["run_in_background"].as_bool() == Some(true) {
        let Some(reg) = s.shells.as_ref() else {
            return Err(AppError::msg("Background commands are not available here."));
        };
        let sh = reg.spawn(s, &command, input["description"].as_str().unwrap_or(""))?;
        return Ok(ToolOutput::text(format!(
            "Command running in background with ID: {}. Use BashOutput to read its output and KillShell to stop it.",
            sh.id
        )));
    }
    let timeout = input["timeout"]
        .as_f64()
        .filter(|v| v.is_finite())
        .map(|ms| Duration::from_millis(ms.clamp(1000.0, 600_000.0) as u64))
        .unwrap_or(default_timeout);
    let cwd = s.cwd.lock().clone();
    let cwd = if cwd.is_dir() { cwd } else { s.root.clone() };
    let (mut cmd, pwd_file) = shell_command(&cwd, &command);
    let live = progress.map(|cb| LiveOut { text: Mutex::new((vec![], std::time::Instant::now())), cb });
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| AppError::msg(format!("Failed to start shell: {e}")))?;
    let pid = child.id();
    let kill_group = move || {
        #[cfg(unix)]
        if let Some(p) = pid {
            kill_process_group(p);
        }
        #[cfg(windows)]
        if let Some(p) = pid {
            kill_process_tree(p);
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let run = async {
        let (o, e) = tokio::join!(
            async {
                match stdout {
                    Some(x) => read_capped(x, live.as_ref()).await,
                    None => (vec![], false),
                }
            },
            async {
                match stderr {
                    Some(x) => read_capped(x, live.as_ref()).await,
                    None => (vec![], false),
                }
            }
        );
        let capped = o.1 || e.1;
        if capped {
            kill_group();
        }
        let status = child.wait().await;
        (o.0, e.0, capped, status)
    };
    let res = tokio::select! {
        r = tokio::time::timeout(timeout, run) => r,
        _ = cancel.cancelled() => {
            kill_group();
            return Ok(ToolOutput::err("Command was interrupted by the user."));
        }
    };
    let (out, err, capped, status) = match res {
        Ok(v) => v,
        Err(_) => {
            kill_group();
            return Ok(ToolOutput::err(format!("Command timed out after {} seconds.", timeout.as_secs())));
        }
    };
    if let Some(pf) = pwd_file {
        if let Ok(p) = std::fs::read_to_string(&pf) {
            let p = PathBuf::from(p.trim());
            if p.is_dir() {
                *s.cwd.lock() = p;
            }
        }
        let _ = std::fs::remove_file(pf);
    }
    let code = status.ok().and_then(|st| st.code()).unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out).to_string();
    let stderr = String::from_utf8_lossy(&err).to_string();
    let mut text = String::new();
    if !stdout.trim_end().is_empty() {
        text.push_str(stdout.trim_end());
    }
    if !stderr.trim_end().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr.trim_end());
    }
    if capped {
        text.push_str("\n[Output exceeded 2 MB; the command was stopped.]");
    } else if code != 0 {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("Exit code {code}"));
    }
    if text.is_empty() {
        text = "(no output)".into();
    }
    Ok(ToolOutput {
        content: vec![json!({"type": "text", "text": truncate_middle(&text, MAX_OUTPUT_CHARS)})],
        is_error: code != 0 || capped,
    })
}

// ---------------------------------------------------------------- Read

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

fn num_arg(input: &Value, key: &str, default: usize, max: usize) -> usize {
    input[key]
        .as_f64()
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(1.0, max as f64) as usize)
        .unwrap_or(default)
}

fn read(s: &CodeSession, input: &Value) -> AppResult<ToolOutput> {
    use std::io::BufRead;
    let path = s.resolve(input["file_path"].as_str().unwrap_or(""));
    if path.is_dir() {
        return Err(AppError::msg(format!(
            "{} is a directory. Use Glob or `ls` via Bash to list it.",
            path.display()
        )));
    }
    let meta = std::fs::metadata(&path).map_err(|_| AppError::msg(format!("File does not exist: {}", path.display())))?;
    if !meta.is_file() {
        return Err(AppError::msg(format!("{} is not a regular file.", path.display())));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf") {
        let a = attachments::from_path(&path)?;
        s.read_files.lock().insert(path.clone());
        if a.kind != "text" {
            return Ok(ToolOutput {
                content: vec![attachments::to_block(&a)],
                is_error: false,
            });
        }
    }
    let file = std::fs::File::open(&path)?;
    let mut reader = std::io::BufReader::new(std::io::Read::take(file, 512 * 1024 * 1024));
    if is_binary(reader.fill_buf()?) {
        return Err(AppError::msg(format!("{} appears to be a binary file.", path.display())));
    }
    s.read_files.lock().insert(path.clone());
    if meta.len() == 0 {
        return Ok(ToolOutput::text("<system-reminder>The file exists but is empty.</system-reminder>"));
    }
    let offset = num_arg(input, "offset", 1, 1_000_000_000);
    let limit = num_arg(input, "limit", 2000, 10_000);
    let end = offset.saturating_add(limit);
    let count_all = meta.len() <= 20 * 1024 * 1024;
    let mut out = String::new();
    let mut total = 0usize;
    let mut more = false;
    for (i, line) in reader.split(b'\n').enumerate() {
        let Ok(line) = line else { break };
        let n = i + 1;
        total = n;
        if n < offset {
            continue;
        }
        if n >= end {
            more = true;
            if count_all {
                continue;
            }
            break;
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches('\r');
        let line = if line.chars().count() > 2000 {
            format!("{}… [line truncated]", line.chars().take(2000).collect::<String>())
        } else {
            line.to_string()
        };
        out.push_str(&format!("{n:>6}\t{line}\n"));
    }
    if more {
        if count_all {
            out.push_str(&format!("\n[Showing lines {}-{} of {}. Use offset/limit to read more.]", offset, end - 1, total));
        } else {
            out.push_str(&format!("\n[Showing lines {}-{}. The file is large; use offset/limit to read more.]", offset, end - 1));
        }
    }
    if out.is_empty() {
        out = format!("<system-reminder>The file has only {total} lines; offset {offset} is past the end.</system-reminder>");
    }
    Ok(ToolOutput::text(out))
}

// ---------------------------------------------------------------- Write / Edit

fn write(s: &CodeSession, input: &Value) -> AppResult<ToolOutput> {
    let raw = input["file_path"].as_str().unwrap_or("");
    if raw.is_empty() {
        return Err(AppError::msg("file_path is required"));
    }
    let path = s.resolve(raw);
    let content = input["content"]
        .as_str()
        .ok_or_else(|| AppError::msg("content is required"))?;
    let existed = path.exists();
    if existed && !s.read_files.lock().contains(&path) {
        return Err(AppError::msg("The file already exists and has not been read yet. Read it first, then write it."));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    s.remember_original(&path);
    std::fs::write(&path, content)?;
    s.read_files.lock().insert(path.clone());
    Ok(ToolOutput::text(if existed {
        format!("The file {} has been updated.", path.display())
    } else {
        format!("File created successfully at: {}", path.display())
    }))
}

fn edit(s: &CodeSession, input: &Value) -> AppResult<ToolOutput> {
    let path = s.resolve(input["file_path"].as_str().unwrap_or(""));
    let old = input["old_string"].as_str().unwrap_or("");
    let new = input["new_string"].as_str().unwrap_or("");
    let all = input["replace_all"].as_bool().unwrap_or(false);
    if !path.exists() {
        if old.is_empty() {
            // creating a new file through Edit
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            s.remember_original(&path);
            std::fs::write(&path, new)?;
            s.read_files.lock().insert(path.clone());
            return Ok(ToolOutput::text(format!("File created successfully at: {}", path.display())));
        }
        return Err(AppError::msg(format!("File does not exist: {}", path.display())));
    }
    if !s.read_files.lock().contains(&path) {
        return Err(AppError::msg("File has not been read yet. Read it first before editing it."));
    }
    if old == new {
        return Err(AppError::msg("old_string and new_string are identical; nothing to change."));
    }
    if std::fs::metadata(&path)?.len() > 50 * 1024 * 1024 {
        return Err(AppError::msg("The file is too large to edit with this tool."));
    }
    let text = std::fs::read_to_string(&path)?;
    let count = if old.is_empty() { 0 } else { text.matches(old).count() };
    if count == 0 {
        return Err(AppError::msg("String to replace not found in file. Make sure it matches exactly, including whitespace and indentation."));
    }
    if count > 1 && !all {
        return Err(AppError::msg(format!(
            "Found {count} matches of the string to replace, but replace_all is false. Add more surrounding context to make it unique, or set replace_all to true."
        )));
    }
    let pos = text.find(old).unwrap_or(0);
    let updated = if all { text.replace(old, new) } else { text.replacen(old, new, 1) };
    s.remember_original(&path);
    std::fs::write(&path, &updated)?;
    // snippet around the first change
    let start_line = text[..pos].matches('\n').count();
    let new_lines = new.matches('\n').count() + 1;
    let lines: Vec<&str> = updated.lines().collect();
    let from = start_line.saturating_sub(3);
    let to = (start_line + new_lines + 3).min(lines.len());
    let mut snippet = String::new();
    for (i, l) in lines.iter().enumerate().take(to).skip(from) {
        snippet.push_str(&format!("{:>6}\t{}\n", i + 1, l));
    }
    Ok(ToolOutput::text(format!(
        "The file {} has been updated{}. Here's the result of running `cat -n` on a snippet of the edited file:\n{}",
        path.display(),
        if all && count > 1 { format!(" ({count} replacements)") } else { String::new() },
        snippet
    )))
}

/// Unified diff for an Edit/Write tool call (for permission prompts and UI).
/// Current text of a file for a diff preview: regular files up to 4 MB only,
/// so a path like /dev/zero or a FIFO can't hang or exhaust memory.
fn read_for_preview(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

pub fn preview_diff(s: Option<&CodeSession>, name: &str, input: &Value) -> Option<String> {
    let path_str = input["file_path"].as_str()?;
    let (old_text, new_text) = match name {
        "Edit" => {
            let path = s.map(|s| s.resolve(path_str)).unwrap_or_else(|| PathBuf::from(path_str));
            let old = input["old_string"].as_str().unwrap_or("");
            let new = input["new_string"].as_str().unwrap_or("");
            match read_for_preview(&path) {
                Some(t) if !old.is_empty() && t.contains(old) => {
                    let all = input["replace_all"].as_bool().unwrap_or(false);
                    let u = if all { t.replace(old, new) } else { t.replacen(old, new, 1) };
                    (t, u)
                }
                _ => (old.to_string(), new.to_string()),
            }
        }
        "Write" => {
            let path = s.map(|s| s.resolve(path_str)).unwrap_or_else(|| PathBuf::from(path_str));
            let old = read_for_preview(&path).unwrap_or_default();
            (old, input["content"].as_str().unwrap_or("").to_string())
        }
        _ => return None,
    };
    let diff = similar::TextDiff::from_lines(&old_text, &new_text);
    let out = diff
        .unified_diff()
        .context_radius(3)
        .header(path_str, path_str)
        .to_string();
    Some(truncate_middle(&out, 60_000))
}

// ---------------------------------------------------------------- Glob / Grep

fn mtime(p: &Path) -> SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

fn walker(base: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(base)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
}

fn glob(s: &CodeSession, input: &Value, cancel: &CancellationToken) -> AppResult<ToolOutput> {
    let pattern = input["pattern"].as_str().unwrap_or("");
    if pattern.is_empty() {
        return Err(AppError::msg("pattern is required"));
    }
    let base = input["path"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| s.resolve(p))
        .unwrap_or_else(|| s.cwd.lock().clone());
    let pat = if pattern.contains('/') || pattern.starts_with("**") {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    };
    let g = Glob::new(&pat).map_err(|e| AppError::msg(format!("Invalid glob: {e}")))?;
    let set = GlobSetBuilder::new().add(g).build().map_err(|e| AppError::msg(e.to_string()))?;
    let mut hits: Vec<(SystemTime, PathBuf)> = vec![];
    for (visited, e) in walker(&base).flatten().enumerate() {
        if cancel.is_cancelled() || visited > 400_000 {
            break;
        }
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let rel = e.path().strip_prefix(&base).unwrap_or(e.path());
        if set.is_match(rel) {
            hits.push((mtime(e.path()), e.path().to_path_buf()));
        }
        if hits.len() > 20_000 {
            break;
        }
    }
    if hits.is_empty() {
        return Ok(ToolOutput::text("No files found"));
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    let total = hits.len();
    let mut out: Vec<String> = hits.into_iter().take(100).map(|(_, p)| p.display().to_string()).collect();
    if total > 100 {
        out.push(format!("(Results are truncated: showing 100 of {total}. Use a more specific path or pattern.)"));
    }
    Ok(ToolOutput::text(out.join("\n")))
}

fn type_exts(t: &str) -> Vec<&'static str> {
    match t {
        "rust" | "rs" => vec!["rs"],
        "js" | "javascript" => vec!["js", "jsx", "mjs", "cjs"],
        "ts" | "typescript" => vec!["ts", "tsx", "mts", "cts"],
        "py" | "python" => vec!["py", "pyi"],
        "go" => vec!["go"],
        "java" => vec!["java"],
        "kotlin" | "kt" => vec!["kt", "kts"],
        "c" => vec!["c", "h"],
        "cpp" | "c++" => vec!["cpp", "cc", "cxx", "hpp", "hh", "h"],
        "cs" | "csharp" => vec!["cs"],
        "rb" | "ruby" => vec!["rb"],
        "php" => vec!["php"],
        "swift" => vec!["swift"],
        "md" | "markdown" => vec!["md", "markdown"],
        "json" => vec!["json"],
        "yaml" | "yml" => vec!["yaml", "yml"],
        "toml" => vec!["toml"],
        "html" => vec!["html", "htm"],
        "css" => vec!["css", "scss", "less"],
        "sh" | "shell" => vec!["sh", "bash", "zsh"],
        "sql" => vec!["sql"],
        _ => vec![],
    }
}

fn grep(s: &CodeSession, input: &Value, cancel: &CancellationToken) -> AppResult<ToolOutput> {
    let pattern = input["pattern"].as_str().unwrap_or("");
    if pattern.is_empty() {
        return Err(AppError::msg("pattern is required"));
    }
    let mode = input["output_mode"].as_str().unwrap_or("files_with_matches");
    let ci = input["-i"].as_bool().unwrap_or(false);
    let multiline = input["multiline"].as_bool().unwrap_or(false);
    let show_n = input["-n"].as_bool().unwrap_or(true);
    let ctx_arg = |k: &str| input[k].as_f64().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1000.0) as usize);
    let ctx = ctx_arg("-C");
    let after = ctx_arg("-A").or(ctx).unwrap_or(0);
    let before = ctx_arg("-B").or(ctx).unwrap_or(0);
    let head_limit = input["head_limit"].as_f64().filter(|v| v.is_finite()).map(|v| v.clamp(1.0, 100_000.0) as usize);
    let re = RegexBuilder::new(pattern)
        .case_insensitive(ci)
        .multi_line(true)
        .dot_matches_new_line(multiline)
        .build()
        .map_err(|e| AppError::msg(format!("Invalid regex: {e}")))?;
    let base = input["path"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| s.resolve(p))
        .unwrap_or_else(|| s.cwd.lock().clone());
    let globset = match input["glob"].as_str().filter(|g| !g.is_empty()) {
        Some(g) => {
            let mut b = GlobSetBuilder::new();
            // support "*.{ts,tsx}" and plain names
            let pat = if g.contains('/') { g.to_string() } else { format!("**/{g}") };
            b.add(Glob::new(&pat).map_err(|e| AppError::msg(format!("Invalid glob: {e}")))?);
            Some(b.build().map_err(|e| AppError::msg(e.to_string()))?)
        }
        None => None,
    };
    let exts = input["type"].as_str().map(type_exts).unwrap_or_default();

    let files: Vec<PathBuf> = if base.is_file() {
        vec![base.clone()]
    } else {
        walker(&base)
            .flatten()
            .take(400_000)
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.into_path())
            .filter(|p| {
                let rel = p.strip_prefix(&base).unwrap_or(p);
                globset.as_ref().map(|g| g.is_match(rel)).unwrap_or(true)
                    && (exts.is_empty()
                        || p.extension()
                            .map(|e| exts.contains(&e.to_string_lossy().to_ascii_lowercase().as_str()))
                            .unwrap_or(false))
            })
            .collect()
    };

    let mut file_hits: Vec<(SystemTime, PathBuf, usize)> = vec![];
    let mut lines_out: Vec<String> = vec![];
    let mut scanned: u64 = 0;
    for f in files {
        if cancel.is_cancelled() || scanned > 2_000_000_000 {
            break;
        }
        let Ok(meta) = std::fs::metadata(&f) else { continue };
        scanned += meta.len();
        if meta.len() > 10 * 1024 * 1024 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&f) else { continue };
        if is_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        if mode == "content" {
            let lines: Vec<&str> = text.lines().collect();
            let mut matched_lines: Vec<usize> = vec![];
            if multiline {
                for m in re.find_iter(&text) {
                    let l = text[..m.start()].matches('\n').count();
                    if !matched_lines.contains(&l) {
                        matched_lines.push(l);
                    }
                }
            } else {
                for (i, l) in lines.iter().enumerate() {
                    if re.is_match(l) {
                        matched_lines.push(i);
                    }
                }
            }
            if matched_lines.is_empty() {
                continue;
            }
            let mut shown: Vec<usize> = vec![];
            for &m in &matched_lines {
                let from = m.saturating_sub(before);
                let to = m.saturating_add(after).min(lines.len().saturating_sub(1));
                for i in from..=to {
                    if !shown.contains(&i) {
                        shown.push(i);
                    }
                }
            }
            shown.sort_unstable();
            let mut prev: Option<usize> = None;
            for i in shown {
                if (before > 0 || after > 0) && prev.map(|p| i > p + 1).unwrap_or(false) {
                    lines_out.push("--".into());
                }
                let sep = if matched_lines.contains(&i) { ':' } else { '-' };
                let line = lines.get(i).copied().unwrap_or("");
                let line = if line.chars().count() > 500 {
                    format!("{}…", line.chars().take(500).collect::<String>())
                } else {
                    line.to_string()
                };
                if show_n {
                    lines_out.push(format!("{}{sep}{}{sep}{line}", f.display(), i + 1));
                } else {
                    lines_out.push(format!("{}{sep}{line}", f.display()));
                }
                prev = Some(i);
            }
            if lines_out.len() > 5000 {
                break;
            }
        } else {
            let n = re.find_iter(&text).count();
            if n > 0 {
                file_hits.push((mtime(&f), f.clone(), n));
            }
        }
    }
    let mut out: Vec<String> = match mode {
        "content" => lines_out,
        "count" => file_hits.iter().map(|(_, p, n)| format!("{}:{n}", p.display())).collect(),
        _ => {
            file_hits.sort_by_key(|h| std::cmp::Reverse(h.0));
            file_hits.iter().map(|(_, p, _)| p.display().to_string()).collect()
        }
    };
    if out.is_empty() {
        return Ok(ToolOutput::text("No matches found"));
    }
    let limit = head_limit.unwrap_or(if mode == "content" { 400 } else { 200 });
    let total = out.len();
    out.truncate(limit);
    let mut text = out.join("\n");
    if total > limit {
        text.push_str(&format!("\n(Showing {limit} of {total} results.)"));
    }
    if mode == "files_with_matches" {
        text = format!("Found {} file{}\n{text}", total, if total == 1 { "" } else { "s" });
    }
    Ok(ToolOutput::text(truncate_middle(&text, MAX_OUTPUT_CHARS)))
}

// ---------------------------------------------------------------- env snapshot

/// Environment block captured when a code session starts.
pub fn env_snapshot(cwd: &Path, model: &str) -> String {
    let is_git = cwd.join(".git").exists() || {
        let mut p = cwd.parent();
        let mut found = false;
        while let Some(d) = p {
            if d.join(".git").exists() {
                found = true;
                break;
            }
            p = d.parent();
        }
        found
    };
    let mut s = format!(
        "Working directory: {}\nIs directory a git repo: {}\nPlatform: {}\nToday's date: {}\nModel: {}",
        cwd.display(),
        if is_git { "Yes" } else { "No" },
        std::env::consts::OS,
        chrono::Local::now().format("%Y-%m-%d"),
        model
    );
    if is_git {
        let run = |args: &[&str]| -> String {
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default()
        };
        let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"]);
        let status = run(&["status", "--short"]);
        let log = run(&["log", "--oneline", "-n", "5"]);
        s.push_str(&format!(
            "\n\ngitStatus (snapshot at session start; it will not update):\nCurrent branch: {}\n\nStatus:\n{}\n\nRecent commits:\n{}",
            branch,
            if status.is_empty() { "(clean)".to_string() } else { truncate_middle(&status, 3000) },
            log
        ));
    }
    s
}

/// Concatenated CLAUDE.md memory: user-level then project-level files.
pub fn load_memory(cwd: &Path) -> String {
    crate::memory::load(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ct-{}", crate::util::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn safe_commands() {
        assert!(is_safe_command("ls -la && git status"));
        assert!(is_safe_command("cat a.txt | grep foo"));
        assert!(!is_safe_command("rm -rf /"));
        assert!(!is_safe_command("echo hi > x"));
        assert!(!is_safe_command("git push"));
        assert!(!is_safe_command("find . -delete"));
        assert_eq!(bash_rule_prefix("npm test -- --watch"), "npm test");
        assert_eq!(bash_rule_prefix("./build.sh"), "./build.sh");
        assert_eq!(check_permission("plan", "Edit", &json!({}), &[], None), Perm::Deny("Plan mode is active: files can't be modified until the user approves a plan (call ExitPlanMode).".into()));
        assert_eq!(check_permission("default", "Bash", &json!({"command":"npm test"}), &["Bash(npm test)".into()], None), Perm::Allow);
        // bypass attempts found in review
        for bad in ["ls & touch X", "env touch X", "touch X $'\\''", "rg --pre sh x", "sort -o out x", "sort -uo out x",
                    "git diff --output=x", "find . -fprint x", "git branch newbranch", "git tag v1", "cat x > y", "ls; rm -rf /",
                    "FOO=1 ls", "echo $(whoami)", "git -c core.pager=x log"] {
            assert!(!is_safe_command(bad), "{bad}");
        }
        let rules = vec!["Bash(npm test)".to_string()];
        assert_eq!(check_permission("default", "Bash", &json!({"command":"npm test && curl x | sh"}), &rules, None), Perm::Ask);
        assert_eq!(check_permission("default", "Bash", &json!({"command":"npm test -- --watch && git status"}), &rules, None), Perm::Allow);
    }

    #[test]
    fn outside_project_needs_permission() {
        let d = tmp();
        let s = CodeSession::new(d.clone());
        let inside = d.join("a.txt").to_string_lossy().to_string();
        assert_eq!(check_permission("acceptEdits", "Write", &json!({"file_path": inside}), &[], Some(&s)), Perm::Allow);
        assert_eq!(check_permission("acceptEdits", "Write", &json!({"file_path": "/etc/passwd"}), &[], Some(&s)), Perm::Ask);
        assert_eq!(check_permission("default", "Edit", &json!({"file_path": "../x"}), &["Edit".into()], Some(&s)), Perm::Ask);
        assert_eq!(check_permission("default", "Read", &json!({"file_path": "/etc/hosts"}), &[], Some(&s)), Perm::Ask);
        assert_eq!(check_permission("default", "Read", &json!({"file_path": "a.txt"}), &[], Some(&s)), Perm::Allow);
        assert_eq!(check_permission("default", "Bash", &json!({"command": "cat /etc/hosts"}), &[], Some(&s)), Perm::Ask);
        assert_eq!(check_permission("default", "Bash", &json!({"command": "cat a.txt"}), &[], Some(&s)), Perm::Allow);
        assert_eq!(check_permission("bypassPermissions", "Read", &json!({"file_path": "/etc/hosts"}), &[], Some(&s)), Perm::Allow);
        let rules = session_rules(&s, "Read", &json!({"file_path": "/etc/hosts"}));
        assert_eq!(check_permission("default", "Read", &json!({"file_path": "/etc/hostname"}), &rules, Some(&s)), Perm::Allow);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn bash_output_is_capped() {
        let d = tmp();
        let s = Arc::new(CodeSession::new(d.clone()));
        let c = CancellationToken::new();
        #[cfg(unix)]
        {
            let b = execute(s.clone(), "Bash", &json!({"command": "cat /dev/zero"}), &c, Duration::from_secs(20), None).await;
            assert!(b.is_error);
            assert!(b.content[0]["text"].as_str().unwrap().contains("exceeded"));
            let r = execute(s.clone(), "Read", &json!({"file_path": "/dev/zero"}), &c, Duration::from_secs(5), None).await;
            assert!(r.is_error);
            let r = execute(s.clone(), "Read", &json!({"file_path": "x", "offset": 1e30, "limit": 1e30}), &c, Duration::from_secs(5), None).await;
            assert!(r.is_error); // missing file, but must not panic
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_the_whole_command() {
        let d = tmp();
        let s = Arc::new(CodeSession::new(d.clone()));
        let c = CancellationToken::new();
        let marker = d.join("pid");
        let cmd = format!("sleep 30 & echo $! > '{}'; wait", marker.display());
        let b = execute(s.clone(), "Bash", &json!({"command": cmd, "timeout": 1000}), &c, Duration::from_secs(20), None).await;
        assert!(b.is_error && b.content[0]["text"].as_str().unwrap().contains("timed out"), "{:?}", b.content);
        let pid: i32 = std::fs::read_to_string(&marker).unwrap().trim().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        // Running = exists and isn't a zombie waiting to be reaped.
        let alive = if std::path::Path::new("/proc").is_dir() {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|stat| !stat.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z'))
                .unwrap_or(false)
        } else {
            // SAFETY: signal 0 only checks whether the process exists.
            unsafe { libc::kill(pid, 0) == 0 }
        };
        assert!(!alive, "background child survived");
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn read_edit_write_glob_grep_bash() {
        let d = tmp();
        let s = Arc::new(CodeSession::new(d.clone()));
        let c = CancellationToken::new();
        let t = Duration::from_secs(20);
        let f = d.join("src/main.rs");
        let w = execute(s.clone(), "Write", &json!({"file_path": f.to_string_lossy(), "content": "fn main() {\n    println!(\"hi\");\n}\n"}), &c, t, None).await;
        assert!(!w.is_error, "{:?}", w.content);
        let e = execute(s.clone(), "Edit", &json!({"file_path": "src/main.rs", "old_string": "\"hi\"", "new_string": "\"hello\""}), &c, t, None).await;
        assert!(!e.is_error, "{:?}", e.content);
        assert!(std::fs::read_to_string(&f).unwrap().contains("hello"));
        let r = execute(s.clone(), "Read", &json!({"file_path": f.to_string_lossy()}), &c, t, None).await;
        assert!(r.content[0]["text"].as_str().unwrap().contains("     2\t    println!(\"hello\");"));
        let g = execute(s.clone(), "Glob", &json!({"pattern": "*.rs"}), &c, t, None).await;
        assert!(g.content[0]["text"].as_str().unwrap().contains("main.rs"));
        let g = execute(s.clone(), "Grep", &json!({"pattern": "hel+o", "output_mode": "content"}), &c, t, None).await;
        assert!(g.content[0]["text"].as_str().unwrap().contains(":2:"), "{:?}", g.content);
        #[cfg(unix)]
        {
            let b = execute(s.clone(), "Bash", &json!({"command": "mkdir -p sub && cd sub && pwd"}), &c, t, None).await;
            assert!(!b.is_error);
            assert!(s.cwd.lock().ends_with("sub"));
            let b = execute(s.clone(), "Bash", &json!({"command": "exit 3"}), &c, t, None).await;
            assert!(b.is_error);
            assert!(b.content[0]["text"].as_str().unwrap().contains("Exit code 3"));
        }
        let diff = preview_diff(Some(&s), "Edit", &json!({"file_path": f.to_string_lossy(), "old_string": "hello", "new_string": "bye"})).unwrap();
        assert!(diff.contains("-    println!(\"hello\");") && diff.contains("+    println!(\"bye\");"), "{diff}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn background_shell_output_and_kill() {
        let dir = std::env::temp_dir().join(format!("cr-bg-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let reg = Arc::new(crate::shells::Shells::default());
        let s = Arc::new(CodeSession::for_conversation(dir.clone(), "conv1", reg.clone()));
        let c = CancellationToken::new();
        let r = execute(s.clone(), "Bash", &json!({"command": "echo first; sleep 30", "run_in_background": true}), &c, Duration::from_secs(5), None).await;
        assert!(!r.is_error);
        let text = r.content[0]["text"].as_str().unwrap().to_string();
        let id = text.split("ID: ").nth(1).unwrap().split('.').next().unwrap().to_string();
        let mut got = String::new();
        for _ in 0..50 {
            let o = execute(s.clone(), "BashOutput", &json!({"bash_id": id}), &c, Duration::from_secs(5), None).await;
            got.push_str(o.content[0]["text"].as_str().unwrap());
            if got.contains("first") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(got.contains("first") && got.contains("<status>running</status>"), "{got}");
        // another conversation can't see it
        let other = Arc::new(CodeSession::for_conversation(dir.clone(), "conv2", reg.clone()));
        let o = execute(other, "KillShell", &json!({"shell_id": id}), &c, Duration::from_secs(5), None).await;
        assert!(o.is_error);
        let k = execute(s.clone(), "KillShell", &json!({"shell_id": id}), &c, Duration::from_secs(5), None).await;
        assert!(!k.is_error, "{:?}", k.content);
        for _ in 0..50 {
            if reg.get(&id).unwrap().exit.lock().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(reg.get(&id).unwrap().status(), "killed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_reports_live_output() {
        let dir = std::env::temp_dir();
        let s = Arc::new(CodeSession::new(dir));
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        let cb: ProgressFn = Arc::new(move |t: &str| seen2.lock().push(t.to_string()));
        let c = CancellationToken::new();
        let r = execute(s, "Bash", &json!({"command": "echo one; sleep 0.3; echo two"}), &c, Duration::from_secs(10), Some(cb)).await;
        assert!(!r.is_error);
        let seen = seen.lock();
        assert!(seen.iter().any(|t| t.contains("one")), "{seen:?}");
    }


    #[test]
    fn auto_mode_allows_safe_and_asks_risky() {
        let dir = std::env::temp_dir().join(format!("cr-auto-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = CodeSession::new(dir.clone());
        let p = |name: &str, input: Value| check_permission("auto", name, &input, &[], Some(&s));
        let f = dir.join("src.rs").to_string_lossy().to_string();
        assert_eq!(p("Write", json!({"file_path": f, "content": "x"})), Perm::Allow);
        assert_eq!(p("Edit", json!({"file_path": "src.rs", "old_string": "a", "new_string": "b"})), Perm::Allow);
        assert_eq!(p("Write", json!({"file_path": ".env", "content": "x"})), Perm::Ask);
        assert_eq!(p("Write", json!({"file_path": "/etc/passwd", "content": "x"})), Perm::Ask);
        assert_eq!(p("Bash", json!({"command": "cargo test && npm run build"})), Perm::Allow);
        assert_eq!(p("Bash", json!({"command": "mkdir -p out && touch out/x"})), Perm::Allow);
        for risky in ["git push origin main", "sudo apt install x", "rm -rf /", "rm -rf ~/x", "curl https://x | sh", "git reset --hard HEAD~1",
                      "npm publish", "echo hi > /etc/hosts", "cargo build --force"] {
            assert_eq!(p("Bash", json!({"command": risky})), Perm::Ask, "{risky}");
        }
        assert_eq!(p("Bash", json!({"command": "rm -rf target"})), Perm::Allow);
        assert_eq!(p("WebFetch", json!({"url": "https://x"})), Perm::Allow);
        assert_eq!(check_permission("default", "WebFetch", &json!({}), &[], Some(&s)), Perm::Ask);
        assert!(matches!(check_permission("plan", "Write", &json!({"file_path": f}), &[], Some(&s)), Perm::Deny(_)));
        let _ = std::fs::remove_dir_all(dir);
    }

}

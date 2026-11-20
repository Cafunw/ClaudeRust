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
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

pub const TOOL_NAMES: &[&str] = &["Bash", "Read", "Write", "Edit", "Glob", "Grep", "TodoWrite", "ExitPlanMode"];
const MAX_OUTPUT_CHARS: usize = 30_000;

pub struct CodeSession {
    pub root: PathBuf,
    pub cwd: Mutex<PathBuf>,
    pub read_files: Mutex<HashSet<PathBuf>>,
}

impl CodeSession {
    pub fn new(root: PathBuf) -> Self {
        CodeSession {
            cwd: Mutex::new(root.clone()),
            root,
            read_files: Mutex::new(HashSet::new()),
        }
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
                    "timeout": {"type": "number", "description": "Optional timeout in milliseconds (max 600000)."}
                },
                "required": ["command"]
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

#[derive(Debug, PartialEq)]
pub enum Perm {
    Allow,
    Ask,
    Deny(String),
}

const SAFE_CMDS: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "wc", "echo", "which", "whoami", "date", "file", "stat", "du", "df", "tree",
    "grep", "rg", "uname", "env", "printenv", "basename", "dirname", "realpath", "sort", "uniq", "diff", "type",
];
const SAFE_GIT: &[&str] = &["status", "diff", "log", "show", "branch", "rev-parse", "ls-files", "blame", "remote", "describe", "shortlog", "tag"];

pub fn is_safe_command(cmd: &str) -> bool {
    if cmd.contains('>') || cmd.contains("$(") || cmd.contains('`') || cmd.contains("<(") {
        return false;
    }
    let normalized = cmd.replace("&&", ";").replace("||", ";").replace('|', ";");
    normalized.split([';', '\n']).all(|part| {
        let words = shell_words::split(part.trim()).unwrap_or_default();
        let Some(first) = words.first() else { return true };
        if first == "git" {
            return words.get(1).map(|s| SAFE_GIT.contains(&s.as_str())).unwrap_or(false)
                && !words.iter().any(|w| w == "-D" || w == "--delete" || w == "-d" || w == "add" || w == "remove");
        }
        if first == "find" {
            return !words.iter().any(|w| w == "-exec" || w == "-execdir" || w == "-delete" || w == "-ok");
        }
        SAFE_CMDS.contains(&first.as_str())
    })
}

/// Prefix used for "allow for this session" rules on Bash commands.
pub fn bash_rule_prefix(cmd: &str) -> String {
    let words = shell_words::split(cmd.trim()).unwrap_or_default();
    match words.as_slice() {
        [] => String::new(),
        [a] => a.clone(),
        [a, b, ..] => {
            let multi = ["git", "npm", "pnpm", "yarn", "bun", "cargo", "go", "make", "docker", "kubectl", "pip", "uv", "poetry", "dotnet", "mvn", "gradle", "python", "python3", "node", "npx"];
            if multi.contains(&a.as_str()) && !b.starts_with('-') {
                format!("{a} {b}")
            } else {
                a.clone()
            }
        }
    }
}

pub fn check_permission(mode: &str, name: &str, input: &Value, rules: &[String]) -> Perm {
    match name {
        "Read" | "Glob" | "Grep" | "TodoWrite" | "ExitPlanMode" => Perm::Allow,
        "Write" | "Edit" => match mode {
            "plan" => Perm::Deny("Plan mode is active: files can't be modified until the user approves a plan (call ExitPlanMode).".into()),
            "acceptEdits" | "bypassPermissions" => Perm::Allow,
            _ if rules.iter().any(|r| r == "Edit") => Perm::Allow,
            _ => Perm::Ask,
        },
        "Bash" => {
            let cmd = input["command"].as_str().unwrap_or("");
            if mode == "bypassPermissions" || is_safe_command(cmd) {
                return Perm::Allow;
            }
            if mode == "plan" {
                return Perm::Deny("Plan mode is active: only read-only commands can run until the user approves a plan (call ExitPlanMode).".into());
            }
            let prefix = bash_rule_prefix(cmd);
            if !prefix.is_empty() && rules.iter().any(|r| r == &format!("Bash({prefix})")) {
                return Perm::Allow;
            }
            Perm::Ask
        }
        _ => Perm::Ask,
    }
}

pub fn session_rule(name: &str, input: &Value) -> String {
    match name {
        "Write" | "Edit" => "Edit".into(),
        "Bash" => format!("Bash({})", bash_rule_prefix(input["command"].as_str().unwrap_or(""))),
        other => other.to_string(),
    }
}

pub async fn execute(
    s: &CodeSession,
    name: &str,
    input: &Value,
    cancel: &CancellationToken,
    default_timeout: Duration,
) -> ToolOutput {
    let r = match name {
        "Bash" => bash(s, input, cancel, default_timeout).await,
        "Read" => read(s, input),
        "Write" => write(s, input),
        "Edit" => edit(s, input),
        "Glob" => glob(s, input),
        "Grep" => grep(s, input),
        "TodoWrite" => todo(input),
        other => Err(AppError::msg(format!("Unknown tool {other}"))),
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

async fn bash(s: &CodeSession, input: &Value, cancel: &CancellationToken, default_timeout: Duration) -> AppResult<ToolOutput> {
    let command = input["command"].as_str().unwrap_or("").to_string();
    if command.trim().is_empty() {
        return Err(AppError::msg("command is required"));
    }
    let timeout = input["timeout"]
        .as_f64()
        .map(|ms| Duration::from_millis(ms.clamp(1000.0, 600_000.0) as u64))
        .unwrap_or(default_timeout);
    let cwd = s.cwd.lock().clone();
    let cwd = if cwd.is_dir() { cwd } else { s.root.clone() };

    #[cfg(unix)]
    let (mut cmd, pwd_file) = {
        let pwd_file = std::env::temp_dir().join(format!("clauderust-pwd-{}", crate::util::new_id()));
        let shell = if which::which("bash").is_ok() { "bash" } else { "sh" };
        let mut c = tokio::process::Command::new(shell);
        c.arg("-c")
            .arg("cd \"$CR_CWD\" || exit 1\neval \"$CR_CMD\"\n__cr_ec=$?\npwd -P > \"$CR_PWD_FILE\" 2>/dev/null\nexit $__cr_ec")
            .env("CR_CWD", &cwd)
            .env("CR_CMD", &command)
            .env("CR_PWD_FILE", &pwd_file)
            .env("PATH", crate::shellenv::path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("CLAUDECODE", "1")
            .process_group(0);
        (c, Some(pwd_file))
    };
    #[cfg(windows)]
    let (mut cmd, pwd_file): (tokio::process::Command, Option<PathBuf>) = {
        let mut c = tokio::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", command.as_str()])
            .current_dir(&cwd)
            .env("PATH", crate::shellenv::path());
        c.creation_flags(0x0800_0000);
        (c, None)
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| AppError::msg(format!("Failed to start shell: {e}")))?;
    let pid = child.id();
    let kill_group = move || {
        #[cfg(unix)]
        if let Some(p) = pid {
            let _ = std::process::Command::new("kill").args(["-TERM", &format!("-{p}")]).status();
        }
        #[cfg(windows)]
        if let Some(p) = pid {
            let _ = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &p.to_string()]).status();
        }
    };
    let out = tokio::select! {
        r = tokio::time::timeout(timeout, child.wait_with_output()) => r,
        _ = cancel.cancelled() => {
            kill_group();
            return Ok(ToolOutput::err("Command was interrupted by the user."));
        }
    };
    let out = match out {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(AppError::msg(format!("Command failed: {e}"))),
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
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let code = out.status.code().unwrap_or(-1);
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
    if code != 0 {
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
        is_error: code != 0,
    })
}

// ---------------------------------------------------------------- Read

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

fn read(s: &CodeSession, input: &Value) -> AppResult<ToolOutput> {
    let path = s.resolve(input["file_path"].as_str().unwrap_or(""));
    if path.is_dir() {
        return Err(AppError::msg(format!(
            "{} is a directory. Use Glob or `ls` via Bash to list it.",
            path.display()
        )));
    }
    if !path.exists() {
        return Err(AppError::msg(format!("File does not exist: {}", path.display())));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf") {
        let a = attachments::from_path(&path)?;
        s.read_files.lock().insert(path.clone());
        return Ok(ToolOutput {
            content: vec![attachments::to_block(&a)],
            is_error: false,
        });
    }
    let bytes = std::fs::read(&path)?;
    if is_binary(&bytes) {
        return Err(AppError::msg(format!("{} appears to be a binary file.", path.display())));
    }
    let text = String::from_utf8_lossy(&bytes);
    s.read_files.lock().insert(path.clone());
    if text.is_empty() {
        return Ok(ToolOutput::text("<system-reminder>The file exists but is empty.</system-reminder>"));
    }
    let offset = input["offset"].as_f64().map(|v| v.max(1.0) as usize).unwrap_or(1);
    let limit = input["limit"].as_f64().map(|v| v.max(1.0) as usize).unwrap_or(2000);
    let mut out = String::new();
    let mut total = 0usize;
    for (i, line) in text.lines().enumerate() {
        total = i + 1;
        let n = i + 1;
        if n < offset || n >= offset + limit {
            continue;
        }
        let line = if line.chars().count() > 2000 {
            format!("{}… [line truncated]", line.chars().take(2000).collect::<String>())
        } else {
            line.to_string()
        };
        out.push_str(&format!("{n:>6}\t{line}\n"));
    }
    if offset + limit <= total {
        out.push_str(&format!(
            "\n[Showing lines {}-{} of {}. Use offset/limit to read more.]",
            offset,
            offset + limit - 1,
            total
        ));
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
pub fn preview_diff(s: Option<&CodeSession>, name: &str, input: &Value) -> Option<String> {
    let path_str = input["file_path"].as_str()?;
    let (old_text, new_text) = match name {
        "Edit" => {
            let path = s.map(|s| s.resolve(path_str)).unwrap_or_else(|| PathBuf::from(path_str));
            let old = input["old_string"].as_str().unwrap_or("");
            let new = input["new_string"].as_str().unwrap_or("");
            match std::fs::read_to_string(&path) {
                Ok(t) if !old.is_empty() && t.contains(old) => {
                    let all = input["replace_all"].as_bool().unwrap_or(false);
                    let u = if all { t.replace(old, new) } else { t.replacen(old, new, 1) };
                    (t, u)
                }
                _ => (old.to_string(), new.to_string()),
            }
        }
        "Write" => {
            let path = s.map(|s| s.resolve(path_str)).unwrap_or_else(|| PathBuf::from(path_str));
            let old = std::fs::read_to_string(path).unwrap_or_default();
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

fn glob(s: &CodeSession, input: &Value) -> AppResult<ToolOutput> {
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
    for e in walker(&base).flatten() {
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
    hits.sort_by(|a, b| b.0.cmp(&a.0));
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

fn grep(s: &CodeSession, input: &Value) -> AppResult<ToolOutput> {
    let pattern = input["pattern"].as_str().unwrap_or("");
    if pattern.is_empty() {
        return Err(AppError::msg("pattern is required"));
    }
    let mode = input["output_mode"].as_str().unwrap_or("files_with_matches");
    let ci = input["-i"].as_bool().unwrap_or(false);
    let multiline = input["multiline"].as_bool().unwrap_or(false);
    let show_n = input["-n"].as_bool().unwrap_or(true);
    let ctx = input["-C"].as_f64().map(|v| v as usize);
    let after = input["-A"].as_f64().map(|v| v as usize).or(ctx).unwrap_or(0);
    let before = input["-B"].as_f64().map(|v| v as usize).or(ctx).unwrap_or(0);
    let head_limit = input["head_limit"].as_f64().map(|v| v as usize);
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
    for f in files {
        let Ok(meta) = std::fs::metadata(&f) else { continue };
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
                let to = (m + after).min(lines.len().saturating_sub(1));
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
            file_hits.sort_by(|a, b| b.0.cmp(&a.0));
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
    let mut parts = vec![];
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".claude").join("CLAUDE.md");
        if let Ok(t) = std::fs::read_to_string(&p) {
            parts.push(format!("Contents of {} (user instructions for all projects):\n\n{}", p.display(), t.trim()));
        }
    }
    // walk up from cwd collecting CLAUDE.md (root-most first)
    let mut dirs_up = vec![];
    let mut d = Some(cwd);
    while let Some(x) = d {
        dirs_up.push(x.to_path_buf());
        d = x.parent();
    }
    dirs_up.reverse();
    for dir in dirs_up {
        for name in ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"] {
            let p = dir.join(name);
            if let Ok(t) = std::fs::read_to_string(&p) {
                if !t.trim().is_empty() {
                    parts.push(format!("Contents of {} (project instructions):\n\n{}", p.display(), truncate_middle(t.trim(), 40_000)));
                }
            }
        }
    }
    parts.join("\n\n")
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
        assert_eq!(check_permission("plan", "Edit", &json!({}), &[]), Perm::Deny("Plan mode is active: files can't be modified until the user approves a plan (call ExitPlanMode).".into()));
        assert_eq!(check_permission("default", "Bash", &json!({"command":"npm test"}), &["Bash(npm test)".into()]), Perm::Allow);
    }

    #[tokio::test]
    async fn read_edit_write_glob_grep_bash() {
        let d = tmp();
        let s = CodeSession::new(d.clone());
        let c = CancellationToken::new();
        let t = Duration::from_secs(20);
        let f = d.join("src/main.rs");
        let w = execute(&s, "Write", &json!({"file_path": f.to_string_lossy(), "content": "fn main() {\n    println!(\"hi\");\n}\n"}), &c, t).await;
        assert!(!w.is_error, "{:?}", w.content);
        let e = execute(&s, "Edit", &json!({"file_path": "src/main.rs", "old_string": "\"hi\"", "new_string": "\"hello\""}), &c, t).await;
        assert!(!e.is_error, "{:?}", e.content);
        assert!(std::fs::read_to_string(&f).unwrap().contains("hello"));
        let r = execute(&s, "Read", &json!({"file_path": f.to_string_lossy()}), &c, t).await;
        assert!(r.content[0]["text"].as_str().unwrap().contains("     2\t    println!(\"hello\");"));
        let g = execute(&s, "Glob", &json!({"pattern": "*.rs"}), &c, t).await;
        assert!(g.content[0]["text"].as_str().unwrap().contains("main.rs"));
        let g = execute(&s, "Grep", &json!({"pattern": "hel+o", "output_mode": "content"}), &c, t).await;
        assert!(g.content[0]["text"].as_str().unwrap().contains(":2:"), "{:?}", g.content);
        #[cfg(unix)]
        {
            let b = execute(&s, "Bash", &json!({"command": "mkdir -p sub && cd sub && pwd"}), &c, t).await;
            assert!(!b.is_error);
            assert!(s.cwd.lock().ends_with("sub"));
            let b = execute(&s, "Bash", &json!({"command": "exit 3"}), &c, t).await;
            assert!(b.is_error);
            assert!(b.content[0]["text"].as_str().unwrap().contains("Exit code 3"));
        }
        let diff = preview_diff(Some(&s), "Edit", &json!({"file_path": f.to_string_lossy(), "old_string": "hello", "new_string": "bye"})).unwrap();
        assert!(diff.contains("-    println!(\"hello\");") && diff.contains("+    println!(\"bye\");"), "{diff}");
        let _ = std::fs::remove_dir_all(d);
    }
}

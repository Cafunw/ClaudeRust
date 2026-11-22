// Settings → About: "Copy diagnostics", a report for a bug report with
// keys, tokens, credentials, e-mail addresses and home folders taken out.

// order matters: credentials in a URL go before the e-mail rule sees their "@"
const RULES = [
  [/\b([a-z][a-z0-9+.-]*:\/\/)[^\s/@]+@/gi, '$1[redacted]@'],
  [/\bsk-[A-Za-z0-9_-]{8,}/g, 'sk-[redacted]'],
  [/\beyJ[\w-]+\.[\w-]+\.[\w-]+/g, '[jwt]'],
  [/\b(bearer|basic)\s+[^\s"',;]+/gi, '$1 [redacted]'],
  [/\b((?:api[_-]?key|x-api-key|access[_-]?token|refresh[_-]?token|id[_-]?token|token|secret|client[_-]?secret|password|passwd|authorization)["']?\s*[:=]\s*["']?)(?!\[redacted\]|bearer |basic )[^\s"',&;]+/gi, '$1[redacted]'],
  [/[A-Za-z]:[\\/]+Users[\\/]+[^\\/\s"']+/gi, '~'],
  [/\/(?:Users|home)\/[^/\s"']+/g, '~'],
  [/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/g, '[email]'],
];

/** `text` with secrets, credentials, e-mail addresses and home-folder paths replaced. */
export function redact(text) {
  let s = String(text ?? '');
  for (const [re, to] of RULES) s = s.replace(re, to);
  return s;
}

/** The report "Copy diagnostics" puts on the clipboard, redacted as a whole.
 *  `servers`: mcp_status entries (names only go in); `log`: the log's last lines. */
export function diagnosticsReport({ version, platform, userAgent, settings = {}, servers = [], log = '' }) {
  const disabled = new Set(settings.disabled_mcp_servers || []);
  const names = servers.filter((s) => s.status !== 'disabled' && !disabled.has(s.name)).map((s) => s.name);
  return redact([
    `ClaudeRust ${version || '?'}`,
    `OS: ${platform || '?'} (${userAgent || ''})`,
    `Chat engine: ${settings.chat_engine || 'auto'}; code engine: ${settings.code_engine || 'auto'}; API key: ${settings.has_api_key ? 'yes' : 'no'}`,
    `MCP servers on: ${names.join(', ') || 'none'}`,
    '',
    '--- log (last lines) ---',
    log || '(empty)',
  ].join('\n'));
}

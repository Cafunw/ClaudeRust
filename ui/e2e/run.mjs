// Run the e2e suite against a dev copy started here, the way CI runs it:
//
//   node ui/e2e/run.mjs [tauri config, default dev/e2e.conf.json]
//
// The copy gets the config's own identifier (its own settings and chats), the
// test key, the debugging port the tests drive, and CLAUDE_CONFIG_DIR on an
// empty folder, so a Code session runs none of your own Claude Code hooks.
// It is stopped at the end; its output (and, on failure, the app's log) goes
// to target/e2e. Exits with the tests' exit code. Windows only (WebView2).
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const config = path.resolve(process.argv[2] || path.join(root, 'dev', 'e2e.conf.json'));
/** The first of these ports this machine lets us listen on (a Windows runner reserves some ranges, and then the WebView's debugging port never opens). */
async function freePort(candidates) {
  for (const port of candidates) {
    const ok = await new Promise((resolve) => {
      const s = net.createServer();
      s.once('error', () => resolve(false));
      s.listen(port, '127.0.0.1', () => s.close(() => resolve(true)));
    });
    if (ok) return port;
  }
  throw new Error('no free port for the WebView debugging endpoint');
}
const PORT = process.env.CLAUDERUST_E2E_PORT ? Number(process.env.CLAUDERUST_E2E_PORT) : await freePort([9333, 9222, 9444, 19333, 29333]);
const CDP = `http://127.0.0.1:${PORT}`;
// a first build of the dev copy takes minutes; the CI job's own limit is 30
const START_MS = 20 * 60_000;
const logs = path.join(root, 'target', 'e2e');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The CDP targets, or null when nothing listens. */
async function targets() {
  try {
    return await (await fetch(`${CDP}/json`, { signal: AbortSignal.timeout(2000) })).json();
  } catch {
    return null;
  }
}

if (await targets()) {
  console.error(`Something already listens on ${CDP}; stop it first (the tests would drive it instead).`);
  process.exit(1);
}

fs.mkdirSync(logs, { recursive: true });
const devLog = path.join(logs, 'dev.log');
const out = fs.openSync(devLog, 'w');
const home = fs.mkdtempSync(path.join(os.tmpdir(), 'clauderust-e2e-config-'));
const env = {
  ...process.env,
  CLAUDERUST_API_KEY: 'sk-ant-test',
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
  CLAUDE_CONFIG_DIR: home,
};
const tauri = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
console.log(`Starting the dev copy (${path.relative(root, config)}); its output: ${devLog}`);
const dev = spawn(process.execPath, [tauri, 'dev', '--no-watch', '--config', config], {
  cwd: root, env, stdio: ['ignore', out, out], detached: process.platform !== 'win32',
});
let exited = null;
dev.on('exit', (code) => { exited = code ?? 'a signal'; });

function stop() {
  // the copy runs under cargo under the Tauri CLI: end the whole tree
  if (exited === null) {
    if (process.platform === 'win32') spawnSync('taskkill', ['/pid', String(dev.pid), '/T', '/F'], { stdio: 'ignore' });
    else process.kill(-dev.pid, 'SIGTERM');
  }
  fs.rmSync(home, { recursive: true, force: true });
}
process.on('SIGINT', () => { stop(); process.exit(130); });

/** The last `n` characters of the dev copy's output (CI shows nothing but this job's own log). */
function tail(n) {
  const text = fs.readFileSync(devLog, 'utf8');
  return text.slice(-n).trim();
}

/** The app's own log, which says where its start stopped (the dev copy itself prints nothing once it runs). */
function appLogTail(n) {
  try {
    const identifier = JSON.parse(fs.readFileSync(config, 'utf8')).identifier;
    return fs.readFileSync(path.join(process.env.APPDATA, identifier, 'logs', 'clauderust.log'), 'utf8').slice(-n).trim();
  } catch (e) {
    return `(no app log: ${e.code || e.message})`;
  }
}

/** Where the start stopped, from the OS: the app's and WebView2's processes with their command lines, the port, the data folders. */
function processInfo() {
  if (process.platform !== 'win32') return '';
  const identifier = JSON.parse(fs.readFileSync(config, 'utf8')).identifier;
  const ps = [
    "Get-CimInstance Win32_Process | Where-Object { $_.Name -in 'clauderust.exe','msedgewebview2.exe' } | ForEach-Object { $c = [string]$_.CommandLine; if ($c.Length -gt 300) { $c = $c.Substring(0, 300) }; '{0} {1} {2}' -f $_.ProcessId, $_.Name, $c }",
    "Get-Process clauderust -ErrorAction SilentlyContinue | ForEach-Object { 'clauderust responding: ' + $_.Responding + ', window: ' + $_.MainWindowHandle }",
    `netstat -ano | Select-String ':${PORT}'`,
    'netsh interface ipv4 show excludedportrange protocol=tcp',
    `foreach ($d in "$env:APPDATA\\${identifier}", "$env:LOCALAPPDATA\\${identifier}") { 'folder ' + $d; Get-ChildItem -Recurse -Depth 3 $d -ErrorAction SilentlyContinue | Select-Object -First 25 | ForEach-Object { '  ' + $_.FullName + ' ' + $_.Length } }`,
    `Get-ChildItem -Recurse "$env:LOCALAPPDATA\\${identifier}" -Filter DevToolsActivePort -ErrorAction SilentlyContinue | ForEach-Object { 'DevToolsActivePort: ' + (Get-Content $_.FullName -Raw) }`,
  ].join('; ');
  return spawnSync('powershell', ['-NoProfile', '-Command', ps], { encoding: 'utf8' }).stdout.trim();
}

/** What the debugging port answers and how many WebView2 processes exist, for the heartbeat. */
async function portState() {
  const t = await targets();
  const webviews = process.platform === 'win32'
    ? (spawnSync('tasklist', ['/FI', 'IMAGENAME eq msedgewebview2.exe', '/NH'], { encoding: 'utf8' }).stdout.match(/msedgewebview2\.exe/g) || []).length
    : 0;
  return `port ${PORT}: ${t === null ? 'closed' : JSON.stringify(t.map((x) => `${x.type} ${String(x.url).slice(0, 60)}`))}; WebView2 processes: ${webviews}`;
}

let code = 1;
try {
  const t0 = Date.now();
  let beat = t0;
  let infoShown = false;
  // the app's page (from the Tauri CLI's dev server), not the blank one the window starts with
  while (!(await targets())?.some((t) => t.type === 'page' && /^(https?|tauri):/.test(t.url))) {
    if (exited !== null) throw new Error(`the dev copy exited (${exited}) before its window was up; see ${devLog}`);
    if (Date.now() - t0 > START_MS) throw new Error(`the dev copy's window was not up after ${START_MS / 60_000} minutes; see ${devLog}`);
    // a heartbeat, so a slow build can be told from a stuck start in the job's log
    if (Date.now() - beat > 60_000) {
      beat = Date.now();
      const last = tail(300).split(/\r?\n/).pop();
      console.log(`[${Math.round((beat - t0) / 1000)} s] waiting for the window; the dev copy's last line: ${last}; ${await portState()}`);
      // once, a few minutes in (the build is done by then): what the OS says about the app and WebView2
      if (!infoShown && beat - t0 > 6 * 60_000) {
        infoShown = true;
        console.log(`--- processes and folders ---\n${processInfo()}`);
      }
    }
    await sleep(1000);
  }
  console.log(`The dev copy is up after ${Math.round((Date.now() - t0) / 1000)} s; running the tests.`);
  const files = fs.readdirSync(path.join(root, 'ui', 'e2e')).filter((f) => f.endsWith('.test.mjs')).map((f) => path.join('ui', 'e2e', f));
  // CLAUDERUST_E2E_REQUIRED: a test that would skip fails instead
  code = spawnSync(process.execPath, ['--test', '--test-concurrency=1', ...files], { cwd: root, stdio: 'inherit', env: { ...process.env, CLAUDERUST_E2E_REQUIRED: '1', CLAUDERUST_E2E_PORT: String(PORT) } }).status ?? 1;
} catch (e) {
  console.error(e.message);
  console.error(`--- the dev copy's output, last lines ---\n${tail(3000)}`);
  console.error(`--- the app's own log, last lines ---\n${appLogTail(6000)}`);
  console.error(`--- ${await portState()}\n${processInfo()}`);
} finally {
  stop();
}
if (code !== 0 && process.env.APPDATA) {
  const identifier = JSON.parse(fs.readFileSync(config, 'utf8')).identifier;
  const appLog = path.join(process.env.APPDATA, identifier, 'logs', 'clauderust.log');
  if (fs.existsSync(appLog)) fs.copyFileSync(appLog, path.join(logs, 'app.log'));
}
process.exit(code);

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
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const config = path.resolve(process.argv[2] || path.join(root, 'dev', 'e2e.conf.json'));
const CDP = 'http://127.0.0.1:9333';
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
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=9333',
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

let code = 1;
try {
  const t0 = Date.now();
  let beat = t0;
  // the app's page (from the Tauri CLI's dev server), not the blank one the window starts with
  while (!(await targets())?.some((t) => t.type === 'page' && /^(https?|tauri):/.test(t.url))) {
    if (exited !== null) throw new Error(`the dev copy exited (${exited}) before its window was up; see ${devLog}`);
    if (Date.now() - t0 > START_MS) throw new Error(`the dev copy's window was not up after ${START_MS / 60_000} minutes; see ${devLog}`);
    // a heartbeat, so a slow build can be told from a stuck start in the job's log
    if (Date.now() - beat > 60_000) {
      beat = Date.now();
      const last = tail(300).split(/\r?\n/).pop();
      console.log(`[${Math.round((beat - t0) / 1000)} s] waiting for the window; the dev copy's last line: ${last}`);
    }
    await sleep(1000);
  }
  console.log(`The dev copy is up after ${Math.round((Date.now() - t0) / 1000)} s; running the tests.`);
  const files = fs.readdirSync(path.join(root, 'ui', 'e2e')).filter((f) => f.endsWith('.test.mjs')).map((f) => path.join('ui', 'e2e', f));
  // CLAUDERUST_E2E_REQUIRED: a test that would skip fails instead
  code = spawnSync(process.execPath, ['--test', '--test-concurrency=1', ...files], { cwd: root, stdio: 'inherit', env: { ...process.env, CLAUDERUST_E2E_REQUIRED: '1' } }).status ?? 1;
} catch (e) {
  console.error(e.message);
  console.error(`--- the dev copy's output, last lines ---\n${tail(6000)}`);
} finally {
  stop();
}
if (code !== 0 && process.env.APPDATA) {
  const identifier = JSON.parse(fs.readFileSync(config, 'utf8')).identifier;
  const appLog = path.join(process.env.APPDATA, identifier, 'logs', 'clauderust.log');
  if (fs.existsSync(appLog)) fs.copyFileSync(appLog, path.join(logs, 'app.log'));
}
process.exit(code);

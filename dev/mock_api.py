"""Mock Claude API for end-to-end tests of ClaudeRust (streams SSE)."""
import json, sys, time, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = sys.argv[2] if len(sys.argv) > 2 else "/tmp/mock_requests.jsonl"
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
lock = threading.Lock()

MODELS = [{"id": m, "display_name": n, "type": "model"} for m, n in [
    ("claude-opus-5-5", "Claude Opus 5.5"), ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
    ("claude-haiku-5-5", "Claude Haiku 5.5"), ("claude-fable-5-2", "Claude Fable 5.2"),
    ("claude-opus-4-8", "Claude Opus 4.8")]]

RICH = """Here's a quick overview with **markdown**, a table, math and code.

## Results

| Language | Speed | Memory |
|---|---|---|
| Rust | Fast | Low |
| Python | Slower | Higher |

The famous identity is $e^{i\\pi} + 1 = 0$, and the Gaussian integral:

$$\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx = \\sqrt{\\pi}$$

```rust
fn main() {
    let greeting = "Hello from Rust";
    println!("{greeting}!");
}
```

- First point with `inline code`
- Second point, see https://www.rust-lang.org.

> Tip: Rust's ownership model prevents data races at compile time.
"""

ARTIFACT = """I'll build a small interactive counter for you.

<antArtifact identifier="counter-app" type="text/html" title="Counter App">
<!doctype html>
<html><head><style>
body{font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#fdf6f0}
button{font-size:22px;padding:10px 22px;border-radius:12px;border:none;background:#c96442;color:#fff;cursor:pointer}
h1{font-size:64px;margin:0 0 16px;text-align:center;color:#3d3d3a}
</style></head><body><div><h1 id="n">0</h1><button onclick="document.getElementById('n').textContent=+document.getElementById('n').textContent+1">Increment</button></div></body></html>
</antArtifact>

Click **Increment** to count up. Let me know if you want styling changes!"""


def log(entry):
    with lock:
        with open(LOG, "a") as f:
            f.write(json.dumps(entry) + "\n")


def last_user_text(body):
    msgs = body.get("messages", [])
    for m in reversed(msgs):
        if m["role"] != "user":
            continue
        c = m["content"]
        if isinstance(c, str):
            return c, m
        texts = [b.get("text", "") for b in c if b.get("type") == "text" and not b.get("text", "").startswith("<system-reminder>")]
        if any(b.get("type") == "tool_result" for b in c):
            return "__TOOL_RESULT__", m
        return "\n".join(texts), m
    return "", None


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            body = json.dumps({"data": MODELS, "has_more": False, "first_id": MODELS[0]["id"], "last_id": MODELS[-1]["id"]}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_error(404)

    def sse(self, ev):
        data = f"event: {ev['type']}\ndata: {json.dumps(ev)}\n\n".encode()
        self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
        self.wfile.flush()

    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        headers = {k.lower(): v for k, v in self.headers.items()}
        log({"path": self.path, "headers": {k: v for k, v in headers.items() if k != "x-api-key"}, "key_ok": headers.get("x-api-key") == "sk-ant-test", "body": body})
        if headers.get("x-api-key") != "sk-ant-test":
            err = json.dumps({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}}).encode()
            self.send_response(401)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(err)))
            self.end_headers()
            self.wfile.write(err)
            return
        text, _ = last_user_text(body)
        model = body.get("model", "claude-opus-5-5")
        if not body.get("stream"):
            out = json.dumps({"id": "msg_title", "type": "message", "role": "assistant", "model": model,
                              "content": [{"type": "text", "text": "Rust Overview Demo"}], "stop_reason": "end_turn",
                              "usage": {"input_tokens": 50, "output_tokens": 5}}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(out)))
            self.end_headers()
            self.wfile.write(out)
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        self.sse({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": model, "content": [],
                                                       "usage": {"input_tokens": 1200, "output_tokens": 1, "cache_read_input_tokens": 800}}})
        idx = 0
        has_tools = any(t.get("name") == "Glob" for t in body.get("tools", []))

        def stream_text(s, chunk=12, delay=0.02):
            nonlocal idx
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "text", "text": ""}})
            for i in range(0, len(s), chunk):
                self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "text_delta", "text": s[i:i + chunk]}})
                time.sleep(delay)
            self.sse({"type": "content_block_stop", "index": idx})
            idx += 1

        def thinking(s):
            nonlocal idx
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "thinking", "thinking": ""}})
            for i in range(0, len(s), 20):
                self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "thinking_delta", "thinking": s[i:i + 20]}})
                time.sleep(0.02)
            self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "signature_delta", "signature": "sig-" + str(time.time())}})
            self.sse({"type": "content_block_stop", "index": idx})
            idx += 1

        stop = "end_turn"
        if text == "__TOOL_RESULT__":
            stream_text("I found the files. The project has a `src` folder with the main entry point. Everything looks well organized.")
        elif "slow" in text.lower():
            for i in range(200):
                if i == 0:
                    self.sse({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
                self.sse({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": f"word{i} "}})
                time.sleep(0.05)
            self.sse({"type": "content_block_stop", "index": 0})
        elif has_tools and ("list" in text.lower() or "files" in text.lower()):
            thinking("The user wants to see the files. I'll use Glob to list them.")
            stream_text("Let me look at the project files.")
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "tool_use", "id": "toolu_glob1", "name": "Glob", "input": {}}})
            for part in ['{"pat', 'tern": "*', '*/*"}']:
                self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "input_json_delta", "partial_json": part}})
                time.sleep(0.05)
            self.sse({"type": "content_block_stop", "index": idx})
            idx += 1
            stop = "tool_use"
        elif any(t.get("name","").endswith("__get_forecast") for t in body.get("tools", [])) and "weather" in text.lower():
            name = [t["name"] for t in body["tools"] if t.get("name","").endswith("__get_forecast")][0]
            stream_text("I'll check the forecast.")
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "tool_use", "id": "toolu_mcp1", "name": name, "input": {}}})
            self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "input_json_delta", "partial_json": json.dumps({"city": "Istanbul"})}})
            self.sse({"type": "content_block_stop", "index": idx})
            idx += 1
            stop = "tool_use"
        elif has_tools and "edit" in text.lower():
            stream_text("I'll create the file now.")
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "tool_use", "id": "toolu_w1", "name": "Write", "input": {}}})
            self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "input_json_delta", "partial_json": json.dumps({"file_path": "hello.txt", "content": "hello world\n"})}})
            self.sse({"type": "content_block_stop", "index": idx})
            idx += 1
            stop = "tool_use"
        elif "artifact" in text.lower() or "app" in text.lower():
            stream_text(ARTIFACT, chunk=30)
        elif "search" in text.lower():
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}}})
            self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "input_json_delta", "partial_json": '{"query": "rust 2026 release"}'}})
            self.sse({"type": "content_block_stop", "index": idx}); idx += 1
            time.sleep(0.2)
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [
                {"type": "web_search_result", "url": "https://blog.rust-lang.org/2026/", "title": "Rust Blog", "encrypted_content": "x"},
                {"type": "web_search_result", "url": "https://www.example.com/news", "title": "Example News", "encrypted_content": "y"}]}})
            self.sse({"type": "content_block_stop", "index": idx}); idx += 1
            self.sse({"type": "content_block_start", "index": idx, "content_block": {"type": "text", "text": "", "citations": []}})
            self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "text_delta", "text": "Rust shipped several releases this year"}})
            self.sse({"type": "content_block_delta", "index": idx, "delta": {"type": "citations_delta", "citation": {"type": "web_search_result_location", "url": "https://blog.rust-lang.org/2026/", "title": "Rust Blog", "cited_text": "..."}}})
            self.sse({"type": "content_block_stop", "index": idx}); idx += 1
            stream_text(", with a focus on compile times.")
        elif "think" in text.lower():
            thinking("Let me reason about this carefully. First, consider the constraints. Then evaluate options step by step before answering.")
            stream_text("After thinking it through: **the answer is 42**.")
        elif "refuse" in text.lower():
            stop = "refusal"
        else:
            stream_text(RICH, chunk=24)
        self.sse({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 321}})
        self.sse({"type": "message_stop"})
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()

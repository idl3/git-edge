#!/usr/bin/env python3
"""Reference receiver for git-edge push events (and the conformance EVENTS run).

Verifies x-ge-signature (v1=HMAC-SHA256(secret, "v1\\n<ts>\\n<delivery>\\n" + body)),
bounds x-ge-timestamp, dedups on x-ge-delivery, and appends verified events as
JSON lines. Usage:
  GE_PUSH_WEBHOOK_SECRET=... push_event_receiver.py <port> <log.jsonl> [--fail-first]
"""
import hashlib, hmac, json, os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

SECRET = os.environ["GE_PUSH_WEBHOOK_SECRET"].encode()
PORT, LOG = int(sys.argv[1]), sys.argv[2]
FAIL_FIRST = "--fail-first" in sys.argv
MAX_SKEW_MS = 5 * 60 * 1000
seen, state = set(), {"n": 0}

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        ts, dl = self.headers.get("x-ge-timestamp", ""), self.headers.get("x-ge-delivery", "")
        want = "v1=" + hmac.new(SECRET, f"v1\n{ts}\n{dl}\n".encode() + body, hashlib.sha256).hexdigest()
        if not hmac.compare_digest(want, self.headers.get("x-ge-signature", "")):
            return self.reply(401)
        if not ts.isdigit() or abs(time.time() * 1000 - int(ts)) > MAX_SKEW_MS:
            return self.reply(401)
        state["n"] += 1
        if FAIL_FIRST and state["n"] == 1:
            return self.reply(503)  # exercises the sender's retry with the same delivery id
        if dl not in seen:
            seen.add(dl)
            with open(LOG, "a") as f:
                f.write(json.dumps({"delivery": dl, "body": json.loads(body)}) + "\n")
        self.reply(200)

    def reply(self, code):
        self.send_response(code)
        self.end_headers()

    def log_message(self, *a):
        pass

HTTPServer(("127.0.0.1", PORT), H).serve_forever()

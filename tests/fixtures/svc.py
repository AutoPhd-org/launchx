#!/usr/bin/env python3
"""A level-2 test service. Behaviour is set by FIXTURE_MODE:

ok       follows the contract (health, state, events, an echo Upgrade, SIGTERM)
crash    exits 1 at once
noauth   answers without checking the token
"""
import json
import os
import signal
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODE = os.environ.get("FIXTURE_MODE", "ok")
SERVICE = os.environ.get("LAUNCHX_SERVICE", "fixture")
TOKEN = os.environ.get("LAUNCHX_TOKEN", "")
STATE = {"service": SERVICE, "counter": 0, "dependency": os.environ.get("LAUNCHX_DEP_ALPHA_URL")}

if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("fixture 1.2.3")
    raise SystemExit(0)
if len(sys.argv) > 1 and sys.argv[1] == "state":
    import urllib.request
    home = os.environ["LAUNCHX_HOME"]
    run = json.load(open(os.path.join(home, "run", SERVICE + ".json")))
    request = urllib.request.Request(run["url"] + "/api/state", headers={"Authorization": "Bearer " + run["token"]})
    print(urllib.request.urlopen(request).read().decode())
    raise SystemExit(0)
if MODE == "crash":
    print("crashing on purpose", flush=True)
    raise SystemExit(1)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply(self, code, body, kind="application/json"):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self):
        if MODE == "noauth" or self.headers.get("Authorization") == "Bearer " + TOKEN:
            return True
        self.reply(401, '{"error":"Unauthorized"}')
        return False

    def do_GET(self):
        if not self.authorized():
            return
        if self.path == "/api/health":
            self.reply(200, json.dumps({"id": SERVICE, "version": "1.2.3", "status": "ok",
                                        "summary": "counter at {}".format(STATE["counter"])}))
        elif self.path == "/api/state":
            self.reply(200, json.dumps(STATE))
        elif self.path == "/api/events":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            try:
                for _ in range(3):
                    self.wfile.write(b": heartbeat\n\nevent: change\ndata: {}\n\n")
                    self.wfile.flush()
                    time.sleep(0.2)
            except OSError:
                pass
            self.close_connection = True
        elif self.path == "/echo" and self.headers.get("Upgrade") == "echo":
            self.send_response(101)
            self.send_header("Upgrade", "echo")
            self.send_header("Connection", "Upgrade")
            self.end_headers()
            self.wfile.flush()
            data = self.connection.recv(1024)
            self.connection.sendall(b"echo:" + data)
            self.close_connection = True
        elif self.path == "/headers":
            self.reply(200, json.dumps({k.lower(): v for k, v in self.headers.items()}))
        else:
            self.reply(200, "<!doctype html><title>{}</title><p>fixture page for {}</p>".format(SERVICE, SERVICE), "text/html")

    def do_POST(self):
        if not self.authorized():
            return
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        STATE["counter"] += 1
        self.reply(200, json.dumps({"counter": STATE["counter"]}))


server = ThreadingHTTPServer((os.environ.get("LAUNCHX_HOST", "127.0.0.1"), int(os.environ.get("LAUNCHX_PORT", "0"))), Handler)
server.daemon_threads = True
signal.signal(signal.SIGTERM, lambda *_: threading.Thread(target=server.shutdown).start())
print("fixture {} listening on {}".format(SERVICE, server.server_address[1]), flush=True)
server.serve_forever(poll_interval=0.1)
print("fixture {} stopped".format(SERVICE), flush=True)

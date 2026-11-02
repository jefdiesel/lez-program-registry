# Test helper (scripts/test-not-included.sh): JSON-RPC proxy to $UPSTREAM (default 127.0.0.1:3062) that answers getTransaction with null (a tx that "never shows up").
import http.server, json, os, sys, urllib.request
UP = os.environ.get("UPSTREAM", "http://127.0.0.1:3062")
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        req = json.loads(body)
        if isinstance(req, dict) and req.get("method") == "getTransaction":
            out = json.dumps({"jsonrpc": "2.0", "id": req.get("id"), "result": None}).encode()
            with open(sys.argv[2], "a") as f: f.write("getTransaction\n")
        else:
            r = urllib.request.Request(UP, data=body, headers={"Content-Type": "application/json"})
            out = urllib.request.urlopen(r, timeout=60).read()
            if isinstance(req, dict):
                with open(sys.argv[2], "a") as f: f.write(req.get("method", "?") + "\n")
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()

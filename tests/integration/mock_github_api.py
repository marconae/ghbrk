#!/usr/bin/env python3
"""Minimal HTTPS mock for the GitHub API -- test-only."""
import http.server
import json
import ssl

CERT_DIR = "/certs"


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/api/v3/user":
            auth = self.headers.get("Authorization", "")
            if auth and auth != "bearer ":
                body = json.dumps(
                    {"login": "test-user", "id": 1, "type": "User"}
                ).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            else:
                body = json.dumps({"message": "Bad credentials"}).encode()
                self.send_response(401)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, fmt, *args):
        print(fmt % args, flush=True)


ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(f"{CERT_DIR}/mock-github.crt", f"{CERT_DIR}/mock-github.key")
server = http.server.HTTPServer(("0.0.0.0", 443), Handler)
server.socket = ctx.wrap_socket(server.socket, server_side=True)
print("mock-github HTTPS listening on :443", flush=True)
server.serve_forever()

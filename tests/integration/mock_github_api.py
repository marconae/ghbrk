#!/usr/bin/env python3
"""Minimal HTTPS mock for the GitHub API -- test-only.

Serves just enough of the REST and GraphQL surface for `gh api`, `gh pr
create`, and `gh pr comment` to complete against a fake `mock-github` host.
Every GraphQL request's variables are appended to `/tmp/graphql-requests.log`
as one JSON object per line, so the integration harness can assert on the
body text a command actually sent -- independent of whether `gh` goes on to
report success.
"""
import http.server
import json
import re
import ssl

CERT_DIR = "/certs"
GRAPHQL_LOG = "/tmp/graphql-requests.log"


def log_graphql_request(query, variables):
    with open(GRAPHQL_LOG, "a") as f:
        f.write(json.dumps({"query": query, "variables": variables}) + "\n")


def repository_payload(owner, name):
    return {
        "id": "R_kgatestrepo",
        "name": name,
        "owner": {"login": owner},
        "hasIssuesEnabled": True,
        "description": "",
        "hasWikiEnabled": True,
        "viewerPermission": "WRITE",
        "defaultBranchRef": {"name": "main"},
        "parent": None,
        "mergeCommitAllowed": True,
        "rebaseMergeAllowed": True,
        "squashMergeAllowed": True,
    }


class Handler(http.server.BaseHTTPRequestHandler):
    def _send_json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/api/v3/user":
            auth = self.headers.get("Authorization", "")
            if auth and auth != "bearer ":
                self._send_json(200, {"login": "test-user", "id": 1, "type": "User"})
            else:
                self._send_json(401, {"message": "Bad credentials"})
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""

        if self.path != "/api/graphql":
            self.send_response(404)
            self.end_headers()
            return

        try:
            payload = json.loads(raw.decode("utf-8")) if raw else {}
        except ValueError:
            payload = {}
        query = payload.get("query", "")
        variables = payload.get("variables", {})
        log_graphql_request(query, variables)

        if "pullRequest(number:" in query:
            number = variables.get("pr_number")
            owner = variables.get("owner")
            repo = variables.get("repo")
            self._send_json(
                200,
                {
                    "data": {
                        "repository": {
                            "pullRequest": {
                                "id": "PR_kgatestpr",
                                "url": f"https://mock-github/{owner}/{repo}/pull/{number}",
                                "number": number,
                            }
                        }
                    }
                },
            )
            return

        if "addComment(" in query:
            self._send_json(
                200,
                {
                    "data": {
                        "addComment": {
                            "commentEdge": {
                                "node": {
                                    "url": "https://mock-github/acme/web/pull/1#comment",
                                }
                            }
                        }
                    }
                },
            )
            return

        if "query RepositoryInfo" in query or "query RepositoryFindStale" in query:
            owner = variables.get("owner")
            name = variables.get("name")
            self._send_json(
                200, {"data": {"repository": repository_payload(owner, name)}}
            )
            return

        if "pullRequests(" in query or "PullRequestForBranch" in query:
            self._send_json(
                200,
                {
                    "data": {
                        "repository": {
                            "pullRequests": {"nodes": []},
                            "defaultBranchRef": {"name": "main"},
                        }
                    }
                },
            )
            return

        if re.search(r"mutation\s+\w*CreatePullRequest", query) or "createPullRequest(" in query:
            body = variables.get("input", {}).get("body", "")
            self._send_json(
                200,
                {
                    "data": {
                        "createPullRequest": {
                            "pullRequest": {
                                "id": "PR_kgatestpr",
                                "url": "https://mock-github/acme/web/pull/1",
                                "number": 1,
                                "body": body,
                            }
                        }
                    }
                },
            )
            return

        if "query UserCurrent" in query or "viewer {" in query:
            self._send_json(200, {"data": {"viewer": {"login": "test-user"}}})
            return

        self._send_json(200, {"data": {}})

    def log_message(self, fmt, *args):
        print(fmt % args, flush=True)


ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(f"{CERT_DIR}/mock-github.crt", f"{CERT_DIR}/mock-github.key")
server = http.server.HTTPServer(("0.0.0.0", 443), Handler)
server.socket = ctx.wrap_socket(server.socket, server_side=True)
print("mock-github HTTPS listening on :443", flush=True)
server.serve_forever()

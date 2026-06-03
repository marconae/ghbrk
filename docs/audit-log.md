[ghbrk](../README.md) / [Docs](./index.md) / Audit Log

---

# Audit Log

Every allow and deny decision is appended as a JSON line to `/var/log/ghbrk/audit.log` (mode `0640`). Token and key material never appear in this file.

## Format

Each line is a JSON object with these fields:

| Field | Description |
|-------|-------------|
| `timestamp` | ISO 8601 UTC timestamp |
| `user` | Unix username of the calling process |
| `tool` | `git` or `gh` |
| `args` | Full argument list as passed to the broker |
| `org` | Resolved GitHub organisation |
| `repo` | Resolved repository name |
| `branch` | Resolved branch (git push only; omitted otherwise) |
| `operation` | Operation name — see [Operations reference](./policy.md#operations-reference) |
| `decision` | `"allow"`, `"passthrough"`, or `{"deny": {"reason": "..."}}` |

## Example entries

```json
{"timestamp":"2026-04-27T09:12:00Z","user":"alice","tool":"git","args":["push","origin","feature/ui"],"org":"acme","repo":"platform","branch":"feature/ui","operation":"push","decision":"allow"}
{"timestamp":"2026-04-27T09:13:44Z","user":"alice","tool":"git","args":["push","origin","main"],"org":"acme","repo":"platform","branch":"main","operation":"push","decision":{"deny":{"reason":"no matching rule"}}}
{"timestamp":"2026-04-27T09:15:02Z","user":"alice","tool":"gh","args":["pr","list"],"org":"acme","repo":"platform","operation":"gh_api_read","decision":"passthrough"}
```

## Following the log

```bash
sudo tail -f /var/log/ghbrk/audit.log | jq .
```

Members of `ghbrk-clients` can read the log directly:

```bash
tail -f /var/log/ghbrk/audit.log | jq .
```

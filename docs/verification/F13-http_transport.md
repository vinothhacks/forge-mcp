# F13: HTTP transport

**Verdict: PASS** (no fix, no rebuild). Binary SHA-256 `ae7389b81b8e5db4041d0b4267b3cfe27a9e66fdc62ba0a4e023e954b3e9064e`. Date: 2026-11-15.

| # | Criterion | Result |
|---|---|---|
| 1 | `GET /health` without token returns 200 `{status, service, version}` | PASS |
| 2 | `POST /mcp`: no auth 401 (-32001), wrong token 401, lowercase `bearer` 200, `?token=` 200, wrong header + right query 401 | PASS |
| 3 | `POST /message` behaves like `/mcp` | PASS |
| 4 | Notification returns 202 `{}` | PASS |
| 5 | `GET /sse`: `text/event-stream`, first event `endpoint` / `/message`, later responses arrive as `message` events; 401 without token | PASS |
| 6 | `Content-Type: text/plain` | Documented: 415 plain text (axum `Json` extractor) |
| 7 | Batch `[initialize, ping]` | PASS |
| 8 | `--allow-root` rejects outside paths with a JSON-RPC error, accepts inside | PASS |
| 9 | `workspace_open`, `workspace_status`, `code_search`, `symbol_find` over HTTP | PASS |
| B | `--auth-token ""` disables auth | PASS |
| C | `FORGE_AUTH_TOKEN`, `FORGE_PORT`, `FORGE_TRANSPORT`, single-value `FORGE_ALLOW_ROOT` | PASS |
| 10 | `--transport sse` alias, `--transport bogus` exits 1, `--host 0.0.0.0` binds | PASS |
| 11 | Port in use exits 1 with OS error 10048 | PASS |
| 12 | Malformed JSON body returns 400; server survives; no 5xx anywhere | PASS |

H8 confirmed on all three points and left as designed: non-JSON content types get a plain 415/400 (not JSON-RPC), notifications get 202, SSE `endpoint` data is relative.

Known limitations (documented in README, not changed):
- `FORGE_ALLOW_ROOT` accepts one path only. Use repeated `--allow-root` flags for several roots.
- Every POST response is broadcast to all `/sse` subscribers; responses are not routed per session.
- Lagging SSE subscribers drop messages silently (channel capacity 128).
- Bearer token comparison is not constant-time.

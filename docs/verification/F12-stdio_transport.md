# F12: stdio transport and MCP protocol

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `47c99121...8a5a`. Date: 2026-11-15.

| Criterion | Before fix | After fix |
|---|---|---|
| `initialize` returns `serverInfo` forge-mcp 0.1.0 and `tools.listChanged:false` | PASS | PASS |
| `notifications/initialized` produces no output | PASS | PASS |
| `ping` returns `{}`; `tools/list` returns exactly the 11 tools | PASS | PASS |
| Every tool schema is an object whose `required` keys exist in `properties` | PASS | PASS |
| Batch requests answered as an array; notification-only batch gives no output | PASS | PASS |
| Unknown method -32601; malformed JSON -32700 with `id: null`; unknown tool and bad argument types -32000 | PASS | PASS |
| String and `0` ids echoed exactly | PASS | PASS |
| `Content-Length` framing uses UTF-8 byte length (non-ASCII verified); lowercase header and extra headers accepted | PASS | PASS |
| Mixed newline / Content-Length / newline in one session | PASS | PASS |
| Clients requesting 2025-06-18 and 2025-03-26 get valid responses | PASS | PASS |
| H7: protocol version negotiated against a supported list; `instructions` returned | **FAIL** (any string echoed, no instructions) | PASS |
| EOF exits 0; stdout carries only protocol frames | PASS | PASS |

Additional defects found by edge probes, all fixed in `src/mcp.rs`:

| Probe | Before fix | After fix |
|---|---|---|
| Notification with a request method (`ping`, `tools/list`) | Answered with `id: null` (violates JSON-RPC 2.0 §4.1) | No reply |
| Empty batch `[]` | No reply | -32600 |
| Request without `method` | -32601 | -32600 |
| Batch `[1]` | No reply | -32600 |
| Invalid UTF-8 line | **Server exited** | -32700, server keeps running |
| `Content-Length: 99999999999999999` | **Process aborted** (allocation failure) | Frame drained and rejected (64 MiB cap), server keeps running |
| Frame truncated at EOF | Exit 1 | Exit 0 with a warning on stderr |

**Fix:** `negotiate_protocol_version` with `SUPPORTED_PROTOCOL_VERSIONS` (2025-06-18, 2025-03-26, 2024-11-05) and `SERVER_INSTRUCTIONS`; notifications are executed but never answered; invalid requests return -32600; `run_stdio` reads raw bytes, tolerates invalid UTF-8, caps frames at `MAX_STDIO_FRAME_BYTES`, and exits cleanly on truncated input. Over HTTP, notifications now always return 202.

**Regression test:** `test_stdio_jsonrpc_e2e_subprocess` extended with batch, version negotiation, notification silence, -32700, invalid UTF-8, -32600 and truncated-frame cases. It fails on the old binary and passes on the new one.

The real-client check with `@modelcontextprotocol/inspector` could not run (no network); a Node client covered the same handshake. The schema-vs-struct audit from this phase is in `schema-vs-model.md`.

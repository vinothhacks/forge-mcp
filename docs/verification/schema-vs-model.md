# Tool schemas vs request structs

Consolidated from the audits in F2–F15. "Schema" is `ForgeMcpServer::tool_definitions()` in `src/mcp.rs`; "struct" is the request type in `src/model.rs`.

| # | Tool | Mismatch | Resolution | Phase |
|---|---|---|---|---|
| 1 | `context_build` | Schema documented `snapshot_policy` values `require_current, allow_stale`; code honored only the undocumented `require_current_strict` | Fixed: all three values honored, unknown values rejected, schema has an `enum` | F10 |
| 2 | `context_build` | `token_budget` clamp (64..128000) undocumented | Fixed: stated in the schema description | F10 |
| 3 | `symbol_references` | `include_declarations` declared but never read | Fixed: adds the declaration site | F7 |
| 4 | `symbol_references` | `file_path` described as a disambiguator but did not restrict hits | Fixed: restricts hits; description updated | F7 |
| 5 | `symbol_find` | Unknown `kind` silently treated as `function` | Fixed: unknown kinds rejected with the accepted list; `enum` aliases `struct` | F5 |
| 6 | `symbol_find`, `code_search`, `context_build` | `language: "typescript"` / `"javascript"` excluded `.tsx` / `.jsx` (stored as `tsx` / `jsx`) | Fixed: parser stores `typescript` / `javascript` | F15 |
| 7 | `graph_query` | Accepted aliases `relation` and `symbol` not in the schema | Fixed: added to the schema | F8 |
| 8 | `graph_query` | Unknown `query_kind` silently ran `callers` | Fixed: rejected with the accepted list | F8 |
| 9 | `symbol_references`, `impact_analyze` | Accepted alias `symbol` not in the schema | Fixed in F16: added to both schemas | F16 |
| 10 | `code_search`, `symbol_find`, `symbol_references`, `graph_query` | `limit` clamps (1..100 or 1..500) and `graph_query` / `impact_analyze` `max_depth` clamp (1..8) undocumented | Fixed in F16: stated in the descriptions | F16 |
| 11 | `symbol_read` | "One of `symbol_id` or `name` is required" only enforced at runtime | Fixed in F16: stated in the description; runtime error is specific (F6) | F6, F16 |
| 12 | `job_status` | "One of `job_id` or `workspace_id` is required" only enforced at runtime | Fixed in F16: stated in the description | F16 |
| 13 | `graph_query` | Schema `enum` lists the 7 canonical kinds; the parser also accepts aliases such as `calls`, `inherits`, `shortest_path` | By design: the enum advertises canonical values; aliases stay accepted for compatibility | — |
| 14 | all | No `additionalProperties: false`; unknown argument names are ignored | By design: lenient input for agent clients; documented here | — |
| 15 | all | No `outputSchema`; results are described by the shared envelope in the README | By design for v0.1.0 | — |

Unresolved rows: none.

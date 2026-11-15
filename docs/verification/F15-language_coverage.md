# F15: Tree-sitter language coverage

**Verdict: FAIL-FIXED.** Binary before fix `ae7389b8...9064e`; shadow binary with fix `3d2db902...51a5`. Date: 2026-11-15.

| File | Language label (before / after) | Symbols as expected | Notes |
|---|---|---|---|
| `rust.rs` | rust / rust | yes | `///` docs; enum indexed as struct; `impl T for S` gives `implements`; `use` gives imports |
| `ts.ts` | typescript / typescript | yes | `/** */` docs; `extends` / `implements`; imports; calls |
| `tsx.tsx` | **tsx** / typescript | yes | TSX grammar still used |
| `js.js`, `js.mjs`, `js.cjs` | javascript / javascript | yes | ESM import, `require` call |
| `jsx.jsx` | **jsx** / javascript | yes | parsed by the JavaScript grammar |
| `py.py`, `pyi.pyi` | python / python | yes | `_private` visibility, docstrings, imports |
| `go.go` | go / go | method parent wrong before fix | `//` docs, import block |
| `broken.rs` | rust | yes | unbalanced brace still indexes, no error |
| `notes.txt` | not indexed | n/a | unsupported extensions are skipped |

**H6 confirmed.** `.tsx` and `.jsx` were labelled `tsx`/`jsx`, so `language: "typescript"` and `"javascript"` filters excluded them. They are now labelled `typescript`/`javascript`; grammar selection still uses the extension. Existing indexes keep the old labels until re-indexed with `force_reindex: true`.

**Go receiver bug (handed over from F8).** `func (r *HttpRouter) Dispatch` was indexed as `r.Dispatch` with no parent and no `contains` edge. Now `HttpRouter.Dispatch` with parent `HttpRouter`; generic receivers like `(b *Box[T])` also resolve. Limitation: the parent is linked only when the struct is in the same file.

**Regression test:** `test_language_coverage_matrix` (fails on the old parser, passes after).

Documented, not changed: a Rust `fn` inside `mod` is labelled `method`; `export const h = async () => {}` has no visibility; TS `extends` in a class with `implements` is stored as `implements`; Python base classes are stored as `implements`; Go symbols carry no exported visibility.

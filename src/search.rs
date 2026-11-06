use crate::embeddings::{cosine_similarity, split_code_tokens, EmbeddingEngine};
use crate::model::*;
use crate::store::ForgeStore;
use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::params;
use std::collections::{HashMap, HashSet};

pub fn is_test_path_or_symbol(file_path: &str, symbol_name: Option<&str>) -> bool {
    let fp = file_path.to_ascii_lowercase();
    if fp.contains("/test/")
        || fp.contains("/tests/")
        || fp.starts_with("tests/")
        || fp.starts_with("test/")
        || fp.ends_with("_test.rs")
        || fp.ends_with("_test.go")
        || fp.ends_with("_test.py")
        || fp.ends_with(".test.ts")
        || fp.ends_with(".test.js")
        || fp.ends_with(".spec.ts")
        || fp.ends_with(".spec.js")
        || fp.contains("test_")
    {
        return true;
    }
    if let Some(sn) = symbol_name {
        let sn_l = sn.to_ascii_lowercase();
        if sn_l.starts_with("test_") || sn_l.contains("::test_") {
            return true;
        }
    }
    false
}

/// Execute hybrid search (BM25 FTS5 + AST symbol match + cosine vector similarity) fused via RRF.
pub fn code_search(
    store: &ForgeStore,
    embedder: &EmbeddingEngine,
    req: &CodeSearchRequest,
) -> Result<ResponseEnvelope<CodeSearchHit>> {
    let (ws, snap, freshness, changed_files) =
        store.check_workspace_freshness(&req.workspace_id)?;
    let limit = req.limit.unwrap_or(15).clamp(1, 100);
    let include_tests = req.include_tests.unwrap_or(true);
    let lang_filter = req.language.as_ref().map(|l| l.to_ascii_lowercase());
    let prefix_filter = req.path_prefix.as_ref().map(|p| p.replace('\\', "/"));

    let chunks = store.list_snapshot_chunks(&snap.snapshot_id)?;
    let symbols = store.list_snapshot_symbols(&snap.snapshot_id)?;
    let embeddings = store.list_snapshot_embeddings(&snap.snapshot_id)?;

    let sym_by_id: HashMap<String, Symbol> = symbols
        .into_iter()
        .map(|s| (s.symbol_id.clone(), s))
        .collect();

    // Filter eligible chunks
    let eligible_chunks: Vec<CodeChunk> = chunks
        .into_iter()
        .filter(|c| {
            if !include_tests && is_test_path_or_symbol(&c.file_path, c.symbol_name.as_deref()) {
                return false;
            }
            if let Some(ref lf) = lang_filter {
                if &c.language.to_ascii_lowercase() != lf {
                    return false;
                }
            }
            if let Some(ref pf) = prefix_filter {
                if !c.file_path.starts_with(pf) && !c.file_path.contains(pf) {
                    return false;
                }
            }
            true
        })
        .collect();

    let query_tokens = split_code_tokens(&req.query);
    let query_lower = req.query.trim().to_ascii_lowercase();

    // 1. FTS5 BM25 Lexical scores
    let mut fts_scores: HashMap<String, f64> = HashMap::new();
    if !query_tokens.is_empty() {
        let fts_expr = query_tokens
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let _ = store.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT chunk_id, bm25(chunks_fts)
                FROM chunks_fts
                WHERE snapshot_id = ?1 AND chunks_fts MATCH ?2
                ORDER BY bm25(chunks_fts) ASC
                LIMIT 100
                "#,
            )?;
            let rows = stmt.query_map(params![snap.snapshot_id, fts_expr], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?;
            for r in rows.flatten() {
                // SQLite bm25() returns negative numbers where more negative = better match
                let positive_score = (-r.1).max(0.001);
                fts_scores.insert(r.0, positive_score);
            }
            Ok(())
        });
    }

    // Also compute direct lexical token overlap so queries with special chars or subtokens rank reliably
    for chunk in &eligible_chunks {
        let content_lower = chunk.content.to_ascii_lowercase();
        let mut overlap = 0.0;
        if !query_lower.is_empty() && content_lower.contains(&query_lower) {
            overlap += 3.0;
        }
        for tok in &query_tokens {
            if content_lower.contains(tok) {
                overlap += 1.0;
            }
        }
        if overlap > 0.0 {
            let entry = fts_scores.entry(chunk.chunk_id.clone()).or_insert(0.0);
            *entry += overlap;
        }
    }

    // 2. AST Symbol Name / Signature / Doc Comment scores
    let mut symbol_scores: HashMap<String, f64> = HashMap::new();
    for chunk in &eligible_chunks {
        let mut s_score = 0.0;
        if let Some(ref sid) = chunk.symbol_id {
            if let Some(sym) = sym_by_id.get(sid) {
                let name_l = sym.name.to_ascii_lowercase();
                let qual_l = sym.qualified_name.to_ascii_lowercase();
                let sig_l = sym.signature.to_ascii_lowercase();
                let doc_l = sym
                    .doc_comment
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase();

                if !query_lower.is_empty() {
                    if name_l == query_lower || qual_l == query_lower {
                        s_score += 10.0;
                    } else if name_l.contains(&query_lower) || qual_l.contains(&query_lower) {
                        s_score += 5.0;
                    }
                }
                for tok in &query_tokens {
                    if name_l == *tok {
                        s_score += 4.0;
                    } else if name_l.contains(tok) || qual_l.contains(tok) {
                        s_score += 2.5;
                    }
                    if sig_l.contains(tok) {
                        s_score += 1.0;
                    }
                    if doc_l.contains(tok) {
                        s_score += 1.2;
                    }
                }
            }
        }
        if s_score > 0.0 {
            symbol_scores.insert(chunk.chunk_id.clone(), s_score);
        }
    }

    // 3. Vector Cosine Similarity scores
    let query_vec = embedder.embed_text(&req.query);
    let mut vector_scores: HashMap<String, f64> = HashMap::new();
    for chunk in &eligible_chunks {
        if let Some(c_vec) = embeddings.get(&chunk.chunk_id) {
            let sim = cosine_similarity(&query_vec, c_vec);
            if sim > 0.01 {
                vector_scores.insert(chunk.chunk_id.clone(), sim);
            }
        }
    }

    // Sort ranks for Reciprocal Rank Fusion (RRF, k = 60)
    let lexical_ranks = compute_ranks(&fts_scores);
    let symbol_ranks = compute_ranks(&symbol_scores);
    let vector_ranks = compute_ranks(&vector_scores);

    let rrf_k = 60.0;
    let mut hits = Vec::new();

    for chunk in eligible_chunks {
        let cid = &chunk.chunk_id;
        let lex_s = fts_scores.get(cid).copied().unwrap_or(0.0);
        let sym_s = symbol_scores.get(cid).copied().unwrap_or(0.0);
        let vec_s = vector_scores.get(cid).copied().unwrap_or(0.0);

        if lex_s <= 0.0 && sym_s <= 0.0 && vec_s <= 0.10 {
            continue;
        }

        let mut rrf = 0.0;
        let mut reasons = Vec::new();

        if let Some(&r) = lexical_ranks.get(cid) {
            rrf += 1.2 / (rrf_k + r as f64);
            reasons.push("bm25_lexical".to_string());
        }
        if let Some(&r) = symbol_ranks.get(cid) {
            rrf += 1.5 / (rrf_k + r as f64);
            reasons.push("ast_symbol_match".to_string());
        }
        if let Some(&r) = vector_ranks.get(cid) {
            if vec_s > 0.05 {
                rrf += 1.0 / (rrf_k + r as f64);
                reasons.push("vector_similarity".to_string());
            }
        }

        let sym_meta = chunk.symbol_id.as_ref().and_then(|sid| sym_by_id.get(sid));

        hits.push(CodeSearchHit {
            chunk_id: chunk.chunk_id,
            file_path: chunk.file_path,
            language: chunk.language,
            symbol_id: chunk.symbol_id,
            symbol_name: chunk.symbol_name,
            symbol_kind: sym_meta.map(|s| s.kind),
            signature: sym_meta.map(|s| s.signature.clone()),
            source_range: chunk.source_range,
            content: chunk.content,
            content_hash: chunk.content_hash,
            rrf_score: (rrf * 10000.0).round() / 10000.0,
            lexical_score: (lex_s * 1000.0).round() / 1000.0,
            symbol_score: (sym_s * 1000.0).round() / 1000.0,
            vector_score: (vec_s * 1000.0).round() / 1000.0,
            match_reasons: reasons,
        });
    }

    hits.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.symbol_score
                    .partial_cmp(&a.symbol_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                b.lexical_score
                    .partial_cmp(&a.lexical_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let truncated = hits.len() > limit;
    let now = Utc::now().to_rfc3339();
    let mut results = Vec::new();
    let mut evidence = Vec::new();

    for hit in hits.into_iter().take(limit) {
        evidence.push(Evidence {
            repository_id: ws.repository_id.clone(),
            workspace_id: ws.workspace_id.clone(),
            snapshot_id: snap.snapshot_id.clone(),
            file_path: hit.file_path.clone(),
            source_range: hit.source_range,
            content_hash: hit.content_hash.clone(),
            producer: format!("forge-hybrid-rrf({})", embedder.model_name()),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now.clone(),
        });
        results.push(hit);
    }

    let mut limitations = vec![format!(
        "Hybrid retrieval fused via RRF (BM25 + Tree-sitter symbol match + {} embeddings).",
        embedder.model_name()
    )];
    if freshness == Freshness::Stale {
        limitations.push(format!(
            "Snapshot {} is stale; changed files: {}",
            snap.snapshot_id,
            changed_files.join(", ")
        ));
    }

    Ok(ResponseEnvelope::new(
        snap.snapshot_id,
        freshness,
        results,
        evidence,
        limitations,
        truncated,
        None,
    ))
}

fn compute_ranks(scores: &HashMap<String, f64>) -> HashMap<String, usize> {
    let mut pairs: Vec<(&String, &f64)> = scores.iter().filter(|(_, &v)| v > 0.0).collect();
    pairs.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut ranks = HashMap::new();
    for (idx, (k, _)) in pairs.into_iter().enumerate() {
        ranks.insert(k.clone(), idx + 1);
    }
    ranks
}

/// Build a token-budgeted, deduplicated context package using hybrid retrieval + 1-hop graph neighborhood expansion.
pub fn context_build(
    store: &ForgeStore,
    embedder: &EmbeddingEngine,
    req: &ContextBuildRequest,
) -> Result<ResponseEnvelope<ContextBuildResult>> {
    let (ws, snap, freshness, changed_files) =
        store.check_workspace_freshness(&req.workspace_id)?;

    if req.snapshot_policy.as_deref() == Some("require_current_strict")
        && freshness == Freshness::Stale
    {
        return Err(anyhow!(
            "Workspace snapshot {} is stale and snapshot_policy=require_current_strict",
            snap.snapshot_id
        ));
    }

    let token_budget = req.token_budget.unwrap_or(4000).clamp(64, 128_000);
    let include_tests = req.include_tests.unwrap_or(true);

    let search_env = code_search(
        store,
        embedder,
        &CodeSearchRequest {
            workspace_id: req.workspace_id.clone(),
            query: req.query.clone(),
            language: req.language.clone(),
            path_prefix: req.path_prefix.clone(),
            limit: Some(20),
            include_tests: Some(include_tests),
        },
    )?;

    let relationships = store.list_snapshot_relationships(&snap.snapshot_id)?;
    let symbols = store.list_snapshot_symbols(&snap.snapshot_id)?;
    let chunks = store.list_snapshot_chunks(&snap.snapshot_id)?;

    let sym_by_id: HashMap<String, Symbol> = symbols
        .iter()
        .cloned()
        .map(|s| (s.symbol_id.clone(), s))
        .collect();
    let chunk_by_sym_id: HashMap<String, CodeChunk> = chunks
        .into_iter()
        .filter_map(|c| c.symbol_id.clone().map(|sid| (sid, c)))
        .collect();

    let mut snippets = Vec::new();
    let mut evidence = Vec::new();
    let mut seen_ranges: HashSet<(String, usize, usize)> = HashSet::new();
    let mut seen_sym_ids: HashSet<String> = HashSet::new();
    let mut total_tokens_used = 0usize;
    let mut primary_matches = 0usize;
    let mut graph_expanded_items = 0usize;
    let mut budget_truncated = false;
    let now = Utc::now().to_rfc3339();

    // Pass 1: Seed with top primary hits (up to 2) and queue their 1-hop graph neighbors
    let seed_count = 2.min(search_env.results.len());
    let mut expansion_queue: Vec<(String, String, f64)> = Vec::new();

    for (idx, hit) in search_env.results.iter().take(seed_count).enumerate() {
        let key = (
            hit.file_path.clone(),
            hit.source_range.start_line,
            hit.source_range.end_line,
        );
        if seen_ranges.contains(&key) {
            continue;
        }
        let tok_count = estimate_tokens(&hit.content);
        if total_tokens_used + tok_count > token_budget {
            budget_truncated = true;
            // Try fitting a compact signature-only snippet if budget is tight and we have at least room for signature
            if let Some(ref sig) = hit.signature {
                let sig_tokens = estimate_tokens(sig);
                if total_tokens_used + sig_tokens <= token_budget && snippets.is_empty() {
                    total_tokens_used += sig_tokens;
                    seen_ranges.insert(key);
                    primary_matches += 1;
                    snippets.push(ContextSnippet {
                        file_path: hit.file_path.clone(),
                        language: hit.language.clone(),
                        symbol_id: hit.symbol_id.clone(),
                        symbol_name: hit.symbol_name.clone(),
                        symbol_kind: hit.symbol_kind,
                        signature: hit.signature.clone(),
                        source_range: hit.source_range,
                        content: sig.clone(),
                        content_hash: hit.content_hash.clone(),
                        token_count: sig_tokens,
                        inclusion_reason: format!(
                            "primary_search_match({}) [signature_truncated]",
                            hit.match_reasons.join("+")
                        ),
                        score: hit.rrf_score,
                        related_symbols: Vec::new(),
                    });
                }
            }
            continue;
        }

        seen_ranges.insert(key);
        if let Some(ref sid) = hit.symbol_id {
            seen_sym_ids.insert(sid.clone());
        }

        // Find related symbols (1-hop callers/callees/implements)
        let mut related = Vec::new();
        if let Some(ref sid) = hit.symbol_id {
            for rel in &relationships {
                if rel.from_symbol_id.as_deref() == Some(sid.as_str()) {
                    related.push(format!("{}:{}", rel.kind.as_str(), rel.to_name));
                    if idx < 5 {
                        if let Some(ref target_id) = rel.to_symbol_id {
                            expansion_queue.push((
                                target_id.clone(),
                                format!(
                                    "1-hop {} from {}",
                                    rel.kind.as_str(),
                                    hit.symbol_name.as_deref().unwrap_or(&rel.from_name)
                                ),
                                hit.rrf_score * 0.7,
                            ));
                        }
                    }
                } else if rel.to_symbol_id.as_deref() == Some(sid.as_str()) {
                    related.push(format!("called_by:{}", rel.from_name));
                    if idx < 5 {
                        if let Some(ref caller_id) = rel.from_symbol_id {
                            expansion_queue.push((
                                caller_id.clone(),
                                format!(
                                    "1-hop caller of {}",
                                    hit.symbol_name.as_deref().unwrap_or(&rel.to_name)
                                ),
                                hit.rrf_score * 0.65,
                            ));
                        }
                    }
                }
            }
        }
        related.sort();
        related.dedup();

        total_tokens_used += tok_count;
        primary_matches += 1;

        evidence.push(Evidence {
            repository_id: ws.repository_id.clone(),
            workspace_id: ws.workspace_id.clone(),
            snapshot_id: snap.snapshot_id.clone(),
            file_path: hit.file_path.clone(),
            source_range: hit.source_range,
            content_hash: hit.content_hash.clone(),
            producer: "forge-context-engine(primary)".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now.clone(),
        });

        snippets.push(ContextSnippet {
            file_path: hit.file_path.clone(),
            language: hit.language.clone(),
            symbol_id: hit.symbol_id.clone(),
            symbol_name: hit.symbol_name.clone(),
            symbol_kind: hit.symbol_kind,
            signature: hit.signature.clone(),
            source_range: hit.source_range,
            content: hit.content.clone(),
            content_hash: hit.content_hash.clone(),
            token_count: tok_count,
            inclusion_reason: format!("primary_search_match({})", hit.match_reasons.join("+")),
            score: hit.rrf_score,
            related_symbols: related,
        });
    }

    // Pass 2: Add 1-hop graph neighbors of top primary hits within remaining token budget
    for (neighbor_sym_id, reason, score) in expansion_queue {
        if seen_sym_ids.contains(&neighbor_sym_id) {
            continue;
        }
        let (Some(sym), Some(chunk)) = (
            sym_by_id.get(&neighbor_sym_id),
            chunk_by_sym_id.get(&neighbor_sym_id),
        ) else {
            continue;
        };
        if !include_tests
            && is_test_path_or_symbol(&chunk.file_path, chunk.symbol_name.as_deref())
        {
            continue;
        }
        let key = (
            chunk.file_path.clone(),
            chunk.source_range.start_line,
            chunk.source_range.end_line,
        );
        if seen_ranges.contains(&key) {
            continue;
        }
        let tok_count = estimate_tokens(&chunk.content);
        if total_tokens_used + tok_count > token_budget {
            budget_truncated = true;
            continue;
        }

        seen_sym_ids.insert(neighbor_sym_id);
        seen_ranges.insert(key);
        total_tokens_used += tok_count;
        graph_expanded_items += 1;

        evidence.push(Evidence {
            repository_id: ws.repository_id.clone(),
            workspace_id: ws.workspace_id.clone(),
            snapshot_id: snap.snapshot_id.clone(),
            file_path: chunk.file_path.clone(),
            source_range: chunk.source_range,
            content_hash: chunk.content_hash.clone(),
            producer: "forge-context-engine(graph-1hop)".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now.clone(),
        });

        snippets.push(ContextSnippet {
            file_path: chunk.file_path.clone(),
            language: chunk.language.clone(),
            symbol_id: Some(sym.symbol_id.clone()),
            symbol_name: Some(sym.qualified_name.clone()),
            symbol_kind: Some(sym.kind),
            signature: Some(sym.signature.clone()),
            source_range: chunk.source_range,
            content: chunk.content.clone(),
            content_hash: chunk.content_hash.clone(),
            token_count: tok_count,
            inclusion_reason: format!("graph_expansion({})", reason),
            score: (score * 10000.0).round() / 10000.0,
            related_symbols: Vec::new(),
        });
    }

    // Pass 3: Fill remaining token budget with additional primary search matches
    for hit in search_env.results.iter().skip(seed_count) {
        if let Some(ref sid) = hit.symbol_id {
            if seen_sym_ids.contains(sid) {
                continue;
            }
        }
        let key = (
            hit.file_path.clone(),
            hit.source_range.start_line,
            hit.source_range.end_line,
        );
        if seen_ranges.contains(&key) {
            continue;
        }
        let tok_count = estimate_tokens(&hit.content);
        if total_tokens_used + tok_count > token_budget {
            budget_truncated = true;
            continue;
        }

        seen_ranges.insert(key);
        if let Some(ref sid) = hit.symbol_id {
            seen_sym_ids.insert(sid.clone());
        }
        total_tokens_used += tok_count;
        primary_matches += 1;

        evidence.push(Evidence {
            repository_id: ws.repository_id.clone(),
            workspace_id: ws.workspace_id.clone(),
            snapshot_id: snap.snapshot_id.clone(),
            file_path: hit.file_path.clone(),
            source_range: hit.source_range,
            content_hash: hit.content_hash.clone(),
            producer: "forge-context-engine(primary)".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now.clone(),
        });

        snippets.push(ContextSnippet {
            file_path: hit.file_path.clone(),
            language: hit.language.clone(),
            symbol_id: hit.symbol_id.clone(),
            symbol_name: hit.symbol_name.clone(),
            symbol_kind: hit.symbol_kind,
            signature: hit.signature.clone(),
            source_range: hit.source_range,
            content: hit.content.clone(),
            content_hash: hit.content_hash.clone(),
            token_count: tok_count,
            inclusion_reason: format!("primary_search_match({})", hit.match_reasons.join("+")),
            score: hit.rrf_score,
            related_symbols: Vec::new(),
        });
    }

    let mut limitations = vec![
        "Context assembled from snapshot-consistent hybrid retrieval and 1-hop heuristic graph expansion."
            .to_string(),
    ];
    if freshness == Freshness::Stale {
        limitations.push(format!(
            "Snapshot {} is stale; changed files on disk: {}",
            snap.snapshot_id,
            changed_files.join(", ")
        ));
    }

    let pkg = ContextBuildResult {
        query: req.query.clone(),
        token_budget,
        total_tokens_used,
        primary_matches,
        graph_expanded_items,
        snippets,
    };

    Ok(ResponseEnvelope::new(
        snap.snapshot_id,
        freshness,
        vec![pkg],
        evidence,
        limitations,
        budget_truncated,
        None,
    ))
}

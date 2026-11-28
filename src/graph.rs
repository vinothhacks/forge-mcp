use crate::model::*;
use crate::search::is_test_path_or_symbol;
use crate::store::ForgeStore;
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet, VecDeque};

/// Execute bounded BFS/DFS relationship queries (`callers`, `callees`, `imports`, `importers`, `children`, `implements`, `path`).
pub fn graph_query(
    store: &ForgeStore,
    req: &GraphQueryRequest,
) -> Result<ResponseEnvelope<GraphQueryResult>> {
    let (ws, snap, freshness, changed_files) =
        store.check_workspace_freshness(&req.workspace_id)?;
    let eff_kind = req.effective_query_kind();
    let eff_sym_name = req.effective_symbol_name();
    let qkind = parse_query_kind(eff_kind)?;
    let max_depth = req.max_depth.unwrap_or(2).clamp(1, 8);
    let limit = req.limit.unwrap_or(100).clamp(1, 500);

    let symbols = store.list_snapshot_symbols(&snap.snapshot_id)?;
    let relationships = store.list_snapshot_relationships(&snap.snapshot_id)?;

    let sym_by_id: HashMap<String, Symbol> = symbols
        .iter()
        .cloned()
        .map(|s| (s.symbol_id.clone(), s))
        .collect();

    let root_symbol = if req.symbol_id.is_some() || eff_sym_name.is_some() {
        store
            .resolve_symbol_in_snapshot(
                &snap.snapshot_id,
                req.symbol_id.as_deref(),
                eff_sym_name,
                req.file_path.as_deref(),
            )
            .ok()
    } else {
        None
    };

    let root_name = root_symbol
        .as_ref()
        .map(|s| s.name.clone())
        .or_else(|| eff_sym_name.map(|s| s.to_string()))
        .or_else(|| req.file_path.clone())
        .ok_or_else(|| anyhow!("Provide symbol_id, symbol_name, or file_path for graph_query"))?;

    let mut hops = Vec::new();
    let mut path_found = None;

    match qkind {
        GraphQueryKind::Path => {
            let target_sym = store
                .resolve_symbol_in_snapshot(
                    &snap.snapshot_id,
                    req.target_symbol_id.as_deref(),
                    req.target_symbol_name.as_deref(),
                    None,
                )
                .ok();
            let target_name = target_sym
                .as_ref()
                .map(|s| s.name.clone())
                .or_else(|| req.target_symbol_name.clone())
                .ok_or_else(|| {
                    anyhow!("Provide target_symbol_id or target_symbol_name for path query")
                })?;

            let start_id = root_symbol.as_ref().map(|s| s.symbol_id.clone());
            let target_id = target_sym.as_ref().map(|s| s.symbol_id.clone());

            let found_edges = bfs_shortest_path(
                &relationships,
                start_id.as_deref(),
                &root_name,
                target_id.as_deref(),
                &target_name,
                max_depth,
            );
            path_found = Some(!found_edges.is_empty());
            for (idx, rel) in found_edges.into_iter().enumerate() {
                let from_sym = rel
                    .from_symbol_id
                    .as_ref()
                    .and_then(|id| sym_by_id.get(id).cloned());
                let to_sym = rel
                    .to_symbol_id
                    .as_ref()
                    .and_then(|id| sym_by_id.get(id).cloned());
                hops.push(GraphEdgeHop {
                    depth: idx + 1,
                    relationship: rel,
                    from_symbol: from_sym,
                    to_symbol: to_sym,
                });
            }
        }
        _ => {
            let mut visited_rels = HashSet::new();
            let mut frontier_ids: HashSet<String> = HashSet::new();
            let mut frontier_names: HashSet<String> = HashSet::new();

            if let Some(ref rs) = root_symbol {
                frontier_ids.insert(rs.symbol_id.clone());
                frontier_names.insert(rs.name.clone());
                frontier_names.insert(rs.qualified_name.clone());
            } else {
                frontier_names.insert(root_name.clone());
            }
            if let Some(ref fp) = req.file_path {
                frontier_names.insert(fp.replace('\\', "/"));
            }

            for depth in 1..=max_depth {
                let mut next_ids = HashSet::new();
                let mut next_names = HashSet::new();

                for rel in &relationships {
                    if visited_rels.contains(&rel.relationship_id) {
                        continue;
                    }
                    let matched_forward = match qkind {
                        GraphQueryKind::Callees => {
                            rel.kind == RelationshipKind::Calls
                                && matches_node(
                                    rel.from_symbol_id.as_deref(),
                                    &rel.from_name,
                                    &rel.from_file_path,
                                    &frontier_ids,
                                    &frontier_names,
                                )
                        }
                        GraphQueryKind::Callers => {
                            rel.kind == RelationshipKind::Calls
                                && matches_target(
                                    rel.to_symbol_id.as_deref(),
                                    &rel.to_name,
                                    &frontier_ids,
                                    &frontier_names,
                                )
                        }
                        GraphQueryKind::Imports => {
                            rel.kind == RelationshipKind::Imports
                                && matches_node(
                                    rel.from_symbol_id.as_deref(),
                                    &rel.from_name,
                                    &rel.from_file_path,
                                    &frontier_ids,
                                    &frontier_names,
                                )
                        }
                        GraphQueryKind::Importers => {
                            rel.kind == RelationshipKind::Imports
                                && matches_import_target(&rel.to_name, &frontier_names)
                        }
                        GraphQueryKind::Children => {
                            rel.kind == RelationshipKind::Contains
                                && matches_node(
                                    rel.from_symbol_id.as_deref(),
                                    &rel.from_name,
                                    &rel.from_file_path,
                                    &frontier_ids,
                                    &frontier_names,
                                )
                        }
                        GraphQueryKind::Implements => {
                            matches!(
                                rel.kind,
                                RelationshipKind::Implements | RelationshipKind::Inherits
                            ) && (matches_node(
                                rel.from_symbol_id.as_deref(),
                                &rel.from_name,
                                &rel.from_file_path,
                                &frontier_ids,
                                &frontier_names,
                            ) || matches_target(
                                rel.to_symbol_id.as_deref(),
                                &rel.to_name,
                                &frontier_ids,
                                &frontier_names,
                            ))
                        }
                        GraphQueryKind::Path => false,
                    };

                    if matched_forward {
                        visited_rels.insert(rel.relationship_id.clone());
                        let from_sym = rel
                            .from_symbol_id
                            .as_ref()
                            .and_then(|id| sym_by_id.get(id).cloned());
                        let to_sym = rel
                            .to_symbol_id
                            .as_ref()
                            .and_then(|id| sym_by_id.get(id).cloned());

                        match qkind {
                            GraphQueryKind::Callers | GraphQueryKind::Importers => {
                                if let Some(ref fid) = rel.from_symbol_id {
                                    next_ids.insert(fid.clone());
                                }
                                next_names.insert(rel.from_name.clone());
                            }
                            _ => {
                                if let Some(ref tid) = rel.to_symbol_id {
                                    next_ids.insert(tid.clone());
                                }
                                next_names.insert(rel.to_name.clone());
                            }
                        }

                        hops.push(GraphEdgeHop {
                            depth,
                            relationship: rel.clone(),
                            from_symbol: from_sym,
                            to_symbol: to_sym,
                        });
                    }
                }

                if next_ids.is_empty() && next_names.is_empty() {
                    break;
                }
                frontier_ids = next_ids;
                frontier_names = next_names;
            }
        }
    }

    let truncated = hops.len() > limit;
    hops.truncate(limit);

    let now = timestamp_now();
    let mut evidence = Vec::new();
    let mut unresolved_count = 0usize;

    for hop in &hops {
        // A hop is unresolved when the extractor flagged it, or when its target
        // symbol is not present in this snapshot (external module/crate, stdlib, ...).
        if hop.relationship.resolution_kind == ResolutionKind::Unresolved || hop.to_symbol.is_none()
        {
            unresolved_count += 1;
        }
        evidence.push(Evidence {
            repository_id: ws.repository_id.clone(),
            workspace_id: ws.workspace_id.clone(),
            snapshot_id: snap.snapshot_id.clone(),
            file_path: hop.relationship.from_file_path.clone(),
            source_range: hop.relationship.source_range,
            content_hash: hop.relationship.content_hash.clone(),
            producer: "forge-graph-engine".to_string(),
            resolution_kind: hop.relationship.resolution_kind,
            observed_at: now.clone(),
        });
    }

    let mut limitations = vec![
        "Graph edges are extracted via Tree-sitter AST heuristics; dynamic dispatch is classified as heuristic or unresolved."
            .to_string(),
    ];
    if unresolved_count > 0 {
        limitations.push(format!(
            "{} relationship(s) target external or unresolved symbols (target not present in snapshot).",
            unresolved_count
        ));
    }
    if freshness == Freshness::Stale {
        limitations.push(format!(
            "Snapshot {} is stale; modified files: {}",
            snap.snapshot_id,
            changed_files.join(", ")
        ));
    }

    let query_res = GraphQueryResult {
        query_kind: eff_kind.to_string(),
        root_symbol,
        root_name,
        max_depth,
        hops,
        path_found,
    };

    Ok(ResponseEnvelope::new(
        snap.snapshot_id,
        freshness,
        vec![query_res],
        evidence,
        limitations,
        truncated,
        None,
    ))
}

/// Strict variant of `GraphQueryKind::from_str_lossy`: unknown kinds are an error
/// instead of silently falling back to `callers`.
fn parse_query_kind(s: &str) -> Result<GraphQueryKind> {
    const KNOWN: &[&str] = &[
        "callers",
        "caller",
        "callees",
        "callee",
        "calls",
        "imports",
        "import",
        "importers",
        "importer",
        "imported_by",
        "children",
        "contains",
        "child",
        "implements",
        "inheritance",
        "inherits",
        "path",
        "shortest_path",
    ];
    let norm = s.trim().to_ascii_lowercase();
    if KNOWN.contains(&norm.as_str()) {
        Ok(GraphQueryKind::from_str_lossy(&norm))
    } else {
        Err(anyhow!(
            "unknown query_kind '{}'; expected one of callers, callees, imports, importers, children, implements, path",
            s
        ))
    }
}

fn matches_node(
    sym_id: Option<&str>,
    name: &str,
    file_path: &str,
    frontier_ids: &HashSet<String>,
    frontier_names: &HashSet<String>,
) -> bool {
    if let Some(id) = sym_id {
        if frontier_ids.contains(id) {
            return true;
        }
    }
    if frontier_names.contains(name) || frontier_names.contains(file_path) {
        return true;
    }
    let short = name.rsplit([':', '.', '/']).next().unwrap_or(name);
    frontier_names.contains(short)
}

fn matches_target(
    to_sym_id: Option<&str>,
    to_name: &str,
    frontier_ids: &HashSet<String>,
    frontier_names: &HashSet<String>,
) -> bool {
    if let Some(id) = to_sym_id {
        if frontier_ids.contains(id) {
            return true;
        }
    }
    if frontier_names.contains(to_name) {
        return true;
    }
    let short = to_name.rsplit([':', '.', '/']).next().unwrap_or(to_name);
    frontier_names.contains(short)
}

fn matches_import_target(to_name: &str, frontier_names: &HashSet<String>) -> bool {
    if frontier_names.contains(to_name) {
        return true;
    }
    let modules = import_module_candidates(to_name);
    for f in frontier_names {
        let stem = strip_source_ext(f.rsplit('/').next().unwrap_or(f));
        if !stem.is_empty() && modules.iter().any(|m| m == stem) {
            return true;
        }
    }
    false
}

const SOURCE_EXTS: &[&str] = &[".rs", ".tsx", ".ts", ".mjs", ".jsx", ".js", ".py", ".go"];

fn strip_source_ext(s: &str) -> &str {
    for ext in SOURCE_EXTS {
        if let Some(stripped) = s.strip_suffix(ext) {
            return stripped;
        }
    }
    s
}

/// Module names an import specifier may refer to, compared exactly against a file stem.
/// - Rust paths (`crate::auth::verify`, `super::auth`): every segment after dropping
///   `crate`/`self`/`super` (the module may be followed by imported item names).
/// - Path-like specifiers (`./auth_client`, `../lib/auth.js`, `"fmt"`, `pkg.auth`):
///   only the last segment, after stripping quotes, `./`/`../` prefixes and a source
///   extension; dotted Python modules use their last dotted component.
///
/// Substring matching is deliberately avoided: `auth` must not match `auth_client`.
fn import_module_candidates(to_name: &str) -> Vec<String> {
    let spec = to_name
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
        .replace('\\', "/");
    if spec.contains("::") {
        return spec
            .split("::")
            .map(|seg| seg.trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace()))
            .filter(|seg| !seg.is_empty() && !matches!(*seg, "crate" | "self" | "super" | "*"))
            .map(|seg| seg.to_string())
            .collect();
    }
    let mut rest = spec.as_str();
    loop {
        if let Some(r) = rest.strip_prefix("./") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix("../") {
            rest = r;
        } else {
            break;
        }
    }
    let last = rest.rsplit('/').next().unwrap_or(rest);
    let last = strip_source_ext(last);
    let last = last.rsplit('.').next().unwrap_or(last);
    if last.is_empty() {
        Vec::new()
    } else {
        vec![last.to_string()]
    }
}

fn bfs_shortest_path(
    relationships: &[Relationship],
    start_id: Option<&str>,
    start_name: &str,
    target_id: Option<&str>,
    target_name: &str,
    max_depth: usize,
) -> Vec<Relationship> {
    let mut queue: VecDeque<(Option<String>, String, Vec<Relationship>)> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();

    queue.push_back((
        start_id.map(|s| s.to_string()),
        start_name.to_string(),
        Vec::new(),
    ));
    visited.insert(start_name.to_string());
    if let Some(sid) = start_id {
        visited.insert(sid.to_string());
    }

    while let Some((curr_id, curr_name, path)) = queue.pop_front() {
        if path.len() >= max_depth {
            continue;
        }

        let mut f_ids = HashSet::new();
        if let Some(ref cid) = curr_id {
            f_ids.insert(cid.clone());
        }
        let mut f_names = HashSet::new();
        f_names.insert(curr_name.clone());

        for rel in relationships {
            if matches_node(
                rel.from_symbol_id.as_deref(),
                &rel.from_name,
                &rel.from_file_path,
                &f_ids,
                &f_names,
            ) {
                let mut next_path = path.clone();
                next_path.push(rel.clone());

                let reached = (target_id.is_some() && rel.to_symbol_id.as_deref() == target_id)
                    || rel.to_name == target_name
                    || rel
                        .to_name
                        .rsplit([':', '.'])
                        .next()
                        .unwrap_or(&rel.to_name)
                        == target_name;
                if reached {
                    return next_path;
                }

                let key = rel
                    .to_symbol_id
                    .clone()
                    .unwrap_or_else(|| rel.to_name.clone());
                if !visited.contains(&key) {
                    visited.insert(key);
                    queue.push_back((rel.to_symbol_id.clone(), rel.to_name.clone(), next_path));
                }
            }
        }
    }

    Vec::new()
}

/// Analyze transitive change impact (blast radius) of modifying a symbol or file.
pub fn impact_analyze(
    store: &ForgeStore,
    req: &ImpactAnalyzeRequest,
) -> Result<ResponseEnvelope<ImpactAnalyzeResult>> {
    let (ws, snap, freshness, changed_files) =
        store.check_workspace_freshness(&req.workspace_id)?;
    let max_depth = req.max_depth.unwrap_or(3).clamp(1, 8);

    let symbols = store.list_snapshot_symbols(&snap.snapshot_id)?;
    let relationships = store.list_snapshot_relationships(&snap.snapshot_id)?;

    let eff_sym_name = req.effective_symbol_name();
    let target_symbol = if eff_sym_name.is_some() || req.symbol_id.is_some() {
        // When an explicit symbol name is provided alongside a stale symbol_id, prefer matching by name if it differs
        if let Some(sn) = eff_sym_name {
            store
                .resolve_symbol_in_snapshot(
                    &snap.snapshot_id,
                    None,
                    Some(sn),
                    req.file_path.as_deref(),
                )
                .or_else(|_| {
                    store.resolve_symbol_in_snapshot(
                        &snap.snapshot_id,
                        req.symbol_id.as_deref(),
                        Some(sn),
                        req.file_path.as_deref(),
                    )
                })
                .ok()
        } else {
            store
                .resolve_symbol_in_snapshot(
                    &snap.snapshot_id,
                    req.symbol_id.as_deref(),
                    None,
                    req.file_path.as_deref(),
                )
                .ok()
        }
    } else {
        None
    };

    let target_file_path = req
        .file_path
        .as_ref()
        .map(|p| p.replace('\\', "/"))
        .or_else(|| target_symbol.as_ref().map(|s| s.file_path.clone()));

    let mut frontier_ids = HashSet::new();
    let mut frontier_names = HashSet::new();

    if let Some(ref sym) = target_symbol {
        frontier_ids.insert(sym.symbol_id.clone());
        frontier_names.insert(sym.name.clone());
        frontier_names.insert(sym.qualified_name.clone());
    } else if let Some(sn) = eff_sym_name {
        frontier_names.insert(sn.to_string());
    } else if let Some(ref fp) = target_file_path {
        frontier_names.insert(fp.clone());
        for sym in symbols.iter().filter(|s| &s.file_path == fp) {
            frontier_ids.insert(sym.symbol_id.clone());
            frontier_names.insert(sym.name.clone());
        }
    } else if let Some(sid) = req.symbol_id.as_deref() {
        return Err(anyhow!(
            "symbol_id {:?} not found in snapshot {} for impact_analyze; pass a current symbol_id (from symbol_find) or a symbol_name/file_path",
            sid,
            snap.snapshot_id
        ));
    } else {
        return Err(anyhow!(
            "Provide symbol_id, symbol_name, or file_path for impact_analyze"
        ));
    }

    let mut direct_dependents = Vec::new();
    let mut transitive_dependents = Vec::new();
    let mut affected_files_set = HashSet::new();
    let mut impacted_test_files_set = HashSet::new();
    let mut visited_keys = frontier_names.clone();
    visited_keys.extend(frontier_ids.iter().cloned());

    let mut max_depth_reached = 0usize;
    let mut evidence = Vec::new();
    let now = timestamp_now();

    for distance in 1..=max_depth {
        let mut next_ids = HashSet::new();
        let mut next_names = HashSet::new();

        for rel in &relationships {
            let is_dep = match rel.kind {
                RelationshipKind::Calls
                | RelationshipKind::Implements
                | RelationshipKind::Inherits
                | RelationshipKind::References => matches_target(
                    rel.to_symbol_id.as_deref(),
                    &rel.to_name,
                    &frontier_ids,
                    &frontier_names,
                ),
                RelationshipKind::Imports => matches_import_target(&rel.to_name, &frontier_names),
                RelationshipKind::Contains => false,
            };

            if !is_dep {
                continue;
            }

            let dep_key = rel
                .from_symbol_id
                .clone()
                .unwrap_or_else(|| format!("{}:{}", rel.from_file_path, rel.from_name));
            if visited_keys.contains(&dep_key) {
                continue;
            }
            visited_keys.insert(dep_key);

            if let Some(ref fid) = rel.from_symbol_id {
                next_ids.insert(fid.clone());
            }
            next_names.insert(rel.from_name.clone());

            let is_test = is_test_path_or_symbol(&rel.from_file_path, Some(&rel.from_name));
            affected_files_set.insert(rel.from_file_path.clone());
            if is_test {
                impacted_test_files_set.insert(rel.from_file_path.clone());
            }

            max_depth_reached = max_depth_reached.max(distance);

            evidence.push(Evidence {
                repository_id: ws.repository_id.clone(),
                workspace_id: ws.workspace_id.clone(),
                snapshot_id: snap.snapshot_id.clone(),
                file_path: rel.from_file_path.clone(),
                source_range: rel.source_range,
                content_hash: rel.content_hash.clone(),
                producer: "forge-impact-analyzer".to_string(),
                resolution_kind: rel.resolution_kind,
                observed_at: now.clone(),
            });

            let dep = ImpactDependent {
                symbol_id: rel.from_symbol_id.clone(),
                symbol_name: rel.from_name.clone(),
                file_path: rel.from_file_path.clone(),
                distance,
                relationship_kind: rel.kind,
                resolution_kind: rel.resolution_kind,
                source_range: rel.source_range,
                is_test,
            };

            if distance == 1 {
                direct_dependents.push(dep);
            } else {
                transitive_dependents.push(dep);
            }
        }

        if next_ids.is_empty() && next_names.is_empty() {
            break;
        }
        frontier_ids = next_ids;
        frontier_names = next_names;
    }

    let mut affected_files: Vec<String> = affected_files_set.into_iter().collect();
    affected_files.sort();
    let mut impacted_test_files: Vec<String> = impacted_test_files_set.into_iter().collect();
    impacted_test_files.sort();

    let mut limitations = vec![
        "Impact analysis uses syntactic Tree-sitter call/import/inheritance graphs (heuristic resolution)."
            .to_string(),
    ];
    if freshness == Freshness::Stale {
        limitations.push(format!(
            "Snapshot {} is stale; modified files: {}",
            snap.snapshot_id,
            changed_files.join(", ")
        ));
    }

    let res = ImpactAnalyzeResult {
        target_symbol,
        target_file_path,
        direct_dependents,
        transitive_dependents,
        affected_files,
        impacted_test_files,
        max_depth_reached,
    };

    Ok(ResponseEnvelope::new(
        snap.snapshot_id,
        freshness,
        vec![res],
        evidence,
        limitations,
        false,
        None,
    ))
}

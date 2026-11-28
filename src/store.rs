use crate::embeddings::EmbeddingEngine;
use crate::model::*;
use crate::parser::{detect_language, parse_file_ast, ParsedFile};
use anyhow::{anyhow, Context, Result};
use ignore::WalkBuilder;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// One indexed file: (relative path, absolute path, language, content, content hash).
pub type WorkspaceFileRecord = (String, PathBuf, String, String, String);

/// Result of resolving a symbol by id/name: the chosen symbol, other
/// candidates that also matched, and human-readable notes about fallbacks.
#[derive(Debug, Clone)]
pub struct ResolvedSymbol {
    pub symbol: Symbol,
    pub alternatives: Vec<Symbol>,
    pub notes: Vec<String>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[derive(Clone)]
pub struct ForgeStore {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
    cancelled_jobs: Arc<Mutex<HashSet<String>>>,
}

impl ForgeStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open SQLite database at {}", db_path.display()))?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
            cancelled_jobs: Arc::new(Mutex::new(HashSet::new())),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: PathBuf::from(":memory:"),
            cancelled_jobs: Arc::new(Mutex::new(HashSet::new())),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn with_conn<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> Result<R>,
    {
        let guard = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Database mutex poisoned: {}", e))?;
        f(&guard)
    }

    pub fn with_conn_mut<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R>,
    {
        let mut guard = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Database mutex poisoned: {}", e))?;
        f(&mut guard)
    }

    fn init_schema(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute_batch(
                r#"
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = NORMAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS repositories (
                    repository_id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    root_path TEXT NOT NULL UNIQUE,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS workspaces (
                    workspace_id TEXT PRIMARY KEY,
                    repository_id TEXT NOT NULL,
                    root_path TEXT NOT NULL,
                    active_snapshot_id TEXT,
                    index_generation INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS snapshots (
                    snapshot_id TEXT PRIMARY KEY,
                    repository_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    index_generation INTEGER NOT NULL,
                    root_hash TEXT NOT NULL,
                    file_count INTEGER NOT NULL,
                    symbol_count INTEGER NOT NULL,
                    reference_count INTEGER NOT NULL,
                    relationship_count INTEGER NOT NULL,
                    chunk_count INTEGER NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS file_versions (
                    file_id TEXT PRIMARY KEY,
                    snapshot_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    file_path TEXT NOT NULL,
                    language TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    line_count INTEGER NOT NULL,
                    content TEXT NOT NULL,
                    indexed_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_file_versions_snapshot ON file_versions(snapshot_id, file_path);

                CREATE TABLE IF NOT EXISTS symbols (
                    symbol_id TEXT PRIMARY KEY,
                    snapshot_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    repository_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    qualified_name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    language TEXT NOT NULL,
                    file_path TEXT NOT NULL,
                    start_line INTEGER NOT NULL,
                    start_column INTEGER NOT NULL,
                    end_line INTEGER NOT NULL,
                    end_column INTEGER NOT NULL,
                    start_byte INTEGER NOT NULL,
                    end_byte INTEGER NOT NULL,
                    signature TEXT NOT NULL,
                    doc_comment TEXT,
                    parent_symbol_id TEXT,
                    visibility TEXT,
                    content_hash TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_symbols_snapshot_name ON symbols(snapshot_id, name);
                CREATE INDEX IF NOT EXISTS idx_symbols_snapshot_file ON symbols(snapshot_id, file_path);
                CREATE INDEX IF NOT EXISTS idx_symbols_snapshot_kind ON symbols(snapshot_id, kind);

                CREATE TABLE IF NOT EXISTS "references" (
                    reference_id TEXT PRIMARY KEY,
                    snapshot_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    symbol_name TEXT NOT NULL,
                    target_symbol_id TEXT,
                    file_path TEXT NOT NULL,
                    start_line INTEGER NOT NULL,
                    start_column INTEGER NOT NULL,
                    end_line INTEGER NOT NULL,
                    end_column INTEGER NOT NULL,
                    start_byte INTEGER NOT NULL,
                    end_byte INTEGER NOT NULL,
                    context_line TEXT NOT NULL,
                    reference_kind TEXT NOT NULL,
                    enclosing_symbol_id TEXT,
                    resolution_kind TEXT NOT NULL,
                    content_hash TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_references_snapshot_name ON "references"(snapshot_id, symbol_name);
                CREATE INDEX IF NOT EXISTS idx_references_snapshot_target ON "references"(snapshot_id, target_symbol_id);

                CREATE TABLE IF NOT EXISTS relationships (
                    relationship_id TEXT PRIMARY KEY,
                    snapshot_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    from_symbol_id TEXT,
                    from_name TEXT NOT NULL,
                    from_file_path TEXT NOT NULL,
                    to_symbol_id TEXT,
                    to_name TEXT NOT NULL,
                    to_file_path TEXT,
                    kind TEXT NOT NULL,
                    start_line INTEGER NOT NULL,
                    start_column INTEGER NOT NULL,
                    end_line INTEGER NOT NULL,
                    end_column INTEGER NOT NULL,
                    start_byte INTEGER NOT NULL,
                    end_byte INTEGER NOT NULL,
                    resolution_kind TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    indexing_config TEXT NOT NULL,
                    limitations_json TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_relationships_from ON relationships(snapshot_id, from_symbol_id, kind);
                CREATE INDEX IF NOT EXISTS idx_relationships_to ON relationships(snapshot_id, to_symbol_id, kind);
                CREATE INDEX IF NOT EXISTS idx_relationships_names ON relationships(snapshot_id, from_name, to_name, kind);

                CREATE TABLE IF NOT EXISTS chunks (
                    chunk_id TEXT PRIMARY KEY,
                    snapshot_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    file_path TEXT NOT NULL,
                    language TEXT NOT NULL,
                    symbol_id TEXT,
                    symbol_name TEXT,
                    start_line INTEGER NOT NULL,
                    start_column INTEGER NOT NULL,
                    end_line INTEGER NOT NULL,
                    end_column INTEGER NOT NULL,
                    start_byte INTEGER NOT NULL,
                    end_byte INTEGER NOT NULL,
                    content TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    token_estimate INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_chunks_snapshot_file ON chunks(snapshot_id, file_path);

                CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                    chunk_id UNINDEXED,
                    snapshot_id UNINDEXED,
                    workspace_id UNINDEXED,
                    file_path,
                    symbol_name,
                    content,
                    tokenize = 'unicode61 tokenchars ''_'''
                );

                CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
                    symbol_id UNINDEXED,
                    snapshot_id UNINDEXED,
                    workspace_id UNINDEXED,
                    name,
                    qualified_name,
                    signature,
                    doc_comment,
                    file_path,
                    tokenize = 'unicode61 tokenchars ''_'''
                );

                CREATE TABLE IF NOT EXISTS vector_embeddings (
                    chunk_id TEXT PRIMARY KEY,
                    snapshot_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    model TEXT NOT NULL,
                    dimensions INTEGER NOT NULL,
                    embedding_json TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_vectors_snapshot ON vector_embeddings(snapshot_id);

                CREATE TABLE IF NOT EXISTS jobs (
                    job_id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    progress_pct INTEGER NOT NULL,
                    processed_files INTEGER NOT NULL,
                    total_files INTEGER NOT NULL,
                    snapshot_id TEXT,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                "#,
            )?;
            Ok(())
        })
    }

    /// Discover supported source files in `root_path` respecting `.gitignore` and excluding internal build/index dirs.
    pub fn collect_workspace_files(root_path: &Path) -> Result<Vec<(String, PathBuf, String)>> {
        let mut files = Vec::new();
        let mut builder = WalkBuilder::new(root_path);
        builder
            .hidden(false)
            // `ignore` defaults to `require_git(true)`, which silently disables `.gitignore`
            // handling when the workspace root is not a git repository. Forge must honor
            // `.gitignore` regardless of VCS presence.
            .require_git(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    !matches!(
                        name.as_ref(),
                        ".git"
                            | "node_modules"
                            | "target"
                            | ".forge"
                            | ".cursor"
                            | "__pycache__"
                            | ".venv"
                            | "venv"
                            | "dist"
                            | "build"
                    )
                } else {
                    true
                }
            });

        for result in builder.build() {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }
            let abs_path = entry.path().to_path_buf();
            if let Some(lang) = detect_language(&abs_path) {
                if let Ok(rel) = abs_path.strip_prefix(root_path) {
                    let rel_slash = to_slash_path(rel);
                    if !rel_slash.is_empty() {
                        files.push((rel_slash, abs_path, lang.to_string()));
                    }
                }
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(files)
    }

    /// Compute SHA-256 hashes for all workspace source files and an aggregate root hash.
    pub fn compute_workspace_state(root_path: &Path) -> Result<(String, Vec<WorkspaceFileRecord>)> {
        const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

        let discovered = Self::collect_workspace_files(root_path)?;
        let mut file_records = Vec::with_capacity(discovered.len());
        let mut root_hasher = Sha256::new();

        for (rel_path, abs_path, lang) in discovered {
            let Ok(file) = File::open(&abs_path) else {
                continue;
            };
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
                continue;
            }
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            // Bound the read even if the source grows after the metadata check.
            if file
                .take(MAX_SOURCE_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() as u64 > MAX_SOURCE_BYTES
                || bytes.contains(&0)
            {
                continue;
            }
            let content = String::from_utf8(bytes)
                .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned());
            let hash = sha256_hex(content.as_bytes());
            root_hasher.update(rel_path.as_bytes());
            root_hasher.update(b":");
            root_hasher.update(hash.as_bytes());
            root_hasher.update(b"\n");
            file_records.push((rel_path, abs_path, lang, content, hash));
        }

        let root_hash = hex::encode(root_hasher.finalize());
        Ok((root_hash, file_records))
    }

    /// Open and index a workspace, returning the snapshot envelope.
    pub fn open_workspace(
        &self,
        req: &WorkspaceOpenRequest,
        embedder: &EmbeddingEngine,
        allowed_roots: &[PathBuf],
    ) -> Result<ResponseEnvelope<WorkspaceOpenResult>> {
        let raw_path = Path::new(&req.path);
        if !raw_path.exists() {
            return Err(anyhow!("Workspace path does not exist: {}", req.path));
        }
        let canon_root = normalize_canonical_path(raw_path)
            .with_context(|| format!("Failed to canonicalize workspace path: {}", req.path))?;
        if !canon_root.is_dir() {
            return Err(anyhow!("Workspace path is not a directory: {}", req.path));
        }

        if !allowed_roots.is_empty() {
            let allowed = allowed_roots.iter().any(|ar| {
                normalize_canonical_path(ar).is_ok_and(|canon_ar| canon_root.starts_with(canon_ar))
            });
            if !allowed {
                return Err(anyhow!(
                    "Workspace path '{}' is outside allowed roots",
                    canon_root.display()
                ));
            }
        }

        let root_str = canon_root.to_string_lossy().into_owned();
        let root_path_hash = sha256_hex(root_str.as_bytes());
        let repo_name = canon_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repository".to_string());

        let workspace_id = req.workspace_id.clone().unwrap_or_else(|| {
            let slug: String = repo_name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            let short_hash = &root_path_hash[..8];
            format!("{}-{}", slug.trim_matches('-'), short_hash)
        });

        let repository_id = format!("repo-{}", &root_path_hash[..12]);
        let now = timestamp_now();

        let (root_hash, file_records) = Self::compute_workspace_state(&canon_root)?;

        // Check if current snapshot matches root_hash and force_reindex is false
        let force_reindex = req.force_reindex.unwrap_or(false);
        if !force_reindex {
            if let Some(existing_snap) = self.get_active_snapshot(&workspace_id)? {
                if existing_snap.root_hash == root_hash {
                    let job_id = format!("job-{}", &Uuid::new_v4().to_string()[..8]);
                    self.insert_job(&JobRecord {
                        job_id: job_id.clone(),
                        workspace_id: workspace_id.clone(),
                        kind: "workspace_index".to_string(),
                        status: JobStatusState::Completed,
                        progress_pct: 100,
                        processed_files: existing_snap.file_count,
                        total_files: existing_snap.file_count,
                        snapshot_id: Some(existing_snap.snapshot_id.clone()),
                        error: None,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    })?;

                    let open_res = WorkspaceOpenResult {
                        workspace_id: workspace_id.clone(),
                        repository_id: repository_id.clone(),
                        snapshot_id: existing_snap.snapshot_id.clone(),
                        root_path: root_str.clone(),
                        index_generation: existing_snap.index_generation,
                        file_count: existing_snap.file_count,
                        symbol_count: existing_snap.symbol_count,
                        reference_count: existing_snap.reference_count,
                        relationship_count: existing_snap.relationship_count,
                        chunk_count: existing_snap.chunk_count,
                        job_id,
                        reused_snapshot: true,
                    };
                    let evidence = vec![Evidence {
                        repository_id,
                        workspace_id,
                        snapshot_id: existing_snap.snapshot_id.clone(),
                        file_path: ".".to_string(),
                        source_range: SourceRange::default(),
                        content_hash: root_hash,
                        producer: "forge-snapshot-coordinator".to_string(),
                        resolution_kind: ResolutionKind::Heuristic,
                        observed_at: now,
                    }];
                    return Ok(ResponseEnvelope::new(
                        existing_snap.snapshot_id,
                        Freshness::Current,
                        vec![open_res],
                        evidence,
                        vec!["Tree-sitter AST extraction provides syntactic and heuristic call/import resolution.".to_string()],
                        false,
                        None,
                    ));
                }
            }
        }

        // Create job record
        let job_id = format!("job-{}", &Uuid::new_v4().to_string()[..8]);
        let total_files = file_records.len();
        self.insert_job(&JobRecord {
            job_id: job_id.clone(),
            workspace_id: workspace_id.clone(),
            kind: "workspace_index".to_string(),
            status: JobStatusState::Running,
            progress_pct: 0,
            processed_files: 0,
            total_files,
            snapshot_id: None,
            error: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        })?;

        let prev_gen = self.get_workspace_generation(&workspace_id)?.unwrap_or(0);
        let next_gen = prev_gen + 1;
        let snapshot_id = format!("snap-{}-{}", next_gen, &root_hash[..8]);

        // Parse all files via Tree-sitter
        let mut parsed_files: Vec<(FileVersion, String, ParsedFile)> =
            Vec::with_capacity(file_records.len());

        for (idx, (rel_path, _abs_path, lang, content, content_hash)) in
            file_records.into_iter().enumerate()
        {
            if self.is_job_cancelled(&job_id) {
                self.update_job_status(
                    &job_id,
                    JobStatusState::Cancelled,
                    ((idx * 100) / total_files.max(1)) as u8,
                    idx,
                    None,
                    Some("Job cancelled by user".to_string()),
                )?;
                return Err(anyhow!("Indexing job {} was cancelled", job_id));
            }

            let parsed = parse_file_ast(
                &snapshot_id,
                &workspace_id,
                &repository_id,
                &rel_path,
                &lang,
                &content,
                &content_hash,
            )?;

            let line_count = content.lines().count().max(1);
            let fv = FileVersion {
                file_id: format!(
                    "file-{}-{}",
                    snapshot_id,
                    &sha256_hex(rel_path.as_bytes())[..10]
                ),
                snapshot_id: snapshot_id.clone(),
                workspace_id: workspace_id.clone(),
                file_path: rel_path,
                language: lang,
                content_hash,
                size_bytes: content.len(),
                line_count,
                indexed_at: now.clone(),
            };
            parsed_files.push((fv, content, parsed));
        }

        // Build global symbol lookup map by name to resolve cross-file references and edges
        let mut symbols_by_name: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (_fv, _content, pf) in &parsed_files {
            for sym in &pf.symbols {
                symbols_by_name
                    .entry(sym.name.clone())
                    .or_default()
                    .push((sym.symbol_id.clone(), sym.file_path.clone()));
            }
        }

        let mut all_symbols = Vec::new();
        let mut all_references = Vec::new();
        let mut all_relationships = Vec::new();
        let mut all_chunks = Vec::new();

        for (_fv, _content, pf) in parsed_files.iter_mut() {
            all_symbols.extend(pf.symbols.clone());

            for r in &mut pf.references {
                if let Some(candidates) = symbols_by_name.get(&r.symbol_name) {
                    // Prefer same-file symbol, otherwise first workspace match
                    let chosen = candidates
                        .iter()
                        .find(|(_, fp)| fp == &r.file_path)
                        .or_else(|| candidates.first());
                    if let Some((sym_id, _)) = chosen {
                        r.target_symbol_id = Some(sym_id.clone());
                        r.resolution_kind = ResolutionKind::Heuristic;
                    }
                } else {
                    r.resolution_kind = ResolutionKind::Unresolved;
                }
                all_references.push(r.clone());
            }

            for rel in &mut pf.relationships {
                if rel.to_symbol_id.is_none() {
                    let lookup_name = rel
                        .to_name
                        .rsplit([':', '.', '/'])
                        .next()
                        .unwrap_or(&rel.to_name);
                    if let Some(candidates) = symbols_by_name.get(lookup_name) {
                        let chosen = candidates
                            .iter()
                            .find(|(_, fp)| fp == &rel.from_file_path)
                            .or_else(|| candidates.first());
                        if let Some((sym_id, target_fp)) = chosen {
                            rel.to_symbol_id = Some(sym_id.clone());
                            rel.to_file_path = Some(target_fp.clone());
                            rel.resolution_kind = ResolutionKind::Heuristic;
                        }
                    } else if rel.kind == RelationshipKind::Imports {
                        // Try matching module/file path for imports
                        rel.resolution_kind = ResolutionKind::Heuristic;
                    } else {
                        rel.resolution_kind = ResolutionKind::Unresolved;
                    }
                }
                all_relationships.push(rel.clone());
            }

            all_chunks.extend(pf.chunks.clone());
        }

        // Compute embeddings for all chunks
        let mut chunk_embeddings: Vec<(String, String, usize, String)> =
            Vec::with_capacity(all_chunks.len());
        for chunk in &all_chunks {
            if self.is_job_cancelled(&job_id) {
                return Err(self.abort_cancelled_job(&job_id, total_files));
            }
            let text_for_embed = if let Some(sym_name) = &chunk.symbol_name {
                format!("{} {}\n{}", chunk.file_path, sym_name, chunk.content)
            } else {
                format!("{}\n{}", chunk.file_path, chunk.content)
            };
            let vec = embedder.embed_text(&text_for_embed);
            let dim = vec.len();
            let json = serde_json::to_string(&vec)?;
            chunk_embeddings.push((
                chunk.chunk_id.clone(),
                embedder.model_name().to_string(),
                dim,
                json,
            ));
        }

        let snapshot = Snapshot {
            snapshot_id: snapshot_id.clone(),
            repository_id: repository_id.clone(),
            workspace_id: workspace_id.clone(),
            index_generation: next_gen,
            root_hash: root_hash.clone(),
            file_count: parsed_files.len(),
            symbol_count: all_symbols.len(),
            reference_count: all_references.len(),
            relationship_count: all_relationships.len(),
            chunk_count: all_chunks.len(),
            created_at: now.clone(),
        };

        // Write everything in a single SQLite transaction
        let committed = self.with_conn_mut(|conn| {
            let tx = conn.transaction()?;

            tx.execute(
                r#"
                INSERT INTO repositories (repository_id, name, root_path, created_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(root_path) DO UPDATE SET name = excluded.name
                "#,
                params![repository_id, repo_name, root_str, now],
            )?;

            tx.execute(
                r#"
                INSERT INTO workspaces (workspace_id, repository_id, root_path, active_snapshot_id, index_generation, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    repository_id = excluded.repository_id,
                    root_path = excluded.root_path,
                    active_snapshot_id = excluded.active_snapshot_id,
                    index_generation = excluded.index_generation,
                    updated_at = excluded.updated_at
                "#,
                params![
                    workspace_id,
                    repository_id,
                    root_str,
                    snapshot.snapshot_id,
                    next_gen,
                    now,
                    now
                ],
            )?;

            tx.execute(
                r#"
                INSERT OR REPLACE INTO snapshots (
                    snapshot_id, repository_id, workspace_id, index_generation, root_hash,
                    file_count, symbol_count, reference_count, relationship_count, chunk_count, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                params![
                    snapshot.snapshot_id,
                    snapshot.repository_id,
                    snapshot.workspace_id,
                    snapshot.index_generation,
                    snapshot.root_hash,
                    snapshot.file_count as i64,
                    snapshot.symbol_count as i64,
                    snapshot.reference_count as i64,
                    snapshot.relationship_count as i64,
                    snapshot.chunk_count as i64,
                    snapshot.created_at
                ],
            )?;

            for (fv, content, _) in &parsed_files {
                tx.execute(
                    r#"
                    INSERT OR REPLACE INTO file_versions (
                        file_id, snapshot_id, workspace_id, file_path, language,
                        content_hash, size_bytes, line_count, content, indexed_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                    "#,
                    params![
                        fv.file_id,
                        fv.snapshot_id,
                        fv.workspace_id,
                        fv.file_path,
                        fv.language,
                        fv.content_hash,
                        fv.size_bytes as i64,
                        fv.line_count as i64,
                        content,
                        fv.indexed_at
                    ],
                )?;
            }

            for sym in &all_symbols {
                tx.execute(
                    r#"
                    INSERT OR REPLACE INTO symbols (
                        symbol_id, snapshot_id, workspace_id, repository_id, name, qualified_name,
                        kind, language, file_path, start_line, start_column, end_line, end_column,
                        start_byte, end_byte, signature, doc_comment, parent_symbol_id, visibility, content_hash
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                    "#,
                    params![
                        sym.symbol_id,
                        sym.snapshot_id,
                        sym.workspace_id,
                        sym.repository_id,
                        sym.name,
                        sym.qualified_name,
                        sym.kind.as_str(),
                        sym.language,
                        sym.file_path,
                        sym.source_range.start_line as i64,
                        sym.source_range.start_column as i64,
                        sym.source_range.end_line as i64,
                        sym.source_range.end_column as i64,
                        sym.source_range.start_byte as i64,
                        sym.source_range.end_byte as i64,
                        sym.signature,
                        sym.doc_comment,
                        sym.parent_symbol_id,
                        sym.visibility,
                        sym.content_hash
                    ],
                )?;

                tx.execute(
                    r#"
                    INSERT INTO symbols_fts (
                        symbol_id, snapshot_id, workspace_id, name, qualified_name, signature, doc_comment, file_path
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    "#,
                    params![
                        sym.symbol_id,
                        sym.snapshot_id,
                        sym.workspace_id,
                        sym.name,
                        sym.qualified_name,
                        sym.signature,
                        sym.doc_comment.as_deref().unwrap_or(""),
                        sym.file_path
                    ],
                )?;
            }

            for r in &all_references {
                tx.execute(
                    r#"
                    INSERT OR REPLACE INTO "references" (
                        reference_id, snapshot_id, workspace_id, symbol_name, target_symbol_id,
                        file_path, start_line, start_column, end_line, end_column,
                        start_byte, end_byte, context_line, reference_kind, enclosing_symbol_id,
                        resolution_kind, content_hash
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                    "#,
                    params![
                        r.reference_id,
                        r.snapshot_id,
                        r.workspace_id,
                        r.symbol_name,
                        r.target_symbol_id,
                        r.file_path,
                        r.source_range.start_line as i64,
                        r.source_range.start_column as i64,
                        r.source_range.end_line as i64,
                        r.source_range.end_column as i64,
                        r.source_range.start_byte as i64,
                        r.source_range.end_byte as i64,
                        r.context_line,
                        r.reference_kind,
                        r.enclosing_symbol_id,
                        r.resolution_kind.as_str(),
                        r.content_hash
                    ],
                )?;
            }

            for rel in &all_relationships {
                let lim_json = serde_json::to_string(&rel.limitations)?;
                tx.execute(
                    r#"
                    INSERT OR REPLACE INTO relationships (
                        relationship_id, snapshot_id, workspace_id, from_symbol_id, from_name, from_file_path,
                        to_symbol_id, to_name, to_file_path, kind, start_line, start_column, end_line, end_column,
                        start_byte, end_byte, resolution_kind, content_hash, indexing_config, limitations_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                    "#,
                    params![
                        rel.relationship_id,
                        rel.snapshot_id,
                        rel.workspace_id,
                        rel.from_symbol_id,
                        rel.from_name,
                        rel.from_file_path,
                        rel.to_symbol_id,
                        rel.to_name,
                        rel.to_file_path,
                        rel.kind.as_str(),
                        rel.source_range.start_line as i64,
                        rel.source_range.start_column as i64,
                        rel.source_range.end_line as i64,
                        rel.source_range.end_column as i64,
                        rel.source_range.start_byte as i64,
                        rel.source_range.end_byte as i64,
                        rel.resolution_kind.as_str(),
                        rel.content_hash,
                        rel.indexing_config,
                        lim_json
                    ],
                )?;
            }

            for chunk in &all_chunks {
                tx.execute(
                    r#"
                    INSERT OR REPLACE INTO chunks (
                        chunk_id, snapshot_id, workspace_id, file_path, language, symbol_id, symbol_name,
                        start_line, start_column, end_line, end_column, start_byte, end_byte,
                        content, content_hash, token_estimate
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                    "#,
                    params![
                        chunk.chunk_id,
                        chunk.snapshot_id,
                        chunk.workspace_id,
                        chunk.file_path,
                        chunk.language,
                        chunk.symbol_id,
                        chunk.symbol_name,
                        chunk.source_range.start_line as i64,
                        chunk.source_range.start_column as i64,
                        chunk.source_range.end_line as i64,
                        chunk.source_range.end_column as i64,
                        chunk.source_range.start_byte as i64,
                        chunk.source_range.end_byte as i64,
                        chunk.content,
                        chunk.content_hash,
                        chunk.token_estimate as i64
                    ],
                )?;

                tx.execute(
                    r#"
                    INSERT INTO chunks_fts (
                        chunk_id, snapshot_id, workspace_id, file_path, symbol_name, content
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                    "#,
                    params![
                        chunk.chunk_id,
                        chunk.snapshot_id,
                        chunk.workspace_id,
                        chunk.file_path,
                        chunk.symbol_name.as_deref().unwrap_or(""),
                        chunk.content
                    ],
                )?;
            }

            for (chunk_id, model, dim, emb_json) in &chunk_embeddings {
                tx.execute(
                    r#"
                    INSERT OR REPLACE INTO vector_embeddings (
                        chunk_id, snapshot_id, workspace_id, model, dimensions, embedding_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                    "#,
                    params![chunk_id, snapshot_id, workspace_id, model, *dim as i64, emb_json],
                )?;
            }

            // Last cancellation checkpoint: dropping the transaction rolls back the snapshot.
            if self.is_job_cancelled(&job_id) {
                return Ok(false);
            }
            // Mark the job completed atomically with the snapshot so a concurrent
            // job_cancel cannot overwrite a committed snapshot's job as cancelled.
            tx.execute(
                r#"
                UPDATE jobs
                SET status = ?2, progress_pct = 100, processed_files = ?3,
                    snapshot_id = ?4, error = NULL, updated_at = ?5
                WHERE job_id = ?1
                "#,
                params![
                    job_id,
                    JobStatusState::Completed.as_str(),
                    total_files as i64,
                    snapshot_id,
                    timestamp_now()
                ],
            )?;
            tx.commit()?;
            Ok(true)
        })?;
        if !committed {
            return Err(self.abort_cancelled_job(&job_id, total_files));
        }

        let open_res = WorkspaceOpenResult {
            workspace_id: workspace_id.clone(),
            repository_id: repository_id.clone(),
            snapshot_id: snapshot_id.clone(),
            root_path: root_str,
            index_generation: next_gen,
            file_count: snapshot.file_count,
            symbol_count: snapshot.symbol_count,
            reference_count: snapshot.reference_count,
            relationship_count: snapshot.relationship_count,
            chunk_count: snapshot.chunk_count,
            job_id,
            reused_snapshot: false,
        };

        let evidence = vec![Evidence {
            repository_id,
            workspace_id,
            snapshot_id: snapshot_id.clone(),
            file_path: ".".to_string(),
            source_range: SourceRange::default(),
            content_hash: root_hash,
            producer: "forge-tree-sitter-indexer".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now,
        }];

        Ok(ResponseEnvelope::new(
            snapshot_id,
            Freshness::Current,
            vec![open_res],
            evidence,
            vec![
                "Tree-sitter AST extraction provides syntactic and heuristic call/import resolution."
                    .to_string(),
            ],
            false,
            None,
        ))
    }

    pub fn get_workspace_generation(&self, workspace_id: &str) -> Result<Option<i64>> {
        self.with_conn(|conn| {
            let gen: Option<i64> = conn
                .query_row(
                    "SELECT index_generation FROM workspaces WHERE workspace_id = ?1",
                    params![workspace_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(gen)
        })
    }

    pub fn get_workspace(&self, workspace_id: &str) -> Result<Workspace> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"
                SELECT workspace_id, repository_id, root_path, active_snapshot_id,
                       index_generation, created_at, updated_at
                FROM workspaces
                WHERE workspace_id = ?1
                "#,
                params![workspace_id],
                |row| {
                    Ok(Workspace {
                        workspace_id: row.get(0)?,
                        repository_id: row.get(1)?,
                        root_path: row.get(2)?,
                        active_snapshot_id: row.get(3)?,
                        index_generation: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                anyhow!(
                    "Workspace '{}' not found. Call workspace_open first.",
                    workspace_id
                )
            })
        })
    }

    pub fn get_active_snapshot(&self, workspace_id: &str) -> Result<Option<Snapshot>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"
                SELECT s.snapshot_id, s.repository_id, s.workspace_id, s.index_generation,
                       s.root_hash, s.file_count, s.symbol_count, s.reference_count,
                       s.relationship_count, s.chunk_count, s.created_at
                FROM snapshots s
                JOIN workspaces w ON w.active_snapshot_id = s.snapshot_id
                WHERE w.workspace_id = ?1
                "#,
                params![workspace_id],
                |row| {
                    Ok(Snapshot {
                        snapshot_id: row.get(0)?,
                        repository_id: row.get(1)?,
                        workspace_id: row.get(2)?,
                        index_generation: row.get(3)?,
                        root_hash: row.get(4)?,
                        file_count: row.get::<_, i64>(5)? as usize,
                        symbol_count: row.get::<_, i64>(6)? as usize,
                        reference_count: row.get::<_, i64>(7)? as usize,
                        relationship_count: row.get::<_, i64>(8)? as usize,
                        chunk_count: row.get::<_, i64>(9)? as usize,
                        created_at: row.get(10)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Check whether a workspace's active snapshot matches the live filesystem.
    pub fn check_workspace_freshness(
        &self,
        workspace_id: &str,
    ) -> Result<(Workspace, Snapshot, Freshness, Vec<String>)> {
        let ws = self.get_workspace(workspace_id)?;
        let snap = self
            .get_active_snapshot(workspace_id)?
            .ok_or_else(|| anyhow!("Workspace '{}' has no active snapshot", workspace_id))?;

        let indexed_hashes: HashMap<String, String> = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT file_path, content_hash FROM file_versions WHERE snapshot_id = ?1",
            )?;
            let rows = stmt.query_map(params![snap.snapshot_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut map = HashMap::new();
            for r in rows {
                let (fp, h) = r?;
                map.insert(fp, h);
            }
            Ok(map)
        })?;

        let root_path = Path::new(&ws.root_path);
        if !root_path.exists() {
            return Ok((
                ws,
                snap,
                Freshness::Stale,
                vec!["<workspace_root_missing>".to_string()],
            ));
        }

        let (live_root_hash, live_files) = Self::compute_workspace_state(root_path)?;
        if live_root_hash == snap.root_hash {
            return Ok((ws, snap, Freshness::Current, Vec::new()));
        }

        let mut changed_files = Vec::new();
        let mut seen_paths = HashSet::new();

        for (rel_path, _, _, _, live_hash) in &live_files {
            seen_paths.insert(rel_path.clone());
            match indexed_hashes.get(rel_path) {
                Some(old_hash) if old_hash == live_hash => {}
                _ => changed_files.push(rel_path.clone()),
            }
        }

        for indexed_path in indexed_hashes.keys() {
            if !seen_paths.contains(indexed_path) {
                changed_files.push(indexed_path.clone());
            }
        }
        changed_files.sort();
        Ok((ws, snap, Freshness::Stale, changed_files))
    }

    pub fn workspace_status(
        &self,
        req: &WorkspaceStatusRequest,
    ) -> Result<ResponseEnvelope<WorkspaceStatusResult>> {
        let (ws, snap, freshness, changed_files) =
            self.check_workspace_freshness(&req.workspace_id)?;
        let now = timestamp_now();

        let status_res = WorkspaceStatusResult {
            workspace_id: ws.workspace_id.clone(),
            repository_id: ws.repository_id.clone(),
            root_path: ws.root_path.clone(),
            snapshot_id: snap.snapshot_id.clone(),
            index_generation: snap.index_generation,
            freshness,
            file_count: snap.file_count,
            symbol_count: snap.symbol_count,
            reference_count: snap.reference_count,
            relationship_count: snap.relationship_count,
            chunk_count: snap.chunk_count,
            changed_files: changed_files.clone(),
            created_at: ws.created_at,
            updated_at: ws.updated_at,
        };

        let mut limitations = Vec::new();
        if freshness == Freshness::Stale {
            limitations.push(format!(
                "Snapshot {} is stale: {} file(s) changed on disk ({})",
                snap.snapshot_id,
                changed_files.len(),
                changed_files.join(", ")
            ));
        }

        let evidence = vec![Evidence {
            repository_id: ws.repository_id,
            workspace_id: ws.workspace_id,
            snapshot_id: snap.snapshot_id.clone(),
            file_path: ".".to_string(),
            source_range: SourceRange::default(),
            content_hash: snap.root_hash,
            producer: "forge-snapshot-coordinator".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now,
        }];

        Ok(ResponseEnvelope::new(
            snap.snapshot_id,
            freshness,
            vec![status_res],
            evidence,
            limitations,
            false,
            None,
        ))
    }

    pub fn get_file_content(
        &self,
        snapshot_id: &str,
        file_path: &str,
    ) -> Result<Option<(String, String)>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT content, content_hash FROM file_versions WHERE snapshot_id = ?1 AND file_path = ?2",
                params![snapshot_id, file_path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn find_symbols(&self, req: &SymbolFindRequest) -> Result<ResponseEnvelope<SymbolFindHit>> {
        let (ws, snap, freshness, changed_files) =
            self.check_workspace_freshness(&req.workspace_id)?;
        let limit = req.limit.unwrap_or(50).clamp(1, 500);
        let mode = req.match_mode.unwrap_or_default();
        let query_lower = req.name.to_ascii_lowercase();
        let kind_filter = match req.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => Some(
                SymbolKind::from_str_strict(k)
                    .ok_or_else(|| {
                        anyhow!(
                            "unknown symbol kind '{}'; expected one of: {}",
                            k,
                            SymbolKind::ACCEPTED_KINDS
                        )
                    })?
                    .as_str()
                    .to_string(),
            ),
            None => None,
        };
        let file_filter = req.file_path.as_ref().map(|p| p.replace('\\', "/"));
        let lang_filter = req.language.as_ref().map(|l| l.to_ascii_lowercase());

        let all_matching = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT symbol_id, snapshot_id, workspace_id, repository_id, name, qualified_name,
                       kind, language, file_path, start_line, start_column, end_line, end_column,
                       start_byte, end_byte, signature, doc_comment, parent_symbol_id, visibility, content_hash
                FROM symbols
                WHERE snapshot_id = ?1
                ORDER BY file_path ASC, start_line ASC
                "#,
            )?;
            let rows = stmt.query_map(params![snap.snapshot_id], Self::map_symbol_row)?;
            let mut matches = Vec::new();
            for row in rows {
                let sym = row?;
                let name_lower = sym.name.to_ascii_lowercase();
                let qual_lower = sym.qualified_name.to_ascii_lowercase();
                let name_matched = match mode {
                    MatchMode::Exact => name_lower == query_lower || qual_lower == query_lower,
                    MatchMode::Prefix => {
                        name_lower.starts_with(&query_lower) || qual_lower.starts_with(&query_lower)
                    }
                    MatchMode::Substring => {
                        name_lower.contains(&query_lower) || qual_lower.contains(&query_lower)
                    }
                };
                if !name_matched {
                    continue;
                }
                if let Some(ref kf) = kind_filter {
                    if sym.kind.as_str() != kf {
                        continue;
                    }
                }
                if let Some(ref ff) = file_filter {
                    if !sym.file_path.contains(ff) {
                        continue;
                    }
                }
                if let Some(ref lf) = lang_filter {
                    if &sym.language.to_ascii_lowercase() != lf {
                        continue;
                    }
                }
                matches.push(sym);
            }
            Ok(matches)
        })?;

        let truncated = all_matching.len() > limit;
        let now = timestamp_now();
        let mut results = Vec::new();
        let mut evidence = Vec::new();

        for sym in all_matching.into_iter().take(limit) {
            evidence.push(Evidence {
                repository_id: ws.repository_id.clone(),
                workspace_id: ws.workspace_id.clone(),
                snapshot_id: snap.snapshot_id.clone(),
                file_path: sym.file_path.clone(),
                source_range: sym.source_range,
                content_hash: sym.content_hash.clone(),
                producer: "forge-tree-sitter-symbol-index".to_string(),
                resolution_kind: ResolutionKind::Heuristic,
                observed_at: now.clone(),
            });
            results.push(SymbolFindHit { symbol: sym });
        }

        let mut limitations = vec!["Symbols extracted syntactically via Tree-sitter.".to_string()];
        if freshness == Freshness::Stale {
            limitations.push(format!(
                "Snapshot is stale; modified files: {}",
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

    pub fn read_symbol(
        &self,
        req: &SymbolReadRequest,
    ) -> Result<ResponseEnvelope<SymbolReadResult>> {
        let (ws, snap, freshness, changed_files) =
            self.check_workspace_freshness(&req.workspace_id)?;

        let resolved = self.resolve_symbol_candidates(
            &snap.snapshot_id,
            req.symbol_id.as_deref(),
            req.name.as_deref(),
            req.file_path.as_deref(),
        )?;
        let sym = resolved.symbol;

        let (file_content, _) = self
            .get_file_content(&snap.snapshot_id, &sym.file_path)?
            .ok_or_else(|| anyhow!("Source file '{}' not found in snapshot", sym.file_path))?;

        let lines: Vec<&str> = file_content.lines().collect();
        let start_idx = sym.source_range.start_line.saturating_sub(1);
        let end_idx = sym.source_range.end_line.min(lines.len());
        let body = if start_idx < end_idx {
            lines[start_idx..end_idx].join("\n")
        } else {
            sym.signature.clone()
        };

        let ctx_lines = req.context_lines.unwrap_or(3);
        let ctx_start = start_idx.saturating_sub(ctx_lines);
        let ctx_end = (end_idx + ctx_lines).min(lines.len());
        let enclosing_scope = if ctx_start < ctx_end {
            Some(lines[ctx_start..ctx_end].join("\n"))
        } else {
            None
        };

        let parent_symbol = if let Some(ref pid) = sym.parent_symbol_id {
            self.get_symbol_by_id(&snap.snapshot_id, pid)?
        } else {
            None
        };

        let now = timestamp_now();
        let evidence = vec![Evidence {
            repository_id: ws.repository_id,
            workspace_id: ws.workspace_id,
            snapshot_id: snap.snapshot_id.clone(),
            file_path: sym.file_path.clone(),
            source_range: sym.source_range,
            content_hash: sym.content_hash.clone(),
            producer: "forge-tree-sitter-symbol-reader".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now,
        }];

        let mut limitations = Vec::new();
        if freshness == Freshness::Stale {
            limitations.push(format!(
                "Snapshot is stale; modified files: {}",
                changed_files.join(", ")
            ));
        }
        limitations.extend(resolved.notes);
        if !resolved.alternatives.is_empty() {
            let others: Vec<String> = resolved
                .alternatives
                .iter()
                .map(|s| {
                    format!(
                        "{} ({} {}:{}, {})",
                        s.qualified_name,
                        s.kind.as_str(),
                        s.file_path,
                        s.source_range.start_line,
                        s.symbol_id
                    )
                })
                .collect();
            limitations.push(format!(
                "Ambiguous symbol name '{}': {} candidates matched; returned {} ({} {}:{}). Other candidates: {}. Pass symbol_id, file_path, or a qualified name to disambiguate.",
                req.name.as_deref().unwrap_or(&sym.name),
                resolved.alternatives.len() + 1,
                sym.qualified_name,
                sym.kind.as_str(),
                sym.file_path,
                sym.source_range.start_line,
                others.join("; ")
            ));
        }

        let res = SymbolReadResult {
            symbol: sym,
            body,
            enclosing_scope,
            parent_symbol,
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

    pub fn symbol_references(
        &self,
        req: &SymbolReferencesRequest,
    ) -> Result<ResponseEnvelope<SymbolReferenceHit>> {
        let (ws, snap, freshness, changed_files) =
            self.check_workspace_freshness(&req.workspace_id)?;
        let limit = req.limit.unwrap_or(100).clamp(1, 500);

        let eff_name = req.effective_symbol_name();
        let target_sym = if req.symbol_id.is_some() || eff_name.is_some() {
            self.resolve_symbol_in_snapshot(
                &snap.snapshot_id,
                req.symbol_id.as_deref(),
                eff_name,
                req.file_path.as_deref(),
            )
            .ok()
        } else {
            None
        };

        let search_name = target_sym
            .as_ref()
            .map(|s| s.name.clone())
            .or_else(|| eff_name.map(|s| s.to_string()))
            .ok_or_else(|| anyhow!("Provide either symbol_id or symbol_name"))?;
        let target_id = target_sym.as_ref().map(|s| s.symbol_id.clone());

        let refs = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT reference_id, snapshot_id, workspace_id, symbol_name, target_symbol_id,
                       file_path, start_line, start_column, end_line, end_column,
                       start_byte, end_byte, context_line, reference_kind, enclosing_symbol_id,
                       resolution_kind, content_hash
                FROM "references"
                WHERE snapshot_id = ?1
                  AND (symbol_name = ?2 OR (?3 IS NOT NULL AND target_symbol_id = ?3))
                ORDER BY file_path ASC, start_line ASC
                "#,
            )?;
            let rows =
                stmt.query_map(params![snap.snapshot_id, search_name, target_id], |row| {
                    Ok(Reference {
                        reference_id: row.get(0)?,
                        snapshot_id: row.get(1)?,
                        workspace_id: row.get(2)?,
                        symbol_name: row.get(3)?,
                        target_symbol_id: row.get(4)?,
                        file_path: row.get(5)?,
                        source_range: SourceRange {
                            start_line: row.get::<_, i64>(6)? as usize,
                            start_column: row.get::<_, i64>(7)? as usize,
                            end_line: row.get::<_, i64>(8)? as usize,
                            end_column: row.get::<_, i64>(9)? as usize,
                            start_byte: row.get::<_, i64>(10)? as usize,
                            end_byte: row.get::<_, i64>(11)? as usize,
                        },
                        context_line: row.get(12)?,
                        reference_kind: row.get(13)?,
                        enclosing_symbol_id: row.get(14)?,
                        resolution_kind: ResolutionKind::from_str_lossy(&row.get::<_, String>(15)?),
                        content_hash: row.get(16)?,
                    })
                })?;
            let mut list = Vec::new();
            for r in rows {
                list.push(r?);
            }
            Ok(list)
        })?;

        // Optional file_path restricts hits to that file (in addition to
        // disambiguating the target symbol above).
        let norm_fp = req.file_path.as_deref().map(|p| p.replace('\\', "/"));
        let mut refs: Vec<Reference> = refs
            .into_iter()
            .filter(|r| match &norm_fp {
                Some(fp) => &r.file_path == fp || r.file_path.ends_with(fp),
                None => true,
            })
            .collect();

        // include_declarations: prepend the declaration site from the symbols table.
        let include_decl = req.include_declarations.unwrap_or(false);
        let mut decl_ref: Option<Reference> = None;
        if include_decl {
            if let Some(sym) = target_sym.as_ref() {
                let in_file = match &norm_fp {
                    Some(fp) => &sym.file_path == fp || sym.file_path.ends_with(fp),
                    None => true,
                };
                if in_file {
                    let context_line = sym.signature.lines().next().unwrap_or("").to_string();
                    decl_ref = Some(Reference {
                        reference_id: format!("decl-{}", sym.symbol_id),
                        snapshot_id: snap.snapshot_id.clone(),
                        workspace_id: ws.workspace_id.clone(),
                        symbol_name: sym.name.clone(),
                        target_symbol_id: Some(sym.symbol_id.clone()),
                        file_path: sym.file_path.clone(),
                        source_range: sym.source_range,
                        context_line,
                        reference_kind: "declaration".to_string(),
                        enclosing_symbol_id: sym.parent_symbol_id.clone(),
                        resolution_kind: ResolutionKind::Heuristic,
                        content_hash: sym.content_hash.clone(),
                    });
                }
            }
        }
        if let Some(d) = decl_ref {
            refs.insert(0, d);
        }

        let truncated = refs.len() > limit;
        let now = timestamp_now();
        let mut results = Vec::new();
        let mut evidence = Vec::new();

        for r in refs.into_iter().take(limit) {
            let surrounding_context =
                if let Ok(Some((fc, _))) = self.get_file_content(&snap.snapshot_id, &r.file_path) {
                    let lines: Vec<&str> = fc.lines().collect();
                    let line_idx = r.source_range.start_line.saturating_sub(1);
                    let s = line_idx.saturating_sub(1);
                    let e = (line_idx + 2).min(lines.len());
                    if s < e {
                        lines[s..e].join("\n")
                    } else {
                        r.context_line.clone()
                    }
                } else {
                    r.context_line.clone()
                };

            let enclosing_symbol_name = if let Some(ref enc_id) = r.enclosing_symbol_id {
                self.get_symbol_by_id(&snap.snapshot_id, enc_id)?
                    .map(|s| s.qualified_name)
            } else {
                None
            };

            evidence.push(Evidence {
                repository_id: ws.repository_id.clone(),
                workspace_id: ws.workspace_id.clone(),
                snapshot_id: snap.snapshot_id.clone(),
                file_path: r.file_path.clone(),
                source_range: r.source_range,
                content_hash: r.content_hash.clone(),
                producer: if r.reference_kind == "declaration" {
                    "forge-tree-sitter-symbol-reader".to_string()
                } else {
                    "forge-tree-sitter-reference-extractor".to_string()
                },
                resolution_kind: r.resolution_kind,
                observed_at: now.clone(),
            });

            results.push(SymbolReferenceHit {
                reference: r,
                surrounding_context,
                enclosing_symbol_name,
            });
        }

        let mut limitations = vec![
            "Reference matching uses syntactic Tree-sitter call/identifier extraction (heuristic resolution)."
                .to_string(),
        ];
        if freshness == Freshness::Stale {
            limitations.push(format!(
                "Snapshot is stale; modified files: {}",
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

    pub fn get_symbol_by_id(&self, snapshot_id: &str, symbol_id: &str) -> Result<Option<Symbol>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"
                SELECT symbol_id, snapshot_id, workspace_id, repository_id, name, qualified_name,
                       kind, language, file_path, start_line, start_column, end_line, end_column,
                       start_byte, end_byte, signature, doc_comment, parent_symbol_id, visibility, content_hash
                FROM symbols
                WHERE snapshot_id = ?1 AND symbol_id = ?2
                "#,
                params![snapshot_id, symbol_id],
                Self::map_symbol_row,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn resolve_symbol_in_snapshot(
        &self,
        snapshot_id: &str,
        symbol_id: Option<&str>,
        name: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<Symbol> {
        self.resolve_symbol_candidates(snapshot_id, symbol_id, name, file_path)
            .map(|r| r.symbol)
    }

    /// Resolve a symbol by id or name, returning the chosen symbol plus any
    /// other candidates that matched (so callers can report ambiguity).
    ///
    /// Candidate ranking when a name matches several symbols:
    /// 1. `file_path` filter (exact or suffix match, `\` normalized to `/`).
    /// 2. Definitions (multi-line bodies) before single-line declarations
    ///    such as trait method signatures.
    /// 3. Stable order by (file_path, start_line).
    pub fn resolve_symbol_candidates(
        &self,
        snapshot_id: &str,
        symbol_id: Option<&str>,
        name: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<ResolvedSymbol> {
        let mut notes = Vec::new();
        if let Some(sid) = symbol_id {
            if let Some(sym) = self.get_symbol_by_id(snapshot_id, sid)? {
                return Ok(ResolvedSymbol {
                    symbol: sym,
                    alternatives: Vec::new(),
                    notes,
                });
            }
            if name.is_none() {
                return Err(anyhow!(
                    "Symbol '{}' not found in snapshot {}",
                    sid,
                    snapshot_id
                ));
            }
            notes.push(format!(
                "symbol_id '{}' not found in snapshot {}; fell back to name lookup",
                sid, snapshot_id
            ));
        }
        let nm = match name {
            Some(nm) if !nm.trim().is_empty() => nm,
            _ => {
                return Err(anyhow!(
                    "Symbol lookup requires 'symbol_id' or 'name' (neither was provided)"
                ))
            }
        };

        let norm_fp = file_path.map(|p| p.replace('\\', "/"));
        let syms = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT symbol_id, snapshot_id, workspace_id, repository_id, name, qualified_name,
                       kind, language, file_path, start_line, start_column, end_line, end_column,
                       start_byte, end_byte, signature, doc_comment, parent_symbol_id, visibility, content_hash
                FROM symbols
                WHERE snapshot_id = ?1 AND (name = ?2 OR qualified_name = ?2)
                ORDER BY file_path ASC, start_line ASC
                "#,
            )?;
            let rows = stmt.query_map(params![snapshot_id, nm], Self::map_symbol_row)?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            Ok(v)
        })?;

        let mut candidates: Vec<Symbol> = match norm_fp {
            Some(ref fp) => {
                let filtered: Vec<Symbol> = syms
                    .iter()
                    .filter(|s| &s.file_path == fp || s.file_path.ends_with(fp.as_str()))
                    .cloned()
                    .collect();
                if filtered.is_empty() {
                    if !syms.is_empty() {
                        notes.push(format!(
                            "No symbol named '{}' in file '{}'; fell back to workspace-wide lookup",
                            nm, fp
                        ));
                    }
                    syms
                } else {
                    filtered
                }
            }
            None => syms,
        };

        if candidates.is_empty() {
            return Err(anyhow!(
                "Symbol '{}' not found in snapshot {}{}",
                nm,
                snapshot_id,
                file_path
                    .map(|p| format!(" (file_path={})", p))
                    .unwrap_or_default()
            ));
        }

        // Prefer definitions with bodies over one-line declarations; keep (file_path, start_line) order otherwise.
        let is_definition = |s: &Symbol| s.source_range.end_line > s.source_range.start_line;
        if let Some(pos) = candidates.iter().position(is_definition) {
            if pos != 0 {
                let chosen = candidates.remove(pos);
                candidates.insert(0, chosen);
            }
        }

        let symbol = candidates.remove(0);
        Ok(ResolvedSymbol {
            symbol,
            alternatives: candidates,
            notes,
        })
    }

    pub fn list_snapshot_symbols(&self, snapshot_id: &str) -> Result<Vec<Symbol>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT symbol_id, snapshot_id, workspace_id, repository_id, name, qualified_name,
                       kind, language, file_path, start_line, start_column, end_line, end_column,
                       start_byte, end_byte, signature, doc_comment, parent_symbol_id, visibility, content_hash
                FROM symbols
                WHERE snapshot_id = ?1
                ORDER BY file_path ASC, start_line ASC
                "#,
            )?;
            let rows = stmt.query_map(params![snapshot_id], Self::map_symbol_row)?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            Ok(v)
        })
    }

    pub fn list_snapshot_relationships(&self, snapshot_id: &str) -> Result<Vec<Relationship>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT relationship_id, snapshot_id, workspace_id, from_symbol_id, from_name, from_file_path,
                       to_symbol_id, to_name, to_file_path, kind, start_line, start_column, end_line, end_column,
                       start_byte, end_byte, resolution_kind, content_hash, indexing_config, limitations_json
                FROM relationships
                WHERE snapshot_id = ?1
                "#,
            )?;
            let rows = stmt.query_map(params![snapshot_id], |row| {
                let lim_str: String = row.get(19)?;
                let limitations: Vec<String> =
                    serde_json::from_str(&lim_str).unwrap_or_default();
                Ok(Relationship {
                    relationship_id: row.get(0)?,
                    snapshot_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    from_symbol_id: row.get(3)?,
                    from_name: row.get(4)?,
                    from_file_path: row.get(5)?,
                    to_symbol_id: row.get(6)?,
                    to_name: row.get(7)?,
                    to_file_path: row.get(8)?,
                    kind: RelationshipKind::from_str_lossy(&row.get::<_, String>(9)?),
                    source_range: SourceRange {
                        start_line: row.get::<_, i64>(10)? as usize,
                        start_column: row.get::<_, i64>(11)? as usize,
                        end_line: row.get::<_, i64>(12)? as usize,
                        end_column: row.get::<_, i64>(13)? as usize,
                        start_byte: row.get::<_, i64>(14)? as usize,
                        end_byte: row.get::<_, i64>(15)? as usize,
                    },
                    resolution_kind: ResolutionKind::from_str_lossy(&row.get::<_, String>(16)?),
                    content_hash: row.get(17)?,
                    indexing_config: row.get(18)?,
                    limitations,
                })
            })?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            Ok(v)
        })
    }

    pub fn list_snapshot_chunks(&self, snapshot_id: &str) -> Result<Vec<CodeChunk>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT chunk_id, snapshot_id, workspace_id, file_path, language, symbol_id, symbol_name,
                       start_line, start_column, end_line, end_column, start_byte, end_byte,
                       content, content_hash, token_estimate
                FROM chunks
                WHERE snapshot_id = ?1
                "#,
            )?;
            let rows = stmt.query_map(params![snapshot_id], Self::map_chunk_row)?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            Ok(v)
        })
    }

    pub fn list_snapshot_embeddings(&self, snapshot_id: &str) -> Result<HashMap<String, Vec<f32>>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT chunk_id, embedding_json FROM vector_embeddings WHERE snapshot_id = ?1",
            )?;
            let rows = stmt.query_map(params![snapshot_id], |row| {
                let cid: String = row.get(0)?;
                let json: String = row.get(1)?;
                Ok((cid, json))
            })?;
            let mut map = HashMap::new();
            for r in rows {
                let (cid, json) = r?;
                if let Ok(vec) = serde_json::from_str::<Vec<f32>>(&json) {
                    map.insert(cid, vec);
                }
            }
            Ok(map)
        })
    }

    pub fn map_symbol_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Symbol> {
        Ok(Symbol {
            symbol_id: row.get(0)?,
            snapshot_id: row.get(1)?,
            workspace_id: row.get(2)?,
            repository_id: row.get(3)?,
            name: row.get(4)?,
            qualified_name: row.get(5)?,
            kind: SymbolKind::from_str_lossy(&row.get::<_, String>(6)?),
            language: row.get(7)?,
            file_path: row.get(8)?,
            source_range: SourceRange {
                start_line: row.get::<_, i64>(9)? as usize,
                start_column: row.get::<_, i64>(10)? as usize,
                end_line: row.get::<_, i64>(11)? as usize,
                end_column: row.get::<_, i64>(12)? as usize,
                start_byte: row.get::<_, i64>(13)? as usize,
                end_byte: row.get::<_, i64>(14)? as usize,
            },
            signature: row.get(15)?,
            doc_comment: row.get(16)?,
            parent_symbol_id: row.get(17)?,
            visibility: row.get(18)?,
            content_hash: row.get(19)?,
        })
    }

    pub fn map_chunk_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodeChunk> {
        Ok(CodeChunk {
            chunk_id: row.get(0)?,
            snapshot_id: row.get(1)?,
            workspace_id: row.get(2)?,
            file_path: row.get(3)?,
            language: row.get(4)?,
            symbol_id: row.get(5)?,
            symbol_name: row.get(6)?,
            source_range: SourceRange {
                start_line: row.get::<_, i64>(7)? as usize,
                start_column: row.get::<_, i64>(8)? as usize,
                end_line: row.get::<_, i64>(9)? as usize,
                end_column: row.get::<_, i64>(10)? as usize,
                start_byte: row.get::<_, i64>(11)? as usize,
                end_byte: row.get::<_, i64>(12)? as usize,
            },
            content: row.get(13)?,
            content_hash: row.get(14)?,
            token_estimate: row.get::<_, i64>(15)? as usize,
        })
    }

    // ========================================================================
    // Job Management
    // ========================================================================

    pub fn insert_job(&self, job: &JobRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO jobs (
                    job_id, workspace_id, kind, status, progress_pct,
                    processed_files, total_files, snapshot_id, error, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                params![
                    job.job_id,
                    job.workspace_id,
                    job.kind,
                    job.status.as_str(),
                    job.progress_pct as i64,
                    job.processed_files as i64,
                    job.total_files as i64,
                    job.snapshot_id,
                    job.error,
                    job.created_at,
                    job.updated_at
                ],
            )?;
            Ok(())
        })
    }

    pub fn update_job_status(
        &self,
        job_id: &str,
        status: JobStatusState,
        progress_pct: u8,
        processed_files: usize,
        snapshot_id: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        let now = timestamp_now();
        self.with_conn(|conn| {
            conn.execute(
                r#"
                UPDATE jobs
                SET status = ?2,
                    progress_pct = ?3,
                    processed_files = ?4,
                    snapshot_id = COALESCE(?5, snapshot_id),
                    error = ?6,
                    updated_at = ?7
                WHERE job_id = ?1
                "#,
                params![
                    job_id,
                    status.as_str(),
                    progress_pct as i64,
                    processed_files as i64,
                    snapshot_id,
                    error,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_job(&self, job_id: &str) -> Result<JobRecord> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"
                SELECT job_id, workspace_id, kind, status, progress_pct,
                       processed_files, total_files, snapshot_id, error, created_at, updated_at
                FROM jobs
                WHERE job_id = ?1
                "#,
                params![job_id],
                |row| {
                    Ok(JobRecord {
                        job_id: row.get(0)?,
                        workspace_id: row.get(1)?,
                        kind: row.get(2)?,
                        status: JobStatusState::from_str_lossy(&row.get::<_, String>(3)?),
                        progress_pct: row.get::<_, i64>(4)? as u8,
                        processed_files: row.get::<_, i64>(5)? as usize,
                        total_files: row.get::<_, i64>(6)? as usize,
                        snapshot_id: row.get(7)?,
                        error: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("Job '{}' not found", job_id))
        })
    }

    pub fn job_status(&self, req: &JobStatusRequest) -> Result<ResponseEnvelope<JobRecord>> {
        let job = if let Some(ref jid) = req.job_id {
            self.get_job(jid)?
        } else if let Some(ref wid) = req.workspace_id {
            self.with_conn(|conn| {
                conn.query_row(
                    r#"
                    SELECT job_id, workspace_id, kind, status, progress_pct,
                           processed_files, total_files, snapshot_id, error, created_at, updated_at
                    FROM jobs
                    WHERE workspace_id = ?1
                    ORDER BY created_at DESC
                    LIMIT 1
                    "#,
                    params![wid],
                    |row| {
                        Ok(JobRecord {
                            job_id: row.get(0)?,
                            workspace_id: row.get(1)?,
                            kind: row.get(2)?,
                            status: JobStatusState::from_str_lossy(&row.get::<_, String>(3)?),
                            progress_pct: row.get::<_, i64>(4)? as u8,
                            processed_files: row.get::<_, i64>(5)? as usize,
                            total_files: row.get::<_, i64>(6)? as usize,
                            snapshot_id: row.get(7)?,
                            error: row.get(8)?,
                            created_at: row.get(9)?,
                            updated_at: row.get(10)?,
                        })
                    },
                )
                .optional()?
                .ok_or_else(|| anyhow!("No jobs found for workspace '{}'", wid))
            })?
        } else {
            return Err(anyhow!("Provide job_id or workspace_id for job_status"));
        };
        let (snapshot_id, freshness) =
            if let Ok((_, snap, fresh, _)) = self.check_workspace_freshness(&job.workspace_id) {
                (snap.snapshot_id, fresh)
            } else {
                (
                    job.snapshot_id
                        .clone()
                        .unwrap_or_else(|| "none".to_string()),
                    Freshness::Current,
                )
            };
        let now = timestamp_now();
        let evidence = vec![Evidence {
            repository_id: format!("repo-{}", job.workspace_id),
            workspace_id: job.workspace_id.clone(),
            snapshot_id: snapshot_id.clone(),
            file_path: ".".to_string(),
            source_range: SourceRange::default(),
            content_hash: sha256_hex(job.job_id.as_bytes()),
            producer: "forge-job-manager".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now,
        }];
        Ok(ResponseEnvelope::new(
            snapshot_id,
            freshness,
            vec![job],
            evidence,
            vec![],
            false,
            None,
        ))
    }

    pub fn cancel_job(&self, req: &JobCancelRequest) -> Result<ResponseEnvelope<JobRecord>> {
        // Signal cancellation before touching the DB: get_job may block on the connection
        // mutex while the indexer's write transaction runs, and the indexer re-checks this
        // set just before committing.
        if let Ok(mut set) = self.cancelled_jobs.lock() {
            set.insert(req.job_id.clone());
        }
        let unsignal = || {
            if let Ok(mut set) = self.cancelled_jobs.lock() {
                set.remove(&req.job_id);
            }
        };
        let mut job = match self.get_job(&req.job_id) {
            Ok(job) => job,
            Err(e) => {
                unsignal();
                return Err(e);
            }
        };
        let mut limitations = Vec::new();
        if matches!(job.status, JobStatusState::Queued | JobStatusState::Running) {
            limitations.push(
                "Cancellation requested; indexing stops at the next file boundary and no snapshot is activated."
                    .to_string(),
            );
            self.update_job_status(
                &job.job_id,
                JobStatusState::Cancelled,
                job.progress_pct,
                job.processed_files,
                job.snapshot_id.clone(),
                Some("Cancelled via job_cancel".to_string()),
            )?;
            job = self.get_job(&req.job_id)?;
        } else {
            unsignal();
            limitations.push(format!(
                "Job already {}; nothing to cancel.",
                job.status.as_str()
            ));
        }
        let snapshot_id = job
            .snapshot_id
            .clone()
            .unwrap_or_else(|| "none".to_string());
        let now = timestamp_now();
        let evidence = vec![Evidence {
            repository_id: format!("repo-{}", job.workspace_id),
            workspace_id: job.workspace_id.clone(),
            snapshot_id: snapshot_id.clone(),
            file_path: ".".to_string(),
            source_range: SourceRange::default(),
            content_hash: sha256_hex(job.job_id.as_bytes()),
            producer: "forge-job-manager".to_string(),
            resolution_kind: ResolutionKind::Heuristic,
            observed_at: now,
        }];
        Ok(ResponseEnvelope::new(
            snapshot_id,
            Freshness::Current,
            vec![job],
            evidence,
            limitations,
            false,
            None,
        ))
    }

    /// Record a cancellation observed after file parsing finished and build the error
    /// returned to the workspace_open caller.
    fn abort_cancelled_job(&self, job_id: &str, total_files: usize) -> anyhow::Error {
        if let Err(e) = self.update_job_status(
            job_id,
            JobStatusState::Cancelled,
            100,
            total_files,
            None,
            Some("Job cancelled by user".to_string()),
        ) {
            return e;
        }
        anyhow!("Indexing job {} was cancelled", job_id)
    }

    pub fn is_job_cancelled(&self, job_id: &str) -> bool {
        self.cancelled_jobs
            .lock()
            .map(|s| s.contains(job_id))
            .unwrap_or(false)
    }
}

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Normalize a canonicalized filesystem path, stripping the Windows extended-length `\\?\` prefix
/// so path comparisons and user-facing paths remain clean and consistent across platforms.
pub fn normalize_canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    let canon = std::fs::canonicalize(path)?;
    let s = canon.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\UNC\") {
        Ok(PathBuf::from(format!(r"\\{}", stripped)))
    } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
        Ok(PathBuf::from(stripped))
    } else {
        Ok(canon)
    }
}

/// Convert a relative path into a normalized forward-slash string (`src/main.rs`).
pub fn to_slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => Some(os.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Estimate token count for a string (~4 chars per token or word/punctuation split).
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let char_est = text.len().div_ceil(4);
    let word_est = text.split_whitespace().count();
    char_est.max(word_est).max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Current,
    Stale,
}

impl Freshness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Freshness::Current => "current",
            Freshness::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionKind {
    CompilerResolved,
    Heuristic,
    Unresolved,
}

impl ResolutionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResolutionKind::CompilerResolved => "compiler_resolved",
            ResolutionKind::Heuristic => "heuristic",
            ResolutionKind::Unresolved => "unresolved",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "compiler_resolved" => ResolutionKind::CompilerResolved,
            "unresolved" => ResolutionKind::Unresolved,
            _ => ResolutionKind::Heuristic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SourceRange {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub repository_id: String,
    pub workspace_id: String,
    pub snapshot_id: String,
    pub file_path: String,
    pub source_range: SourceRange,
    pub content_hash: String,
    pub producer: String,
    pub resolution_kind: ResolutionKind,
    pub observed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope<T> {
    pub snapshot_id: String,
    pub freshness: Freshness,
    pub results: Vec<T>,
    pub evidence: Vec<Evidence>,
    pub limitations: Vec<String>,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

impl<T> ResponseEnvelope<T> {
    pub fn new(
        snapshot_id: impl Into<String>,
        freshness: Freshness,
        results: Vec<T>,
        evidence: Vec<Evidence>,
        limitations: Vec<String>,
        truncated: bool,
        next_cursor: Option<String>,
    ) -> Self {
        Self {
            snapshot_id: snapshot_id.into(),
            freshness,
            results,
            evidence,
            limitations,
            truncated,
            next_cursor,
        }
    }
}

// ============================================================================
// Core Entities (Section 6 of Forge Specification)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub repository_id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub repository_id: String,
    pub root_path: String,
    pub active_snapshot_id: Option<String>,
    pub index_generation: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_id: String,
    pub repository_id: String,
    pub workspace_id: String,
    pub index_generation: i64,
    pub root_hash: String,
    pub file_count: usize,
    pub symbol_count: usize,
    pub reference_count: usize,
    pub relationship_count: usize,
    pub chunk_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileVersion {
    pub file_id: String,
    pub snapshot_id: String,
    pub workspace_id: String,
    pub file_path: String,
    pub language: String,
    pub content_hash: String,
    pub size_bytes: usize,
    pub line_count: usize,
    pub indexed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Struct,
    Class,
    Interface,
    Method,
    Trait,
    Module,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Struct => "struct",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Method => "method",
            SymbolKind::Trait => "trait",
            SymbolKind::Module => "module",
        }
    }

    /// Accepted spellings for `kind` filters, listed in error messages.
    pub const ACCEPTED_KINDS: &'static str =
        "function, struct, class, interface, method, trait, module (aliases: fn, func, def, enum, mod, package, namespace)";

    /// Strict parse: returns `None` for unknown kinds instead of guessing.
    /// `enum` aliases to `Struct` because the parser indexes `enum_item` as `Struct`.
    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "function" | "fn" | "func" | "def" => Some(SymbolKind::Function),
            "struct" | "enum" => Some(SymbolKind::Struct),
            "class" => Some(SymbolKind::Class),
            "interface" => Some(SymbolKind::Interface),
            "method" => Some(SymbolKind::Method),
            "trait" => Some(SymbolKind::Trait),
            "module" | "mod" | "package" | "namespace" => Some(SymbolKind::Module),
            _ => None,
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        Self::from_str_strict(s).unwrap_or(SymbolKind::Function)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub symbol_id: String,
    pub snapshot_id: String,
    pub workspace_id: String,
    pub repository_id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub language: String,
    pub file_path: String,
    pub source_range: SourceRange,
    pub signature: String,
    pub doc_comment: Option<String>,
    pub parent_symbol_id: Option<String>,
    pub visibility: Option<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub reference_id: String,
    pub snapshot_id: String,
    pub workspace_id: String,
    pub symbol_name: String,
    pub target_symbol_id: Option<String>,
    pub file_path: String,
    pub source_range: SourceRange,
    pub context_line: String,
    pub reference_kind: String,
    pub enclosing_symbol_id: Option<String>,
    pub resolution_kind: ResolutionKind,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    Calls,
    Imports,
    Contains,
    Implements,
    Inherits,
    References,
}

impl RelationshipKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelationshipKind::Calls => "calls",
            RelationshipKind::Imports => "imports",
            RelationshipKind::Contains => "contains",
            RelationshipKind::Implements => "implements",
            RelationshipKind::Inherits => "inherits",
            RelationshipKind::References => "references",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "calls" | "call" => RelationshipKind::Calls,
            "imports" | "import" => RelationshipKind::Imports,
            "contains" | "contain" | "children" => RelationshipKind::Contains,
            "implements" | "implement" => RelationshipKind::Implements,
            "inherits" | "inherit" | "extends" => RelationshipKind::Inherits,
            _ => RelationshipKind::References,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub relationship_id: String,
    pub snapshot_id: String,
    pub workspace_id: String,
    pub from_symbol_id: Option<String>,
    pub from_name: String,
    pub from_file_path: String,
    pub to_symbol_id: Option<String>,
    pub to_name: String,
    pub to_file_path: Option<String>,
    pub kind: RelationshipKind,
    pub source_range: SourceRange,
    pub resolution_kind: ResolutionKind,
    pub content_hash: String,
    pub indexing_config: String,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    pub chunk_id: String,
    pub snapshot_id: String,
    pub workspace_id: String,
    pub file_path: String,
    pub language: String,
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub source_range: SourceRange,
    pub content: String,
    pub content_hash: String,
    pub token_estimate: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub diagnostic_id: String,
    pub snapshot_id: String,
    pub file_path: String,
    pub severity: String,
    pub message: String,
    pub source_range: SourceRange,
    pub producer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub finding_id: String,
    pub snapshot_id: String,
    pub rule_id: String,
    pub severity: String,
    pub message: String,
    pub file_path: String,
    pub source_range: SourceRange,
    pub resolution_kind: ResolutionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    pub changeset_id: String,
    pub workspace_id: String,
    pub base_snapshot_id: String,
    pub description: String,
    pub affected_files: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub entry_id: String,
    pub workspace_id: String,
    pub key: String,
    pub content: String,
    pub linked_snapshot_id: Option<String>,
    pub updated_at: String,
}

// ============================================================================
// Job Tracking Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatusState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl JobStatusState {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatusState::Queued => "queued",
            JobStatusState::Running => "running",
            JobStatusState::Completed => "completed",
            JobStatusState::Cancelled => "cancelled",
            JobStatusState::Failed => "failed",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "queued" => JobStatusState::Queued,
            "running" => JobStatusState::Running,
            "completed" => JobStatusState::Completed,
            "cancelled" => JobStatusState::Cancelled,
            _ => JobStatusState::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub job_id: String,
    pub workspace_id: String,
    pub kind: String,
    pub status: JobStatusState,
    pub progress_pct: u8,
    pub processed_files: usize,
    pub total_files: usize,
    pub snapshot_id: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// ============================================================================
// MCP Tool Request & Response Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceOpenRequest {
    pub path: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub force_reindex: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceOpenResult {
    pub workspace_id: String,
    pub repository_id: String,
    pub snapshot_id: String,
    pub root_path: String,
    pub index_generation: i64,
    pub file_count: usize,
    pub symbol_count: usize,
    pub reference_count: usize,
    pub relationship_count: usize,
    pub chunk_count: usize,
    pub job_id: String,
    pub reused_snapshot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatusRequest {
    pub workspace_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatusResult {
    pub workspace_id: String,
    pub repository_id: String,
    pub root_path: String,
    pub snapshot_id: String,
    pub index_generation: i64,
    pub freshness: Freshness,
    pub file_count: usize,
    pub symbol_count: usize,
    pub reference_count: usize,
    pub relationship_count: usize,
    pub chunk_count: usize,
    pub changed_files: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSearchRequest {
    pub workspace_id: String,
    pub query: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_tests: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSearchHit {
    pub chunk_id: String,
    pub file_path: String,
    pub language: String,
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub symbol_kind: Option<SymbolKind>,
    pub signature: Option<String>,
    pub source_range: SourceRange,
    pub content: String,
    pub content_hash: String,
    pub rrf_score: f64,
    pub lexical_score: f64,
    pub symbol_score: f64,
    pub vector_score: f64,
    pub match_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum MatchMode {
    Exact,
    Prefix,
    #[default]
    Substring,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolFindRequest {
    pub workspace_id: String,
    pub name: String,
    #[serde(default)]
    pub match_mode: Option<MatchMode>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolFindHit {
    #[serde(flatten)]
    pub symbol: Symbol,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolReadRequest {
    pub workspace_id: String,
    #[serde(default)]
    pub symbol_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub context_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolReadResult {
    pub symbol: Symbol,
    pub body: String,
    pub enclosing_scope: Option<String>,
    pub parent_symbol: Option<Symbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolReferencesRequest {
    pub workspace_id: String,
    #[serde(default)]
    pub symbol_id: Option<String>,
    #[serde(default)]
    pub symbol_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub include_declarations: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
}

impl SymbolReferencesRequest {
    pub fn effective_symbol_name(&self) -> Option<&str> {
        self.symbol_name
            .as_deref()
            .or(self.name.as_deref())
            .or(self.symbol.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolReferenceHit {
    pub reference: Reference,
    pub surrounding_context: String,
    pub enclosing_symbol_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphQueryKind {
    Callers,
    Callees,
    Imports,
    Importers,
    Children,
    Implements,
    Path,
}

impl GraphQueryKind {
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "callers" | "caller" => GraphQueryKind::Callers,
            "callees" | "callee" | "calls" => GraphQueryKind::Callees,
            "imports" | "import" => GraphQueryKind::Imports,
            "importers" | "importer" | "imported_by" => GraphQueryKind::Importers,
            "children" | "contains" | "child" => GraphQueryKind::Children,
            "implements" | "inheritance" | "inherits" => GraphQueryKind::Implements,
            "path" | "shortest_path" => GraphQueryKind::Path,
            _ => GraphQueryKind::Callers,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryRequest {
    pub workspace_id: String,
    #[serde(default)]
    pub query_kind: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub symbol_id: Option<String>,
    #[serde(default)]
    pub symbol_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub target_symbol_id: Option<String>,
    #[serde(default)]
    pub target_symbol_name: Option<String>,
    #[serde(default)]
    pub max_depth: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

impl GraphQueryRequest {
    pub fn effective_query_kind(&self) -> &str {
        self.query_kind
            .as_deref()
            .or(self.direction.as_deref())
            .or(self.kind.as_deref())
            .or(self.relation.as_deref())
            .unwrap_or("callers")
    }

    pub fn effective_symbol_name(&self) -> Option<&str> {
        self.symbol_name
            .as_deref()
            .or(self.name.as_deref())
            .or(self.symbol.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdgeHop {
    pub depth: usize,
    pub relationship: Relationship,
    pub from_symbol: Option<Symbol>,
    pub to_symbol: Option<Symbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryResult {
    pub query_kind: String,
    pub root_symbol: Option<Symbol>,
    pub root_name: String,
    pub max_depth: usize,
    pub hops: Vec<GraphEdgeHop>,
    pub path_found: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalyzeRequest {
    pub workspace_id: String,
    #[serde(default)]
    pub symbol_id: Option<String>,
    #[serde(default)]
    pub symbol_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub max_depth: Option<usize>,
}

impl ImpactAnalyzeRequest {
    pub fn effective_symbol_name(&self) -> Option<&str> {
        self.symbol_name
            .as_deref()
            .or(self.name.as_deref())
            .or(self.symbol.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactDependent {
    pub symbol_id: Option<String>,
    pub symbol_name: String,
    pub file_path: String,
    pub distance: usize,
    pub relationship_kind: RelationshipKind,
    pub resolution_kind: ResolutionKind,
    pub source_range: SourceRange,
    pub is_test: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalyzeResult {
    pub target_symbol: Option<Symbol>,
    pub target_file_path: Option<String>,
    pub direct_dependents: Vec<ImpactDependent>,
    pub transitive_dependents: Vec<ImpactDependent>,
    pub affected_files: Vec<String>,
    pub impacted_test_files: Vec<String>,
    pub max_depth_reached: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBuildRequest {
    pub workspace_id: String,
    pub query: String,
    #[serde(default)]
    pub token_budget: Option<usize>,
    #[serde(default)]
    pub include_tests: Option<bool>,
    #[serde(default)]
    pub snapshot_policy: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub path_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnippet {
    pub file_path: String,
    pub language: String,
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub symbol_kind: Option<SymbolKind>,
    pub signature: Option<String>,
    pub source_range: SourceRange,
    pub content: String,
    pub content_hash: String,
    pub token_count: usize,
    pub inclusion_reason: String,
    pub score: f64,
    pub related_symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBuildResult {
    pub query: String,
    pub token_budget: usize,
    pub total_tokens_used: usize,
    pub primary_matches: usize,
    pub graph_expanded_items: usize,
    pub snippets: Vec<ContextSnippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatusRequest {
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCancelRequest {
    pub job_id: String,
}

use crate::model::*;
use crate::store::sha256_hex;
use anyhow::{anyhow, Result};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

#[derive(Debug, Clone, Default)]
pub struct ParsedFile {
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    pub relationships: Vec<Relationship>,
    pub chunks: Vec<CodeChunk>,
}

pub fn detect_language(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some("rust"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("jsx"),
        "py" | "pyi" => Some("python"),
        "go" => Some("go"),
        _ => None,
    }
}

pub fn get_tree_sitter_language(lang: &str) -> Result<Language> {
    match lang {
        "rust" => Ok(Language::from(tree_sitter_rust::LANGUAGE)),
        "typescript" => Ok(Language::from(tree_sitter_typescript::LANGUAGE_TYPESCRIPT)),
        "tsx" => Ok(Language::from(tree_sitter_typescript::LANGUAGE_TSX)),
        "javascript" | "jsx" => Ok(Language::from(tree_sitter_javascript::LANGUAGE)),
        "python" => Ok(Language::from(tree_sitter_python::LANGUAGE)),
        "go" => Ok(Language::from(tree_sitter_go::LANGUAGE)),
        other => Err(anyhow!("Unsupported language for Tree-sitter: {}", other)),
    }
}

fn node_range(node: Node<'_>) -> SourceRange {
    let sp = node.start_position();
    let ep = node.end_position();
    SourceRange {
        start_line: sp.row + 1,
        start_column: sp.column + 1,
        end_line: ep.row + 1,
        end_column: ep.column + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    }
}

fn node_text<'a>(node: Node<'_>, src: &'a str) -> &'a str {
    let sb = node.start_byte().min(src.len());
    let eb = node.end_byte().min(src.len());
    if sb <= eb {
        &src[sb..eb]
    } else {
        ""
    }
}

fn extract_signature(node: Node<'_>, src: &str) -> String {
    let raw = node_text(node, src);
    let first_line = raw.lines().next().unwrap_or("").trim();
    let cleaned = first_line
        .trim_end_matches('{')
        .trim_end_matches(':')
        .trim();
    if cleaned.len() > 200 {
        format!("{}...", &cleaned[..200])
    } else {
        cleaned.to_string()
    }
}

fn extract_preceding_comments(node: Node<'_>, src: &str, lines: &[&str]) -> Option<String> {
    let mut comments = Vec::new();
    let mut prev = node.prev_sibling();
    let mut expected_line = node.start_position().row;

    while let Some(sib) = prev {
        let kind = sib.kind();
        if kind.contains("comment") {
            let sib_end_row = sib.end_position().row;
            if expected_line <= sib_end_row + 1 {
                let text = node_text(sib, src).trim();
                let cleaned = text
                    .trim_start_matches("///")
                    .trim_start_matches("//!")
                    .trim_start_matches("//")
                    .trim_start_matches("/**")
                    .trim_start_matches("/*")
                    .trim_end_matches("*/")
                    .trim_start_matches('#')
                    .trim();
                if !cleaned.is_empty() {
                    comments.push(cleaned.to_string());
                }
                expected_line = sib.start_position().row;
                prev = sib.prev_sibling();
                continue;
            }
        } else if kind == "attribute_item" || kind == "decorator" {
            expected_line = sib.start_position().row;
            prev = sib.prev_sibling();
            continue;
        }
        break;
    }

    if !comments.is_empty() {
        comments.reverse();
        return Some(comments.join("\n"));
    }

    // Fallback: inspect line immediately above in source
    let start_row = node.start_position().row;
    if start_row > 0 && start_row <= lines.len() {
        let mut idx = start_row;
        while idx > 0 {
            idx -= 1;
            let l = lines[idx].trim();
            if l.starts_with("///")
                || l.starts_with("//")
                || l.starts_with('#')
                || l.starts_with("/*")
                || l.starts_with('*')
            {
                let c = l
                    .trim_start_matches("///")
                    .trim_start_matches("//")
                    .trim_start_matches("/**")
                    .trim_start_matches("/*")
                    .trim_end_matches("*/")
                    .trim_start_matches('*')
                    .trim_start_matches('#')
                    .trim();
                if !c.is_empty() {
                    comments.push(c.to_string());
                }
            } else if l.starts_with("#[") || l.starts_with('@') {
                continue;
            } else {
                break;
            }
        }
        if !comments.is_empty() {
            comments.reverse();
            return Some(comments.join("\n"));
        }
    }

    // Python docstring check inside body block
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "expression_statement" {
                if let Some(first) = child.child(0) {
                    if first.kind() == "string" {
                        let raw_doc = node_text(first, src)
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .trim();
                        if !raw_doc.is_empty() {
                            return Some(raw_doc.to_string());
                        }
                    }
                }
            }
            if !child.kind().contains("comment") {
                break;
            }
        }
    }

    None
}

fn child_ident_name(node: Node<'_>, src: &str) -> Option<String> {
    if let Some(name_node) = node.child_by_field_name("name") {
        let t = node_text(name_node, src).trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "type_identifier" | "field_identifier" | "property_identifier" => {
                let t = node_text(child, src).trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

pub fn parse_file_ast(
    snapshot_id: &str,
    workspace_id: &str,
    repository_id: &str,
    file_path: &str,
    language: &str,
    content: &str,
    file_hash: &str,
) -> Result<ParsedFile> {
    let ts_lang = get_tree_sitter_language(language)?;
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .map_err(|e| anyhow!("Failed to set Tree-sitter language {}: {}", language, e))?;

    let tree = parser
        .parse(content, None)
        .ok_or_else(|| anyhow!("Tree-sitter failed to parse {}", file_path))?;

    let lines: Vec<&str> = content.lines().collect();
    let mut out = ParsedFile::default();

    let mut ctx = WalkContext {
        snapshot_id,
        workspace_id,
        repository_id,
        file_path,
        language,
        content,
        file_hash,
        lines: &lines,
        out: &mut out,
    };

    ctx.walk_node(tree.root_node(), None, None);
    ctx.build_chunks();

    Ok(out)
}

struct WalkContext<'a> {
    snapshot_id: &'a str,
    workspace_id: &'a str,
    repository_id: &'a str,
    file_path: &'a str,
    language: &'a str,
    content: &'a str,
    file_hash: &'a str,
    lines: &'a [&'a str],
    out: &'a mut ParsedFile,
}

impl<'a> WalkContext<'a> {
    fn walk_node(
        &mut self,
        node: Node<'_>,
        parent_sym_id: Option<&str>,
        parent_sym_name: Option<&str>,
    ) {
        let kind = node.kind();
        let mut current_sym_id: Option<String> = parent_sym_id.map(|s| s.to_string());
        let mut current_sym_name: Option<String> = parent_sym_name.map(|s| s.to_string());

        match self.language {
            "rust" => {
                self.extract_rust_node(
                    node,
                    kind,
                    parent_sym_id,
                    parent_sym_name,
                    &mut current_sym_id,
                    &mut current_sym_name,
                );
            }
            "typescript" | "tsx" | "javascript" | "jsx" => {
                self.extract_ts_js_node(
                    node,
                    kind,
                    parent_sym_id,
                    parent_sym_name,
                    &mut current_sym_id,
                    &mut current_sym_name,
                );
            }
            "python" => {
                self.extract_python_node(
                    node,
                    kind,
                    parent_sym_id,
                    parent_sym_name,
                    &mut current_sym_id,
                    &mut current_sym_name,
                );
            }
            "go" => {
                self.extract_go_node(
                    node,
                    kind,
                    parent_sym_id,
                    parent_sym_name,
                    &mut current_sym_id,
                    &mut current_sym_name,
                );
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk_node(
                child,
                current_sym_id.as_deref(),
                current_sym_name.as_deref(),
            );
        }
    }

    fn record_symbol(
        &mut self,
        node: Node<'_>,
        name: String,
        kind: SymbolKind,
        parent_sym_id: Option<&str>,
        parent_sym_name: Option<&str>,
        visibility: Option<String>,
    ) -> (String, String) {
        let range = node_range(node);
        let sep = if self.language == "rust" { "::" } else { "." };
        let qualified_name = match parent_sym_name {
            Some(p) if !p.is_empty() => format!("{}{}{}", p, sep, name),
            _ => name.clone(),
        };
        let raw_id = format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.snapshot_id,
            self.file_path,
            qualified_name,
            kind.as_str(),
            range.start_line,
            range.start_byte,
            range.end_byte
        );
        let symbol_id = format!("sym-{}", &sha256_hex(raw_id.as_bytes())[..16]);
        let body_text = node_text(node, self.content);
        let sym_hash = sha256_hex(body_text.as_bytes());
        let signature = extract_signature(node, self.content);
        let doc_comment = extract_preceding_comments(node, self.content, self.lines);

        let sym = Symbol {
            symbol_id: symbol_id.clone(),
            snapshot_id: self.snapshot_id.to_string(),
            workspace_id: self.workspace_id.to_string(),
            repository_id: self.repository_id.to_string(),
            name: name.clone(),
            qualified_name: qualified_name.clone(),
            kind,
            language: self.language.to_string(),
            file_path: self.file_path.to_string(),
            source_range: range,
            signature,
            doc_comment,
            parent_symbol_id: parent_sym_id.map(|s| s.to_string()),
            visibility,
            content_hash: sym_hash.clone(),
        };
        self.out.symbols.push(sym);

        if let Some(pid) = parent_sym_id {
            let pname = parent_sym_name.unwrap_or("<parent>");
            self.record_relationship(
                Some(pid.to_string()),
                pname.to_string(),
                Some(symbol_id.clone()),
                name,
                RelationshipKind::Contains,
                range,
                ResolutionKind::Heuristic,
            );
        }

        (symbol_id, qualified_name)
    }

    fn record_reference(
        &mut self,
        node: Node<'_>,
        symbol_name: String,
        reference_kind: &str,
        enclosing_sym_id: Option<&str>,
    ) {
        if symbol_name.is_empty() || is_builtin_noise(&symbol_name) {
            return;
        }
        let range = node_range(node);
        let line_idx = range.start_line.saturating_sub(1);
        let context_line = self
            .lines
            .get(line_idx)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let raw_id = format!(
            "{}:{}:{}:{}:{}:{}:{}:{}",
            self.snapshot_id,
            self.file_path,
            symbol_name,
            reference_kind,
            range.start_line,
            range.start_column,
            range.start_byte,
            range.end_byte
        );
        let reference_id = format!("ref-{}", &sha256_hex(raw_id.as_bytes())[..16]);

        self.out.references.push(Reference {
            reference_id,
            snapshot_id: self.snapshot_id.to_string(),
            workspace_id: self.workspace_id.to_string(),
            symbol_name,
            target_symbol_id: None,
            file_path: self.file_path.to_string(),
            source_range: range,
            context_line,
            reference_kind: reference_kind.to_string(),
            enclosing_symbol_id: enclosing_sym_id.map(|s| s.to_string()),
            resolution_kind: ResolutionKind::Heuristic,
            content_hash: self.file_hash.to_string(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn record_relationship(
        &mut self,
        from_symbol_id: Option<String>,
        from_name: String,
        to_symbol_id: Option<String>,
        to_name: String,
        kind: RelationshipKind,
        range: SourceRange,
        resolution_kind: ResolutionKind,
    ) {
        if to_name.is_empty() {
            return;
        }
        let raw_id = format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            self.snapshot_id,
            self.file_path,
            from_name,
            to_name,
            kind.as_str(),
            range.start_line,
            range.start_column,
            range.start_byte,
            range.end_byte
        );
        let relationship_id = format!("rel-{}", &sha256_hex(raw_id.as_bytes())[..16]);

        self.out.relationships.push(Relationship {
            relationship_id,
            snapshot_id: self.snapshot_id.to_string(),
            workspace_id: self.workspace_id.to_string(),
            from_symbol_id,
            from_name,
            from_file_path: self.file_path.to_string(),
            to_symbol_id,
            to_name,
            to_file_path: None,
            kind,
            source_range: range,
            resolution_kind,
            content_hash: self.file_hash.to_string(),
            indexing_config: format!("tree-sitter:{}", self.language),
            limitations: vec![
                "Syntactic Tree-sitter extraction; dynamic dispatch and overloaded names use heuristic matching."
                    .to_string(),
            ],
        });
    }

    // ========================================================================
    // Rust Extractor
    // ========================================================================
    fn extract_rust_node(
        &mut self,
        node: Node<'_>,
        kind: &str,
        parent_sym_id: Option<&str>,
        parent_sym_name: Option<&str>,
        current_sym_id: &mut Option<String>,
        current_sym_name: &mut Option<String>,
    ) {
        let vis = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "visibility_modifier")
            .map(|c| node_text(c, self.content).to_string());

        match kind {
            "function_item" | "function_signature_item" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let sym_kind = if parent_sym_id.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        sym_kind,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "struct_item" | "enum_item" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        SymbolKind::Struct,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "trait_item" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        SymbolKind::Trait,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "mod_item" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        SymbolKind::Module,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "impl_item" => {
                let type_node = node.child_by_field_name("type");
                let trait_node = node.child_by_field_name("trait");
                if let Some(tn) = type_node {
                    let type_name = extract_last_identifier(node_text(tn, self.content));
                    if !type_name.is_empty() {
                        *current_sym_name = Some(type_name.clone());
                        // Link to existing struct symbol if defined earlier in file
                        if let Some(existing) =
                            self.out.symbols.iter().find(|s| s.name == type_name)
                        {
                            *current_sym_id = Some(existing.symbol_id.clone());
                        }
                        if let Some(tr) = trait_node {
                            let trait_name = extract_last_identifier(node_text(tr, self.content));
                            if !trait_name.is_empty() {
                                self.record_reference(
                                    tr,
                                    trait_name.clone(),
                                    "inheritance",
                                    current_sym_id.as_deref(),
                                );
                                self.record_relationship(
                                    current_sym_id.clone(),
                                    type_name,
                                    None,
                                    trait_name,
                                    RelationshipKind::Implements,
                                    node_range(node),
                                    ResolutionKind::Heuristic,
                                );
                            }
                        }
                    }
                }
            }
            "use_declaration" => {
                let raw = node_text(node, self.content)
                    .trim()
                    .trim_start_matches("pub ")
                    .trim_start_matches("use ")
                    .trim_end_matches(';')
                    .trim();
                let target = extract_last_identifier(raw);
                if !target.is_empty() {
                    self.record_reference(node, target.clone(), "import", parent_sym_id);
                    self.record_relationship(
                        parent_sym_id.map(|s| s.to_string()),
                        parent_sym_name.unwrap_or(self.file_path).to_string(),
                        None,
                        raw.to_string(),
                        RelationshipKind::Imports,
                        node_range(node),
                        ResolutionKind::Heuristic,
                    );
                }
            }
            "call_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let raw_callee = node_text(func_node, self.content).trim();
                    let callee = extract_last_identifier(raw_callee);
                    if !callee.is_empty() && !is_builtin_noise(&callee) {
                        self.record_reference(func_node, callee.clone(), "call", parent_sym_id);
                        self.record_relationship(
                            parent_sym_id.map(|s| s.to_string()),
                            parent_sym_name.unwrap_or(self.file_path).to_string(),
                            None,
                            callee,
                            RelationshipKind::Calls,
                            node_range(node),
                            ResolutionKind::Heuristic,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // ========================================================================
    // TypeScript / JavaScript Extractor
    // ========================================================================
    fn extract_ts_js_node(
        &mut self,
        node: Node<'_>,
        kind: &str,
        parent_sym_id: Option<&str>,
        parent_sym_name: Option<&str>,
        current_sym_id: &mut Option<String>,
        current_sym_name: &mut Option<String>,
    ) {
        let is_exported = node
            .parent()
            .map(|p| p.kind() == "export_statement")
            .unwrap_or(false);
        let vis = if is_exported {
            Some("export".to_string())
        } else {
            None
        };

        match kind {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        SymbolKind::Function,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "variable_declarator" => {
                if let (Some(name_node), Some(val_node)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("value"),
                ) {
                    let vk = val_node.kind();
                    if vk == "arrow_function" || vk == "function_expression" || vk == "function" {
                        let name = node_text(name_node, self.content).trim().to_string();
                        if !name.is_empty() {
                            let (id, qname) = self.record_symbol(
                                node,
                                name,
                                SymbolKind::Function,
                                parent_sym_id,
                                parent_sym_name,
                                vis,
                            );
                            *current_sym_id = Some(id);
                            *current_sym_name = Some(qname);
                        }
                    }
                }
            }
            "class_declaration" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name.clone(),
                        SymbolKind::Class,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id.clone());
                    *current_sym_name = Some(qname);

                    // Extract extends / implements clauses
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        if child.kind() == "class_heritage"
                            || child.kind() == "extends_clause"
                            || child.kind() == "implements_clause"
                        {
                            self.extract_ts_heritage(child, &id, &name);
                        }
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name.clone(),
                        SymbolKind::Interface,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id.clone());
                    *current_sym_name = Some(qname);

                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        if child.kind().contains("extends") {
                            self.extract_ts_heritage(child, &id, &name);
                        }
                    }
                }
            }
            "method_definition" | "method_signature" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    if name != "constructor" {
                        let (id, qname) = self.record_symbol(
                            node,
                            name,
                            SymbolKind::Method,
                            parent_sym_id,
                            parent_sym_name,
                            vis,
                        );
                        *current_sym_id = Some(id);
                        *current_sym_name = Some(qname);
                    }
                }
            }
            "internal_module" | "module" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        SymbolKind::Module,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "import_statement" => {
                if let Some(src_node) = node.child_by_field_name("source") {
                    let mod_spec = node_text(src_node, self.content)
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    if !mod_spec.is_empty() {
                        self.record_reference(src_node, mod_spec.clone(), "import", parent_sym_id);
                        self.record_relationship(
                            parent_sym_id.map(|s| s.to_string()),
                            parent_sym_name.unwrap_or(self.file_path).to_string(),
                            None,
                            mod_spec,
                            RelationshipKind::Imports,
                            node_range(node),
                            ResolutionKind::Heuristic,
                        );
                    }
                }
            }
            "call_expression" | "new_expression" => {
                let target_node = node
                    .child_by_field_name("function")
                    .or_else(|| node.child_by_field_name("constructor"));
                if let Some(fn_node) = target_node {
                    let raw = node_text(fn_node, self.content).trim();
                    let callee = extract_last_identifier(raw);
                    if !callee.is_empty() && !is_builtin_noise(&callee) {
                        self.record_reference(fn_node, callee.clone(), "call", parent_sym_id);
                        self.record_relationship(
                            parent_sym_id.map(|s| s.to_string()),
                            parent_sym_name.unwrap_or(self.file_path).to_string(),
                            None,
                            callee,
                            RelationshipKind::Calls,
                            node_range(node),
                            ResolutionKind::Heuristic,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn extract_ts_heritage(&mut self, heritage_node: Node<'_>, class_id: &str, class_name: &str) {
        let text = node_text(heritage_node, self.content);
        for token in text
            .replace(',', " ")
            .replace('<', " ")
            .replace('>', " ")
            .split_whitespace()
        {
            if token == "extends" || token == "implements" {
                continue;
            }
            let ident = extract_last_identifier(token);
            if !ident.is_empty() {
                let rel_kind = if text.contains("implements") {
                    RelationshipKind::Implements
                } else {
                    RelationshipKind::Inherits
                };
                self.record_reference(heritage_node, ident.clone(), "inheritance", Some(class_id));
                self.record_relationship(
                    Some(class_id.to_string()),
                    class_name.to_string(),
                    None,
                    ident,
                    rel_kind,
                    node_range(heritage_node),
                    ResolutionKind::Heuristic,
                );
            }
        }
    }

    // ========================================================================
    // Python Extractor
    // ========================================================================
    fn extract_python_node(
        &mut self,
        node: Node<'_>,
        kind: &str,
        parent_sym_id: Option<&str>,
        parent_sym_name: Option<&str>,
        current_sym_id: &mut Option<String>,
        current_sym_name: &mut Option<String>,
    ) {
        match kind {
            "function_definition" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let sym_kind = if parent_sym_id.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    let vis = if name.starts_with('_') && !name.starts_with("__") {
                        Some("private".to_string())
                    } else {
                        Some("public".to_string())
                    };
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        sym_kind,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "class_definition" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let (id, qname) = self.record_symbol(
                        node,
                        name.clone(),
                        SymbolKind::Class,
                        parent_sym_id,
                        parent_sym_name,
                        Some("public".to_string()),
                    );
                    *current_sym_id = Some(id.clone());
                    *current_sym_name = Some(qname);

                    if let Some(superclasses) = node.child_by_field_name("superclasses") {
                        let mut cursor = superclasses.walk();
                        for child in superclasses.children(&mut cursor) {
                            if child.kind() == "identifier" || child.kind() == "attribute" {
                                let base_name =
                                    extract_last_identifier(node_text(child, self.content));
                                if !base_name.is_empty() && base_name != "object" {
                                    self.record_reference(
                                        child,
                                        base_name.clone(),
                                        "inheritance",
                                        Some(&id),
                                    );
                                    self.record_relationship(
                                        Some(id.clone()),
                                        name.clone(),
                                        None,
                                        base_name,
                                        RelationshipKind::Implements,
                                        node_range(child),
                                        ResolutionKind::Heuristic,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            "import_statement" | "import_from_statement" => {
                let raw = node_text(node, self.content).trim();
                let target = if let Some(mod_node) = node.child_by_field_name("module_name") {
                    node_text(mod_node, self.content).trim().to_string()
                } else {
                    raw.trim_start_matches("import ").trim().to_string()
                };
                if !target.is_empty() {
                    self.record_reference(node, target.clone(), "import", parent_sym_id);
                    self.record_relationship(
                        parent_sym_id.map(|s| s.to_string()),
                        parent_sym_name.unwrap_or(self.file_path).to_string(),
                        None,
                        target,
                        RelationshipKind::Imports,
                        node_range(node),
                        ResolutionKind::Heuristic,
                    );
                }
            }
            "call" => {
                if let Some(fn_node) = node.child_by_field_name("function") {
                    let raw = node_text(fn_node, self.content).trim();
                    let callee = extract_last_identifier(raw);
                    if !callee.is_empty() && !is_builtin_noise(&callee) {
                        self.record_reference(fn_node, callee.clone(), "call", parent_sym_id);
                        self.record_relationship(
                            parent_sym_id.map(|s| s.to_string()),
                            parent_sym_name.unwrap_or(self.file_path).to_string(),
                            None,
                            callee,
                            RelationshipKind::Calls,
                            node_range(node),
                            ResolutionKind::Heuristic,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // ========================================================================
    // Go Extractor
    // ========================================================================
    fn extract_go_node(
        &mut self,
        node: Node<'_>,
        kind: &str,
        parent_sym_id: Option<&str>,
        parent_sym_name: Option<&str>,
        current_sym_id: &mut Option<String>,
        current_sym_name: &mut Option<String>,
    ) {
        match kind {
            "package_clause" => {
                let raw = node_text(node, self.content)
                    .trim()
                    .trim_start_matches("package ")
                    .trim()
                    .to_string();
                if !raw.is_empty() {
                    self.record_symbol(
                        node,
                        raw,
                        SymbolKind::Module,
                        None,
                        None,
                        Some("package".to_string()),
                    );
                }
            }
            "function_declaration" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let vis = if name
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_uppercase())
                        .unwrap_or(false)
                    {
                        Some("exported".to_string())
                    } else {
                        Some("package".to_string())
                    };
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        SymbolKind::Function,
                        parent_sym_id,
                        parent_sym_name,
                        vis,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "method_declaration" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let recv_type = node
                        .child_by_field_name("receiver")
                        .map(|r| extract_last_identifier(node_text(r, self.content)))
                        .filter(|s| !s.is_empty());
                    let recv_sym_id = recv_type.as_ref().and_then(|rt| {
                        self.out
                            .symbols
                            .iter()
                            .find(|s| &s.name == rt)
                            .map(|s| s.symbol_id.clone())
                    });
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        SymbolKind::Method,
                        recv_sym_id.as_deref().or(parent_sym_id),
                        recv_type.as_deref().or(parent_sym_name),
                        None,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "type_spec" => {
                if let Some(name) = child_ident_name(node, self.content) {
                    let type_node = node.child_by_field_name("type");
                    let sym_kind = match type_node.map(|n| n.kind()) {
                        Some("interface_type") => SymbolKind::Interface,
                        _ => SymbolKind::Struct,
                    };
                    let (id, qname) = self.record_symbol(
                        node,
                        name,
                        sym_kind,
                        parent_sym_id,
                        parent_sym_name,
                        None,
                    );
                    *current_sym_id = Some(id);
                    *current_sym_name = Some(qname);
                }
            }
            "import_spec" => {
                if let Some(path_node) = node.child_by_field_name("path") {
                    let imp = node_text(path_node, self.content)
                        .trim()
                        .trim_matches('"')
                        .trim_matches('`')
                        .to_string();
                    if !imp.is_empty() {
                        self.record_reference(path_node, imp.clone(), "import", parent_sym_id);
                        self.record_relationship(
                            parent_sym_id.map(|s| s.to_string()),
                            parent_sym_name.unwrap_or(self.file_path).to_string(),
                            None,
                            imp,
                            RelationshipKind::Imports,
                            node_range(node),
                            ResolutionKind::Heuristic,
                        );
                    }
                }
            }
            "call_expression" => {
                if let Some(fn_node) = node.child_by_field_name("function") {
                    let raw = node_text(fn_node, self.content).trim();
                    let callee = extract_last_identifier(raw);
                    if !callee.is_empty() && !is_builtin_noise(&callee) {
                        self.record_reference(fn_node, callee.clone(), "call", parent_sym_id);
                        self.record_relationship(
                            parent_sym_id.map(|s| s.to_string()),
                            parent_sym_name.unwrap_or(self.file_path).to_string(),
                            None,
                            callee,
                            RelationshipKind::Calls,
                            node_range(node),
                            ResolutionKind::Heuristic,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn build_chunks(&mut self) {
        // 1. One chunk per symbol for direct symbol-backed retrieval
        for sym in &self.out.symbols {
            let start_idx = sym.source_range.start_line.saturating_sub(1);
            let end_idx = sym.source_range.end_line.min(self.lines.len());
            if start_idx >= end_idx {
                continue;
            }
            let chunk_content = self.lines[start_idx..end_idx].join("\n");
            let chunk_hash = sha256_hex(chunk_content.as_bytes());
            let raw_id = format!(
                "{}:{}:sym:{}",
                self.snapshot_id, self.file_path, sym.symbol_id
            );
            let chunk_id = format!("chk-{}", &sha256_hex(raw_id.as_bytes())[..16]);

            self.out.chunks.push(CodeChunk {
                chunk_id,
                snapshot_id: self.snapshot_id.to_string(),
                workspace_id: self.workspace_id.to_string(),
                file_path: self.file_path.to_string(),
                language: self.language.to_string(),
                symbol_id: Some(sym.symbol_id.clone()),
                symbol_name: Some(sym.qualified_name.clone()),
                source_range: sym.source_range,
                token_estimate: estimate_tokens(&chunk_content),
                content: chunk_content,
                content_hash: chunk_hash,
            });
        }

        // 2. If the file has no symbols, or top-level lines outside symbols, also create a file-level chunk
        if self.out.chunks.is_empty() && !self.content.trim().is_empty() {
            let raw_id = format!("{}:{}:file", self.snapshot_id, self.file_path);
            let chunk_id = format!("chk-{}", &sha256_hex(raw_id.as_bytes())[..16]);
            let end_line = self.lines.len().max(1);
            let end_col = self.lines.last().map(|l| l.len() + 1).unwrap_or(1);
            self.out.chunks.push(CodeChunk {
                chunk_id,
                snapshot_id: self.snapshot_id.to_string(),
                workspace_id: self.workspace_id.to_string(),
                file_path: self.file_path.to_string(),
                language: self.language.to_string(),
                symbol_id: None,
                symbol_name: None,
                source_range: SourceRange {
                    start_line: 1,
                    start_column: 1,
                    end_line,
                    end_column: end_col,
                    start_byte: 0,
                    end_byte: self.content.len(),
                },
                token_estimate: estimate_tokens(self.content),
                content: self.content.to_string(),
                content_hash: self.file_hash.to_string(),
            });
        }
    }
}

fn extract_last_identifier(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .trim_end_matches('!')
        .trim_end_matches('?')
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':' && c != '.');
    let last = cleaned
        .rsplit(|c| c == ':' || c == '.' || c == '/')
        .next()
        .unwrap_or(cleaned);
    last.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

fn is_builtin_noise(ident: &str) -> bool {
    matches!(
        ident,
        "if" | "for"
            | "while"
            | "match"
            | "return"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
            | "Box"
            | "Vec"
            | "String"
            | "clone"
            | "into"
            | "from"
            | "unwrap"
            | "expect"
            | "len"
            | "is_empty"
            | "iter"
            | "map"
            | "filter"
            | "collect"
            | "push"
            | "println"
            | "format"
            | "print"
            | "range"
            | "super"
            | "self"
            | "this"
    )
}

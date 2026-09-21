//! `compass-lang-json` — the JSON **catalog** extractor (ADR-0007).
//!
//! Like CSV, a JSON file is indexed for *what things are called*, never for its values:
//!
//! - **API descriptions** announce themselves with a top-level key, so they are recognised by
//!   content, not by file name: OpenAPI / Swagger (`openapi`, `swagger`), JSON Schema
//!   (`$schema`, or `properties` / `$defs`) and Postman collections (`info.schema`). Their
//!   endpoints (`GET /users/{id}`), operation ids, schema names and schema properties
//!   (`User.email`) become symbols — exactly the names an assistant otherwise invents. See
//!   [`describe`].
//! - **Any other JSON** is treated as data: the keys of its records. For an array of objects
//!   that is the first object's keys; for an object, its own keys plus those of the first
//!   record in any array it holds (`{"customers": [{…}]}`).
//!
//! Files are in the `data` category: out of every dependency metric, hidden in the map until
//! switched on. JSON has no imports, so nothing is ever resolved or reported broken.

mod describe;
mod node;

use std::collections::HashSet;

use compass_core::{FileCategory, LanguageId, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

use node::{pairs, span_of};

/// Most keys one file contributes. A record with more fields than this is a machine dump whose
/// names nobody looks up one by one.
const MAX_SYMBOLS: usize = 200;

/// The JSON extractor. Registered by the CLI composition root (ADR-0003).
pub struct JsonExtractor;

impl Extractor for JsonExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("json")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["json"],
            shebangs: &[],
        }
    }

    fn category(&self) -> FileCategory {
        FileCategory::new("data")
    }

    fn grammar(&self) -> Language {
        tree_sitter_json::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut out = Symbols::default();
        if let Some(root) = tree.root_node().named_child(0) {
            let described = root.kind() == "object" && describe::describe(root, source, &mut out);
            if !described {
                record_keys(root, source, &mut out);
            }
        }
        Extraction {
            symbols: out.symbols,
            ..Extraction::default()
        }
    }

    /// JSON references nothing.
    fn resolve(
        &self,
        _imports: &[RawImport],
        _ctx: &dyn ResolutionContext,
        _config: &LangConfig,
    ) -> Vec<ResolvedImport> {
        Vec::new()
    }
}

/// The symbols of one file: each name once, capped at [`MAX_SYMBOLS`].
#[derive(Default)]
struct Symbols {
    symbols: Vec<ExtractedSymbol>,
    seen: HashSet<String>,
}

impl Symbols {
    /// Add `name`, positioned at `at`. Blank and repeated names are dropped.
    fn push(&mut self, name: String, kind: SymbolKind, at: Node) {
        if name.trim().is_empty() || self.symbols.len() >= MAX_SYMBOLS {
            return;
        }
        if self.seen.insert(name.clone()) {
            self.symbols.push(ExtractedSymbol {
                name,
                kind,
                span: span_of(at),
            });
        }
    }
}

/// Generic data: the keys of the records in `root`.
fn record_keys(root: Node, src: &[u8], out: &mut Symbols) {
    match root.kind() {
        "array" => {
            if let Some(record) = first_object(root) {
                push_keys(record, src, out);
            }
        }
        "object" => {
            push_keys(root, src, out);
            // `{"customers": [{…}, …]}` — the records are one level down.
            for (_, _, value) in pairs(root, src) {
                if let Some(record) = first_object(value) {
                    push_keys(record, src, out);
                }
            }
        }
        _ => {}
    }
}

fn push_keys(object: Node, src: &[u8], out: &mut Symbols) {
    for (key, key_node, _) in pairs(object, src) {
        out.push(key, SymbolKind::Field, key_node);
    }
}

/// The first object element of `array` (its first record), if `array` is one and has any.
fn first_object(array: Node) -> Option<Node> {
    if array.kind() != "array" {
        return None;
    }
    let mut i = 0usize;
    while i < array.named_child_count() {
        let element = array.named_child(i as u32)?;
        if element.kind() == "object" {
            return Some(element);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use compass_core::SymbolKind::{Field, Function, Struct};

    fn symbols(json: &str) -> Vec<(String, SymbolKind)> {
        let x = JsonExtractor;
        let tree = compass_extract::parse(&x.grammar(), json.as_bytes()).expect("parse");
        x.extract(json.as_bytes(), &tree)
            .symbols
            .into_iter()
            .map(|s| (s.name, s.kind))
            .collect()
    }

    fn names(json: &str) -> Vec<String> {
        symbols(json).into_iter().map(|(n, _)| n).collect()
    }

    #[test]
    fn an_array_of_records_yields_the_first_records_keys() {
        assert_eq!(
            names(r#"[{"Customer_Id": 1, "full name": "Ann"}, {"Customer_Id": 2, "extra": true}]"#),
            ["Customer_Id", "full name"]
        );
        // Values are never symbols, whatever they are.
        assert!(names(r#"[1, 2, "Customer_Id"]"#).is_empty());
    }

    #[test]
    fn an_object_yields_its_keys_and_those_of_the_records_it_holds() {
        assert_eq!(
            names(
                r#"{"total": 2, "customers": [{"Customer_Id": 1, "email": "a@b.c"}], "meta": {"page": 1}}"#
            ),
            ["total", "customers", "meta", "Customer_Id", "email"]
        );
    }

    #[test]
    fn openapi_endpoints_operation_ids_and_schemas_are_symbols() {
        let spec = r#"{
          "openapi": "3.0.3",
          "info": {"title": "Shop", "version": "1"},
          "paths": {
            "/users/{id}": {
              "parameters": [{"name": "id", "in": "path"}],
              "get": {"operationId": "getUser", "summary": "One user"},
              "delete": {"summary": "no operation id"}
            },
            "/orders": {"post": {"operationId": "createOrder"}}
          },
          "components": {"schemas": {
            "User": {"type": "object", "properties": {"email": {"type": "string"}, "Customer_Id": {"type": "integer"}}},
            "OrderLine": {"type": "object"}
          }}
        }"#;
        assert_eq!(
            symbols(spec),
            [
                ("GET /users/{id}".to_string(), Function),
                ("getUser".to_string(), Function),
                ("DELETE /users/{id}".to_string(), Function),
                ("POST /orders".to_string(), Function),
                ("createOrder".to_string(), Function),
                ("User".to_string(), Struct),
                ("User.email".to_string(), Field),
                ("User.Customer_Id".to_string(), Field),
                ("OrderLine".to_string(), Struct),
            ]
        );
    }

    #[test]
    fn swagger_2_keeps_its_schemas_under_definitions() {
        let spec = r#"{"swagger": "2.0", "paths": {"/ping": {"get": {}}},
                       "definitions": {"Pong": {"properties": {"at": {}}}}}"#;
        assert_eq!(names(spec), ["GET /ping", "Pong", "Pong.at"]);
    }

    #[test]
    fn a_json_schema_yields_its_properties_and_definitions() {
        let schema = r#"{
          "$schema": "https://json-schema.org/draft/2020-12/schema",
          "title": "Customer",
          "type": "object",
          "properties": {"Customer_Id": {"type": "integer"}, "email": {"type": "string"}},
          "$defs": {"Address": {"type": "object", "properties": {"zip": {"type": "string"}}}}
        }"#;
        assert_eq!(
            symbols(schema),
            [
                ("Customer".to_string(), Struct),
                ("Customer_Id".to_string(), Field),
                ("email".to_string(), Field),
                ("Address".to_string(), Struct),
                ("Address.zip".to_string(), Field),
            ]
        );
    }

    #[test]
    fn a_postman_collection_yields_its_requests_through_folders() {
        let collection = r#"{
          "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
          "item": [
            {"name": "Users", "item": [
              {"name": "Get user", "request": {"method": "GET", "url": {"raw": "{{base}}/users/:id"}}}
            ]},
            {"name": "Create order", "request": {"method": "POST", "url": "{{base}}/orders"}}
          ]
        }"#;
        assert_eq!(
            names(collection),
            [
                "GET {{base}}/users/:id",
                "Get user",
                "POST {{base}}/orders",
                "Create order"
            ]
        );
    }

    #[test]
    fn malformed_or_empty_json_yields_nothing_and_never_panics() {
        for json in [
            "",
            "   ",
            "{",
            "[{\"a\":",
            "null",
            "42",
            "\"text\"",
            "{\"paths\": 7, \"openapi\": \"3\"}",
        ] {
            let _ = names(json); // must not panic
        }
        assert!(names("").is_empty());
        assert!(names("42").is_empty());
    }

    #[test]
    fn a_huge_record_is_capped() {
        let fields: Vec<String> = (0..500).map(|i| format!("\"f{i}\": 0")).collect();
        assert_eq!(
            names(&format!("{{{}}}", fields.join(","))).len(),
            MAX_SYMBOLS
        );
    }
}

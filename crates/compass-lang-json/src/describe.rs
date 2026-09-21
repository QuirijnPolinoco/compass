//! Self-describing JSON: formats that say what they are with a top-level key, so they can be
//! recognised from their content alone (ADR-0007 §1) — OpenAPI / Swagger, JSON Schema and
//! Postman collections. What they name is what an assistant otherwise guesses: the endpoints
//! of an API, its operation ids, its schemas and their properties.

use compass_core::SymbolKind;
use tree_sitter::Node;

use crate::node::{get, pairs, string_text};
use crate::Symbols;

/// The HTTP methods an OpenAPI path item may define an operation for. Its other keys
/// (`parameters`, `summary`, `servers`, `$ref`) are not operations.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Index `root` as the self-describing format it declares itself to be. Returns whether it was
/// one; if not, nothing has been added and the caller treats the file as plain data.
pub(crate) fn describe(root: Node, src: &[u8], out: &mut Symbols) -> bool {
    if get(root, "openapi", src).is_some() || get(root, "swagger", src).is_some() {
        openapi(root, src, out);
        return true;
    }
    if is_postman(root, src) {
        postman_items(root, src, out);
        return true;
    }
    let is_schema = get(root, "$schema", src).is_some()
        || get(root, "$defs", src).is_some()
        || (get(root, "properties", src).is_some() && get(root, "type", src).is_some());
    if is_schema {
        json_schema(root, src, out);
        return true;
    }
    false
}

/// `GET /users/{id}` + its `operationId` for every operation, then the named schemas.
fn openapi(root: Node, src: &[u8], out: &mut Symbols) {
    if let Some(paths) = get(root, "paths", src) {
        for (path, _, item) in pairs(paths, src) {
            for (method, method_node, operation) in pairs(item, src) {
                if !HTTP_METHODS.contains(&method.as_str()) {
                    continue;
                }
                let endpoint = format!("{} {path}", method.to_uppercase());
                out.push(endpoint, SymbolKind::Function, method_node);
                // The name generated clients and server stubs give this operation.
                if let Some(id) = get(operation, "operationId", src) {
                    if let Some(name) = string_text(id, src) {
                        out.push(name, SymbolKind::Function, id);
                    }
                }
            }
        }
    }
    // OpenAPI 3 keeps schemas under components.schemas, Swagger 2 under definitions.
    let schemas = get(root, "components", src)
        .and_then(|c| get(c, "schemas", src))
        .or_else(|| get(root, "definitions", src));
    if let Some(schemas) = schemas {
        named_schemas(schemas, src, out);
    }
}

/// A schema document: its own properties, then the schemas it defines.
fn json_schema(root: Node, src: &[u8], out: &mut Symbols) {
    if let Some(title) = get(root, "title", src) {
        if let Some(name) = string_text(title, src) {
            out.push(name, SymbolKind::Struct, title);
        }
    }
    properties(root, None, src, out);
    for key in ["$defs", "definitions"] {
        if let Some(definitions) = get(root, key, src) {
            named_schemas(definitions, src, out);
        }
    }
}

/// `{"User": {…}, "Order": {…}}` → `User`, `User.email`, …, `Order`, ….
fn named_schemas(schemas: Node, src: &[u8], out: &mut Symbols) {
    for (name, name_node, schema) in pairs(schemas, src) {
        out.push(name.clone(), SymbolKind::Struct, name_node);
        properties(schema, Some(&name), src, out);
    }
}

/// A schema's direct `properties`, qualified by the schema's name when it has one — so a
/// search for `email` finds `User.email` and says whose email it is.
fn properties(schema: Node, owner: Option<&str>, src: &[u8], out: &mut Symbols) {
    let Some(properties) = get(schema, "properties", src) else {
        return;
    };
    for (property, property_node, _) in pairs(properties, src) {
        let name = match owner {
            Some(owner) => format!("{owner}.{property}"),
            None => property,
        };
        out.push(name, SymbolKind::Field, property_node);
    }
}

fn is_postman(root: Node, src: &[u8]) -> bool {
    let Some(info) = get(root, "info", src) else {
        return false;
    };
    get(info, "_postman_id", src).is_some()
        || get(info, "schema", src)
            .and_then(|s| string_text(s, src))
            .is_some_and(|s| s.contains("postman"))
}

/// Every request in a collection, through its (nested) folders: `GET {{base}}/users/:id` and
/// the name the team gave it.
fn postman_items(container: Node, src: &[u8], out: &mut Symbols) {
    let Some(items) = get(container, "item", src) else {
        return;
    };
    let mut i = 0usize;
    while i < items.named_child_count() {
        let Some(item) = items.named_child(i as u32) else {
            break;
        };
        if let Some(request) = get(item, "request", src) {
            let method = get(request, "method", src).and_then(|m| string_text(m, src));
            // `url` is a string, or an object whose `raw` is that string.
            let url = get(request, "url", src).and_then(|u| {
                string_text(u, src).or_else(|| get(u, "raw", src).and_then(|r| string_text(r, src)))
            });
            if let (Some(method), Some(url)) = (method, url) {
                out.push(format!("{method} {url}"), SymbolKind::Function, request);
            }
            if let Some(name) = get(item, "name", src) {
                if let Some(text) = string_text(name, src) {
                    out.push(text, SymbolKind::Function, name);
                }
            }
        } else {
            postman_items(item, src, out); // a folder
        }
        i += 1;
    }
}

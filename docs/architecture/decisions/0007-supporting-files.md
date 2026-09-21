# ADR-0007: supporting (non-code) files — two tiers, one category, no generic extensions

- **Status:** Proposed
- **Date:** 2026-09-21
- **Deciders:** Quinn (QuirijnVanDerZanden)

## Context

Compass maps **code**: 14 languages, each a self-contained extractor (ADR-0002) emitting symbols,
imports and calls. Real repositories are more than code. An assistant also has to find *the
right CSV*, know that the customer column is called `Customer_Id` and not `customerId`, find the
endpoint named in an OpenAPI file, or learn that `scripts/deploy.sh` is run by a CI workflow.
Today it gets there by listing directories and opening files one by one — exactly the exploration
Compass exists to remove.

The question: **which non-code file types belong in the map, which never do, and what has to be
true of the model first?** It was analysed family by family (plain data, database artefacts,
build/config/infra, API contracts/templates/docs) and then attacked from three sides (simplicity,
ADR-0002 architecture, daily use). This ADR records the outcome.

Three facts about the current model shaped it:

1. **`Import` is not a neutral label.** Core gives it dependency semantics in at least five
   places: `impact` / guard blast radius, `import_cycles` ("provable" cycles), `isolated_files`,
   hubs / most-connected / context seeding, and community detection. A README linking to
   `main.rs`, or a manifest pointing at an entry file, would count as a *dependent* everywhere.
2. **Call resolution is one global name table across all languages.** A SQL table `users`, a
   GraphQL `type Order` or a CSV column `build` would not only create false `Calls` edges — it
   would silently *delete* correct ones, by turning a unique name into an ambiguous one.
3. **Detection is by extension (and shebang) only, and nothing caps file size.** Almost every
   interesting supporting file is identified by *name* (`package.json`, `Dockerfile`), while the
   extensions they use (`.json`, `.yaml`, `.xml`) are overwhelmingly plain data.

## Decision

### 1. A supporting file earns its place in one of two ways

| Tier | Admission test | What is indexed |
|---|---|---|
| **Relationship** | It references repo files, or code references it, in a statically detectable way | Edges, plus the names an assistant looks up |
| **Catalog** | It holds data or a contract whose **field names** a developer needs, and those names can be read cheaply and reliably | The schema only — never the content |

The catalog tier is what makes *"which file has the customer data, and what is the column
called?"* a single `find_symbol("customer")` call. It is deliberately shallow:

- **CSV / TSV** — the header row. One line is read; a 2 GB file costs the same as a 2 KB one.
- **JSON** — top-level keys, and the keys of the first element when the root is an array of
  records. Depth-limited; values are never indexed.
- **XML** — the root element and the distinct element / attribute names of the first record.
- **Self-describing API descriptions** (the high-value subtype, recognised by a top-level key,
  not by extension): OpenAPI / Swagger (`openapi`, `swagger`) → one symbol per operation
  (`GET /users/{id}`, plus its `operationId`) and per schema with its property names; JSON Schema
  (`$schema`) → definitions and properties; Postman collections → request names and URLs.

### 2. One category, read as one bit

Every extractor declares a category through **one defaulted trait method** (so no existing crate
changes): an open string like `LanguageId` — `code` (default), `markup`, `config`, `data`,
`contract`, … Core interprets exactly **one bit** of it: *code-like* (`code`, `markup`) versus
*supporting*.

Supporting files are, by default:

- **excluded from** blast radius (`impact`, guard), `import_cycles`, the `isolated_files` smell,
  hubs / most-connected, context-pack seeding and the overview's language percentages;
- **never part of call resolution**, which additionally becomes scoped per language;
- reported by `supported_languages` as *file types*, not languages.

This is also the map's **hide/show**: the legend and toggles are generated from the categories
present in the data (nothing hard-coded in the front-end). Supporting categories start hidden;
one click shows them; the choice is remembered.

### 3. Compass never claims a generic data extension for relationships

`.json`, `.yaml`, `.toml`, `.xml`, `.ini` say nothing about meaning. **Relationship** extractors
are therefore detected by **exact file name** (`package.json`, `Dockerfile`,
`docker-compose.yml`) via a second defaulted trait method; a file-name claim beats any extension
claim. No globs, no exclude lists, no `path` argument on `extract()`. Manifest extractors are
**per ecosystem** (`compass-lang-npm`, `compass-lang-cargo`), not per format — a
`compass-lang-json` that knows npm, Composer, Angular and OpenAPI is the shared-crate merge
magnet ADR-0002 exists to prevent.

**Catalog** extractors *do* claim `.csv`, `.tsv`, `.json`, `.xml`, which is safe only because of
§2 (invisible to every code metric, hidden by default) and these guards:

- A **parse-size cap**, applied to every language: above it the file is still a node but is not
  analysed, and says so ("not analysed: too large"). Minified and generated files
  (`*.min.*`, `*.map`, lockfiles) are never analysed — today the vendored `cytoscape.umd.min.js`
  is mapped as a 600-symbol file.
- Files under test-data directories (`tests/`, `fixtures/`, `__snapshots__/`, `testdata/`) join
  the catalog only when a mapped file references them.
- Catalog symbols are namespaced by category, so `find_symbol` can return or exclude them and a
  column can never shadow a function.

### 4. A config that only shapes resolution is an input, never a node

`tsconfig.json`, `go.mod`, `pyproject.toml`, `pom.xml`, Gradle settings: extractors already read
these to resolve imports. As nodes they would be islands nobody links to.

### 5. Literal, file-relative reads of data are edges

`read.csv("data/sales.csv")`, `pd.read_csv("…")`, `include_str!("../schema.sql")`,
`sqlx::query_file!`, `//go:embed` — a string literal naming a file that exists. Compile-time
embeds are `Resolved`; runtime reads are `Heuristic` (the working directory is unknown — tried
from the repo root, then the script's folder, the rule the R extractor already uses for
`source()`). A miss is external, **never** a broken import. Emitted by the *code* language's own
extractor, which needs no knowledge of the target type: it only asks whether the path is mapped.

### 6. Verdicts

| Verdict | File types |
|---|---|
| **Catalog** | CSV/TSV, JSON, XML; OpenAPI/Swagger, JSON Schema, Postman collections |
| **Relationship — first wave** ("what runs or ships this file?") | shell scripts (incl. shebang detection, declared in the contract but never wired), `package.json` (scripts, bin, source-pointing entries), GitHub Actions / GitLab CI (local `uses:` / `include:` / script paths), Dockerfile, docker-compose |
| **Relationship — second wave** (names worth looking up) | GraphQL, SQL designed around **migration** folders (not a single `schema.sql`), Protobuf |
| **Later, own decision** | Markdown (only once §2 keeps doc links out of blast radius), Terraform, Makefile, Cargo / MSBuild manifests, YAML/TOML catalogs, Parquet/Avro/SQLite **schemas** (needs non-tree-sitter readers) |
| **Separate ADR** | Vue / Svelte / Astro single-file components — they are *code*, and need embedded-region support in the engine |
| **Never** | `.env` and anything secret-bearing, lockfiles, binary data bodies and database dumps/seeds, INI/`.properties`/dotfile configs, Kubernetes/Helm/nginx, ORM models (already mapped as code), NoSQL definitions buried in generic JSON/YAML, generic YAML |

### 7. Order of work

Each step is its own PR; nothing in a later step lands before the ones above it.

0. **No new file type:** resolve workspace packages (`@scope/pkg`) in the TypeScript extractor —
   the highest-value item found, because cross-package blast radius is wrong today.
1. **Prerequisites:** language-scoped call resolution → the category bit and its consumers
   (core queries, guard, context, audit, overview, MCP) → file-name detection → the size cap and
   generated-file rule → category toggles in the map.
2. **Catalog tier:** CSV/TSV → JSON with API descriptions → XML; then literal data-read edges
   from R and Python.
3. **Relationship first wave**, then second wave.

## Rationale

- **Two tiers, because there are two real questions.** "What depends on what" and "where is the
  thing called X" are both navigation. Judging CSV by the first question alone wrongly yields
  "never"; judging it by the second yields a feature that costs one line of I/O per file.
- **One bit, not a taxonomy.** Every analysis proposed a different 3–5 value enum — evidence a
  closed enum in core would need a new variant per file type, which ADR-0002 forbids. An open
  string serves the legend; a single derived bit serves every algorithm.
- **Prerequisites before types.** Without the category bit, *any* supporting edge corrupts
  shipped features — a README becomes a god file, CI becomes part of every blast radius. Without
  scoped call resolution, adding symbols *removes* correct call edges. Doing these first makes
  every later type a routine, self-contained crate again.
- **Schema, not content.** Headers and keys are small, stable, and rarely sensitive; bodies are
  none of those. It also keeps the local-first promise honest: field names flow into prompts,
  customer rows never do.

## Consequences

- **Positive:** an assistant can locate data and contracts by field name in one call; the map
  can show — and hide — the whole repository, not just its code; code metrics stay exactly as
  trustworthy as today; each new file type remains one crate.
- **Negative / trade-offs accepted:** two additions to the `Extractor` trait (both defaulted, so
  no existing crate changes, but the "stable interface" grows); catalog extractors for CSV and
  XML headers are not tree-sitter grammars, so `grammar()` needs an opt-out; the size cap changes
  behaviour for very large *code* files too (they lose symbols, visibly); field names of data
  files become visible to the assistant, which a user may not expect — hence `.env` and
  secret-bearing files are excluded outright and the catalog is documented as schema-only.
- **Explicitly rejected:** a `compass-lang-json`/`-yaml` grab-bag crate; content-sniffing to tell
  manifests apart; globs or exclude lists in detection; passing the path into `extract()`;
  a new `References` edge kind (revisit together with CSS-class usage across HTML, which needs
  one anyway); mapping every data file as a bare node with no schema.

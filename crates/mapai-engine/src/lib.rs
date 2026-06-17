//! `mapai-engine` — indexing orchestration. See `docs/architecture/02-architecture.md` §4.
//!
//! Holds the `walk` (gitignore-aware walk plus registry-driven detection), `index`
//! (parse, extract, assemble, resolve), and `cache` (`.mapai/` persistence) modules.
//! The `config` (`.mapai.toml`) and `watch` (live freshness, FR-13) modules arrive post-v1.

pub mod cache;
pub mod index;
pub mod walk;

pub use index::index;

//! `compass-engine` — indexing orchestration. See `docs/architecture/02-architecture.md` §4.
//!
//! Holds the `walk` (gitignore-aware walk plus registry-driven detection), `index`
//! (parse, extract, assemble, resolve), `cache` (`.compass/` persistence), and `watch`
//! (live freshness, FR-13) modules. The `config` (`.compass.toml`) module arrives post-v1.

pub mod cache;
pub mod index;
pub mod walk;
pub mod watch;

pub use index::{index, index_incremental, ExtractionCache};

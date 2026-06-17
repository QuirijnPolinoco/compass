//! The composition root's explicit, `cfg`-gated language registry (ADR-0003).
//!
//! This is the ONE place that names languages. Adding a language means an optional
//! dependency plus a `lang-<name>` feature in `Cargo.toml`, and one `register(...)` line
//! here. No linker-section magic, so a language can never silently vanish from the binary.

use compass_extract::Registry;

/// Build the registry of all compiled-in language extractors.
pub fn register_all() -> Registry {
    #[allow(unused_mut)]
    let mut registry = Registry::new();

    #[cfg(feature = "lang-go")]
    registry.register(Box::new(compass_lang_go::GoExtractor));

    #[cfg(feature = "lang-python")]
    registry.register(Box::new(compass_lang_python::PythonExtractor));

    #[cfg(feature = "lang-java")]
    registry.register(Box::new(compass_lang_java::JavaExtractor));

    #[cfg(feature = "lang-csharp")]
    registry.register(Box::new(compass_lang_csharp::CSharpExtractor));

    #[cfg(feature = "lang-typescript")]
    registry.register(Box::new(compass_lang_typescript::TypeScriptExtractor));

    #[cfg(feature = "lang-rust")]
    registry.register(Box::new(compass_lang_rust::RustExtractor));

    #[cfg(feature = "lang-kotlin")]
    registry.register(Box::new(compass_lang_kotlin::KotlinExtractor));

    #[cfg(feature = "lang-ruby")]
    registry.register(Box::new(compass_lang_ruby::RubyExtractor));

    #[cfg(feature = "lang-php")]
    registry.register(Box::new(compass_lang_php::PhpExtractor));

    #[cfg(feature = "lang-c")]
    registry.register(Box::new(compass_lang_c::CExtractor));

    registry
}

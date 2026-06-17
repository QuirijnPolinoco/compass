# ADR-0003: Extractor registration via an explicit cfg-gated registry

- **Status:** Accepted
- **Date:** 2026-06-17 (from the independent architecture review)
- **Deciders:** Quinn (QuirijnVanDerZanden)

## Context

[ADR-0002](0002-pluggable-language-extractor-architecture.md) chose one crate per language,
compiled in behind cargo features. Something must collect the compiled-in `Extractor`s into
the registry the engine and `supported_languages` (FR-14/H2) read. Because MapAI is a single
static binary, this happens at **compile time**, not via runtime discovery.

The attractive-looking option is **automatic self-registration** via a distributed slice
(`inventory` or `linkme`): each language crate `submit!`s itself and the registry is
assembled with no central edit.

## Options considered

1. **`inventory` self-registration.** Each lang crate registers via a static constructor;
   zero central edit. *Con (decisive):* the linker can **drop** those static constructors
   under dead-code elimination / LTO when a crate is pulled in *only* via a `dep:` feature
   with no directly-referenced symbol — exactly our configuration. The failure is **silent**:
   green in a debug single-language build, but the extractor **vanishes from the release
   binary** (LTO on, multiple languages) — the artifact that actually ships (FR-2/A2) — and
   `supported_languages` silently under-reports.
2. **`linkme` self-registration.** Uses linker sections; more DCE-robust than `inventory`,
   but still requires the crate to be linked and still interacts with LTO. Same class of risk,
   smaller.
3. **Explicit `cfg`-gated `register_all()` in the CLI composition root.** Each lang crate
   exports `pub fn register(reg: &mut Registry)`; the CLI calls it behind the feature cfg:
   ```rust
   fn register_all(reg: &mut Registry) {
       #[cfg(feature = "lang-go")]     mapai_lang_go::register(reg);
       #[cfg(feature = "lang-python")] mapai_lang_python::register(reg);
       // …one line per language…
   }
   ```
   No linker magic; the call is a real, reachable reference, immune to DCE/LTO.

## Decision

Use **Option 3: an explicit `cfg`-gated `register_all()`** in `mapai-cli` as the v1 default.

This keeps the **same single shared edit per language** the design already sanctions (one
feature line + one `register()` line, both in the composition root — neither in core/engine/
mcp), while eliminating the silent-failure mode. Add a CI self-test asserting
`count(registered) == count(enabled features)`.

If `inventory`/`linkme` is ever revisited as an ergonomic optimization, it must first be
validated under the adversarial config: `--release` + `lto = true` + `codegen-units = 1` +
≥2 languages, on linux-musl **and** Windows, with the self-test green.

## Rationale

The mechanism the entire "no core changes" promise rests on must not have a silent,
release-only failure mode. The explicit registry is also **simpler to understand** than
linker-section magic — a real benefit given onboarding friction is the project's #1 risk —
and the headline benefit of self-registration (avoiding one composition-root line) is
marginal at Tier-1 scale and not worth the risk.

## Consequences

- **Positive:** registration is explicit, readable, and DCE/LTO-proof; the release binary
  can't silently lose a language; `supported_languages` stays truthful.
- **Negative / trade-offs accepted:** one extra line in `register_all()` per language (in
  addition to the feature flag) — trivial, and co-located with the feature it mirrors.
- **Follow-ups:** add the `count(registered) == count(enabled features)` CI self-test; keep
  `register_all()` next to the feature list so the two never drift.

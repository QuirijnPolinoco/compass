# Packaging

How Compass is distributed, and the maintainer steps for each channel. All of these consume
the per-platform archives + `.sha256` checksums attached to each GitHub Release by
[`.github/workflows/release.yml`](../.github/workflows/release.yml) (triggered by a `v*` tag).

## Cutting a release

1. Bump `version` in the workspace `Cargo.toml` (and the templates here).
2. Tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. The release workflow builds 5 targets (linux gnu+musl, macOS arm+intel, windows msvc),
   attaches each `compass-<target>.{tar.gz,zip}` and its `.sha256`, and creates the Release.

## Channels

### cargo-binstall (no extra repo needed)

Works straight off the release via the `[package.metadata.binstall]` in
[`crates/compass-cli/Cargo.toml`](../crates/compass-cli/Cargo.toml):

```sh
cargo binstall --git https://github.com/QuirijnPolinoco/compass compass-cli
```

### Homebrew (needs a tap repo)

[`homebrew/compass.rb`](homebrew/compass.rb) is the formula. To publish:

1. Create a tap repo `QuirijnPolinoco/homebrew-tap`.
2. Per release: copy the formula in, set `version`, and paste each `sha256` from the
   release's `compass-<target>.tar.gz.sha256` assets.
3. Users: `brew install QuirijnPolinoco/tap/compass`.

### Scoop (needs a bucket repo)

[`scoop/compass.json`](scoop/compass.json) is the manifest. To publish:

1. Create a bucket repo `QuirijnPolinoco/scoop-bucket`.
2. Per release: set `version` + the `hash` from `compass-x86_64-pc-windows-msvc.zip.sha256`
   (`autoupdate` can automate this going forward).
3. Users: `scoop bucket add compass https://github.com/QuirijnPolinoco/scoop-bucket` then
   `scoop install compass`.

> The `sha256`/`hash` fields are placeholders (`REPLACE_WITH_…`) — fill them from the release
> checksums. Keeping these templates in-repo means the formula/manifest are versioned with the
> code; a future CI step can stamp them automatically on tag.

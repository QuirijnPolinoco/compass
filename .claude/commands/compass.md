---
description: Run the Compass repo-map CLI
argument-hint: init | map | context | overview | deps | broken | watch | serve | languages
allowed-tools: Bash
---
Run the **Compass** repo-map CLI with these arguments: `$ARGUMENTS`
(if no arguments were given, run `overview .`).

Find the binary in this order and use the first that works:
1. `compass` on `PATH`
2. `./target/release/compass` or `./target/debug/compass` (when run inside the Compass repo)
3. `cargo run -p compass-cli --` (from the Compass repo root)

Behavior by subcommand:
- **`map`, `serve`, `watch`** are long-running — start them in the **background** and report
  status without blocking the session. For `map`, surface the `http://127.0.0.1:<port>` URL it
  prints (and that it auto-opens the browser).
- **`init`, `overview`, `context`, `deps`, `broken`, `languages`** are quick — run them in the
  **foreground** and show the output.

Unless the arguments include an explicit path, operate on the current working directory's
repository.

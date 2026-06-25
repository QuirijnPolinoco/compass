//! The single cross-crate end-to-end smoke test: run the real `compass` binary against the
//! Go fixture repo and assert the rendered overview. Proves walk → extract → resolve →
//! graph → query through the actual CLI. Per-language extraction is tested inside each
//! `compass-lang-*` crate; this only checks the wiring.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture")
}

#[test]
fn init_builds_map_and_writes_mcp_config() {
    // Run against a throwaway project so we don't write .mcp.json into the repo.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("init-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("init")
        .arg(&dir)
        .output()
        .expect("run compass init");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        dir.join(".compass/graph.json").exists(),
        "cache not written"
    );
    let mcp = std::fs::read_to_string(dir.join(".mcp.json")).expect(".mcp.json not written");
    assert!(mcp.contains("\"compass\""), ".mcp.json:\n{mcp}");
    assert!(mcp.contains("serve"), ".mcp.json:\n{mcp}");

    // Idempotent: a second run succeeds too.
    let again = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("init")
        .arg(&dir)
        .output()
        .expect("re-run compass init");
    assert!(again.status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

fn fixture_py() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-py")
}

fn fixture_java() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-java")
}

fn fixture_csharp() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-csharp")
}

fn fixture_ts() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-ts")
}

fn fixture_rust() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-rust")
}

fn fixture_kotlin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-kotlin")
}

fn fixture_ruby() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-ruby")
}

fn fixture_php() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-php")
}

fn fixture_c() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-c")
}

fn fixture_broken() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-broken")
}

/// Run an MCP stdio session of newline-delimited JSON-RPC messages and return stdout.
fn mcp_session(dir: PathBuf, messages: &[&str]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("serve")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn compass serve");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        stdin
            .write_all(messages.join("\n").as_bytes())
            .expect("write session");
        stdin.write_all(b"\n").expect("write newline");
    }
    let output = child.wait_with_output().expect("wait for compass serve");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn install_claude_writes_settings_hook_and_mcp() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-claude");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("install")
        .arg("--claude")
        .arg(&dir)
        .output()
        .expect("run compass install --claude");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings =
        std::fs::read_to_string(dir.join(".claude/settings.json")).expect("settings.json written");
    assert!(
        settings.contains("UserPromptSubmit"),
        "settings:\n{settings}"
    );
    assert!(settings.contains("context"), "settings:\n{settings}");
    assert!(settings.contains("--hook"), "settings:\n{settings}");

    assert!(dir.join(".mcp.json").exists(), ".mcp.json not written");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn install_cursor_writes_mcp_server() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-cursor");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("install")
        .arg("--cursor")
        .arg(&dir)
        .output()
        .expect("run compass install --cursor");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mcp = std::fs::read_to_string(dir.join(".cursor/mcp.json")).expect(".cursor/mcp.json");
    assert!(mcp.contains("compass"), "mcp:\n{mcp}");
    assert!(mcp.contains("serve"), "mcp:\n{mcp}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn install_codex_writes_agents_md_idempotently() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-codex");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_compass"))
            .arg("install")
            .arg("--codex")
            .arg(&dir)
            .output()
            .expect("run compass install --codex");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run();
    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).expect("AGENTS.md written");
    assert!(agents.contains("Compass"), "AGENTS.md:\n{agents}");

    // Idempotent: a second run must not append the section twice.
    run();
    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).expect("AGENTS.md");
    assert_eq!(
        agents.matches("## Compass").count(),
        1,
        "expected exactly one `## Compass` section:\n{agents}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn install_claude_preserves_existing_settings() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-claude-merge");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();

    // Pre-write a settings.json with an unrelated key that must survive the merge.
    std::fs::write(
        dir.join(".claude/settings.json"),
        r#"{ "unrelatedKey": "keep-me" }"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("install")
        .arg("--claude")
        .arg(&dir)
        .output()
        .expect("run compass install --claude");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings = std::fs::read_to_string(dir.join(".claude/settings.json")).expect("settings");
    // The pre-existing key survived...
    assert!(settings.contains("unrelatedKey"), "settings:\n{settings}");
    assert!(settings.contains("keep-me"), "settings:\n{settings}");
    // ...and the hook was added.
    assert!(
        settings.contains("UserPromptSubmit"),
        "settings:\n{settings}"
    );
    assert!(settings.contains("--hook"), "settings:\n{settings}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overview_of_go_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 Go files (go.mod is excluded), and the internal import resolved to one edge.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
    assert!(stdout.contains("go"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_c_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_c())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // main.c + util.h; `#include "util.h"` resolves to one edge. (Language id is "c", too
    // short to assert unambiguously — the file/edge counts confirm the C extractor ran.)
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_php_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_php())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 .php files; `require_once 'util.php'` resolves to util.php.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("php"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_ruby_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_ruby())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 .rb files; `require_relative 'util'` resolves to util.rb.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
    assert!(stdout.contains("ruby"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_kotlin_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_kotlin())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 .kt files; `import com.example.util.Helper` resolves under source root `src`.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("kotlin"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_rust_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_rust())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 .rs files; `mod util;` resolves to src/util.rs.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
    assert!(stdout.contains("rust"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_python_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_py())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 3 .py files; `from app.util import helper` resolves to one edge; `os` is external.
    assert!(stdout.contains("files:        3"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
    assert!(stdout.contains("python"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_java_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_java())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 .java files; `import com.example.util.Helper` resolves under source root `src`.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
    assert!(stdout.contains("java"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_csharp_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_csharp())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 .cs files; `using Company.Util` resolves under source root `src`.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
    assert!(stdout.contains("csharp"), "stdout:\n{stdout}");
}

#[test]
fn overview_of_typescript_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("overview")
        .arg(fixture_ts())
        .output()
        .expect("run compass");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 2 .ts files; `import { helper } from "./util"` resolves to one edge.
    assert!(stdout.contains("files:        2"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("typescript"), "stdout:\n{stdout}");
}

#[test]
fn deps_of_go_main() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("deps")
        .arg(fixture())
        .arg("main.go")
        .output()
        .expect("run compass");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "non-zero exit");
    assert!(stdout.contains("-> util/util.go"), "stdout:\n{stdout}");
    assert!(stdout.contains("depended on by (0)"), "stdout:\n{stdout}");
}

#[test]
fn broken_imports_are_reported() {
    let output = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("broken")
        .arg(fixture_broken())
        .output()
        .expect("run compass");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "non-zero exit");
    assert!(stdout.contains("Broken imports (1)"), "stdout:\n{stdout}");
    assert!(stdout.contains("missing"), "stdout:\n{stdout}");
}

#[test]
fn audit_reports_a_circular_import_and_strict_fails() {
    // Two TypeScript files that import each other form a genuine 2-cycle (a→b and b→a). The audit
    // must surface it under "Problems"; by default it still exits 0 (informational), but `--strict`
    // must exit non-zero so CI can gate on real defects.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("audit-cycle");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src").join("a.ts"),
        "import { b } from \"./b\";\nexport function a(): void { b(); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join("b.ts"),
        "import { a } from \"./a\";\nexport function b(): void { a(); }\n",
    )
    .unwrap();

    // Default run: reports the cycle, exits 0 (smells/problems are informational by default).
    let out = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(&dir)
        .output()
        .expect("run compass audit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "default audit must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Circular imports ("),
        "the cycle should be listed under Problems:\n{stdout}"
    );
    assert!(stdout.contains("src/a.ts"), "stdout:\n{stdout}");
    assert!(stdout.contains("src/b.ts"), "stdout:\n{stdout}");
    // The cycle is rendered as a concrete, ordered path (not just an unordered member set), so a
    // reader can see how it closes and which edge to cut.
    assert!(
        stdout.contains("cycle of 2 files"),
        "the cycle size should be named:\n{stdout}"
    );
    assert!(
        stdout.contains("→"),
        "the cycle path should be rendered with arrows:\n{stdout}"
    );
    assert!(
        stdout.contains("Summary: 1 problem"),
        "summary should count the cycle:\n{stdout}"
    );

    // `--strict`: a real problem (the cycle) must make it exit non-zero for CI.
    let strict = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(&dir)
        .arg("--strict")
        .output()
        .expect("run compass audit --strict");
    assert!(
        !strict.status.success(),
        "--strict must exit non-zero when there are real problems:\n{}",
        String::from_utf8_lossy(&strict.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_clean_fixture_reports_no_problems() {
    // The Go fixture (main.go → util/util.go) has no cycles and no broken imports, so the audit
    // reports a clean bill of health and exits 0 even under `--strict`.
    let out = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(fixture())
        .arg("--strict")
        .output()
        .expect("run compass audit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "a clean repo must exit 0 even with --strict; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Circular imports: none"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("Broken imports: none"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("Summary: clean"),
        "a clean repo should say so plainly:\n{stdout}"
    );
}

#[test]
fn audit_empty_repo_is_clean_under_strict() {
    // FOCUS empty-repo case: a 0-file directory must not panic, reports a clean bill of health, and
    // exits 0 even under --strict.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("audit-empty");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(&dir)
        .arg("--strict")
        .output()
        .expect("run compass audit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "an empty repo must exit 0 even with --strict; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("Summary: clean"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("Circular imports: none"),
        "stdout:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_broken_import_is_a_problem_not_an_isolated_smell() {
    // The Python fixture's only file has a broken import. It must be reported as a Problem and NOT
    // double-listed as an "isolated" smell (its import didn't resolve — it isn't disconnected).
    let out = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(fixture_broken())
        .output()
        .expect("run compass audit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "default audit must exit 0");
    assert!(
        stdout.contains("Broken imports (1)"),
        "the broken import should be a Problem:\n{stdout}"
    );
    assert!(stdout.contains("main.py"), "stdout:\n{stdout}");
    // The broken-import file is excluded from the isolated smell, so there are none here.
    assert!(
        stdout.contains("Isolated files: none"),
        "a broken-import file must not also be listed as isolated:\n{stdout}"
    );

    // --strict must fail on the broken import (a real problem).
    let strict = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(fixture_broken())
        .arg("--strict")
        .output()
        .expect("run compass audit --strict");
    assert!(
        !strict.status.success(),
        "--strict must exit non-zero on a broken import:\n{}",
        String::from_utf8_lossy(&strict.stdout)
    );
}

#[test]
fn audit_lists_isolated_files_and_limit_elides() {
    // A repo with one connected pair (a → b) and several standalone files. The standalone files are
    // isolated; --limit caps the printed list and reports how many more were hidden.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("audit-isolated");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src").join("a.ts"),
        "import { b } from \"./b\";\nexport function a(): void { b(); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join("b.ts"),
        "export function b(): void {}\n",
    )
    .unwrap();
    // Five standalone files with no imports in or out.
    for name in ["s1", "s2", "s3", "s4", "s5"] {
        std::fs::write(
            dir.join("src").join(format!("{name}.ts")),
            format!("export const {name} = 1;\n"),
        )
        .unwrap();
    }

    // Full list: every standalone file is reported as isolated; a/b are connected so excluded.
    let full = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(&dir)
        .output()
        .expect("run compass audit");
    let stdout = String::from_utf8_lossy(&full.stdout);
    assert!(full.status.success());
    assert!(
        stdout.contains("Isolated files (5)"),
        "the five standalone files should be isolated:\n{stdout}"
    );
    assert!(stdout.contains("src/s1.ts"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("- src/a.ts"),
        "a connected file must not be isolated:\n{stdout}"
    );

    // --limit 2 caps the list and elides the rest.
    let capped = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(&dir)
        .arg("--limit")
        .arg("2")
        .output()
        .expect("run compass audit --limit 2");
    let capped_out = String::from_utf8_lossy(&capped.stdout);
    assert!(
        capped_out.contains("… and 3 more"),
        "the capped list should elide the remainder:\n{capped_out}"
    );
    // The summary still reports the complete count.
    assert!(
        capped_out.contains("5 isolated files noted as smells"),
        "the summary count must be complete even when the list is capped:\n{capped_out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_json_output_is_machine_readable() {
    // The two-file TS cycle, audited with --json, must emit a parseable report whose summary and
    // circular_imports (with a concrete path) reflect the cycle.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("audit-json");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src").join("a.ts"),
        "import { b } from \"./b\";\nexport function a(): void { b(); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join("b.ts"),
        "import { a } from \"./a\";\nexport function b(): void { a(); }\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("audit")
        .arg(&dir)
        .arg("--json")
        .output()
        .expect("run compass audit --json");
    assert!(out.status.success(), "default audit (json) must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("audit --json must be valid JSON: {e}\n{stdout}"));

    assert_eq!(report["summary"]["clean"], serde_json::json!(false));
    assert_eq!(report["summary"]["problem_count"], serde_json::json!(1));
    let cycles = report["problems"]["circular_imports"]
        .as_array()
        .expect("circular_imports array");
    assert_eq!(cycles.len(), 1, "report:\n{stdout}");
    let path = cycles[0]["path"].as_array().expect("cycle path array");
    assert_eq!(path.len(), 2, "the 2-cycle path has both files:\n{stdout}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_lists_tools_and_resolves_dependencies() {
    let stdout = mcp_session(
        fixture(),
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"file_dependencies","arguments":{"file":"main.go"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"graph_stats","arguments":{}}}"#,
        ],
    );
    // tools/list advertises the original tools...
    assert!(stdout.contains("file_dependencies"), "stdout:\n{stdout}");
    assert!(stdout.contains("broken_imports"), "stdout:\n{stdout}");
    assert!(stdout.contains("overview"), "stdout:\n{stdout}");
    // ...and the three new graph tools.
    assert!(stdout.contains("graph_stats"), "stdout:\n{stdout}");
    assert!(stdout.contains("hubs"), "stdout:\n{stdout}");
    assert!(stdout.contains("get_community"), "stdout:\n{stdout}");
    // ...and the audit-surfacing tools (so an AI host can ask about cycles / isolated files).
    assert!(stdout.contains("import_cycles"), "stdout:\n{stdout}");
    assert!(stdout.contains("isolated_files"), "stdout:\n{stdout}");
    // file_dependencies(main.go) resolves the internal import.
    assert!(stdout.contains("util/util.go"), "stdout:\n{stdout}");
    // graph_stats returns a sensible payload (its file_count field).
    assert!(stdout.contains("file_count"), "stdout:\n{stdout}");
    assert!(stdout.contains("community_count"), "stdout:\n{stdout}");
}

/// Drive a real MCP handshake over stdio and confirm the `overview` tool returns the map.
/// Proves the MCP seam (rmcp stdio → query port → core).
#[test]
fn mcp_overview_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("serve")
        .arg(fixture())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn compass serve");

    let session = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"overview","arguments":{}}}"#,
    ]
    .join("\n");

    {
        // Write the session, then drop stdin so the server sees EOF and exits.
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(session.as_bytes()).expect("write session");
        stdin.write_all(b"\n").expect("write newline");
    }

    let output = child.wait_with_output().expect("wait for compass serve");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The overview JSON is embedded (escaped) in the tools/call text result.
    assert!(
        stdout.contains("\"id\":3"),
        "no tools/call response:\n{stdout}"
    );
    assert!(stdout.contains("file_count"), "overview missing:\n{stdout}");
    assert!(
        stdout.contains("import_edge_count"),
        "overview missing:\n{stdout}"
    );
    assert!(
        stdout.contains("\"isError\":false"),
        "tool errored:\n{stdout}"
    );
}

/// Drive the pre-injection hook (`compass context --hook`) with a `UserPromptSubmit` payload on
/// stdin and confirm it records a per-session token-savings event with a positive (estimated)
/// injected-token count — the data the local `/tokens` dashboard reads.
#[test]
fn context_hook_logs_token_event() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("context-hook-tokens");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc Alpha() {}\n").unwrap();
    std::fs::write(dir.join("b.go"), "package m\n\nfunc Beta() {}\n").unwrap();

    // Build the `.compass` cache the hook loads from (fast per-prompt injection).
    let init = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("init")
        .arg(&dir)
        .output()
        .expect("run compass init");
    assert!(
        init.status.success(),
        "init stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    // Feed a session id + prompt the way Claude Code's UserPromptSubmit hook would.
    let session_id = "sess-hook-1";
    let payload = format!(r#"{{"session_id":"{session_id}","prompt":"explain Alpha and Beta"}}"#);

    let mut child = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("context")
        .arg("--hook")
        .arg(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn compass context --hook");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(payload.as_bytes()).expect("write payload");
    }
    let output = child.wait_with_output().expect("wait for compass context");
    assert!(
        output.status.success(),
        "context --hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The per-session token log exists and records a first-injection event with > 0 tokens.
    let log_path = dir
        .join(".compass/sessions")
        .join(format!("{session_id}.tokens.json"));
    let log = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("token log {} not written: {e}", log_path.display()));
    let events: serde_json::Value = serde_json::from_str(&log).expect("token log is JSON");
    let arr = events.as_array().expect("token log is an array");
    assert!(!arr.is_empty(), "no token events logged:\n{log}");
    let injected = arr[0]["est_tokens_injected"].as_u64().unwrap_or(0);
    assert!(injected > 0, "expected est_tokens_injected > 0:\n{log}");
    // Nothing has been shown yet, so the first injection de-dups nothing.
    assert_eq!(
        arr[0]["files_deduped"].as_u64(),
        Some(0),
        "first injection should drop nothing:\n{log}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- `compass guard` (opt-in PreToolUse hub-edit confirmation hook) -------------------------

/// Build a throwaway Go repo where `util/util.go` is imported by six leaf files (so it's a clear
/// high-centrality hub) and `compass init` it, so a cached graph the guard can load exists.
/// Returns the repo root.
fn make_guard_repo(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("util")).unwrap();
    std::fs::write(dir.join("go.mod"), "module example.com/guard\n\ngo 1.22\n").unwrap();
    std::fs::write(
        dir.join("util").join("util.go"),
        "package util\n\nfunc Helper() string { return \"x\" }\n",
    )
    .unwrap();
    // Six leaves, each importing the util package → util has import degree 6; each leaf has 1.
    for (i, leaf) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
        let func = (b'A' + i as u8) as char;
        std::fs::write(
            dir.join(format!("{leaf}.go")),
            format!(
                "package main\n\nimport \"example.com/guard/util\"\n\nfunc {func}() string {{ return util.Helper() }}\n"
            ),
        )
        .unwrap();
    }

    let init = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("init")
        .arg(&dir)
        .output()
        .expect("run compass init");
    assert!(
        init.status.success(),
        "init stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    dir
}

/// Build a throwaway Go repo where the top-level `main.go` imports six leaf packages but is imported
/// by nobody (high OUT-degree, zero IN-degree — a pure aggregator/entrypoint) and `compass init` it.
/// Returns the repo root. Used to prove the guard does NOT flag a zero-blast-radius file.
fn make_aggregator_repo(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("go.mod"), "module example.com/agg\n\ngo 1.22\n").unwrap();
    let mut imports = String::new();
    let mut uses = String::new();
    for i in 1..=6 {
        std::fs::create_dir_all(dir.join(format!("lib{i}"))).unwrap();
        std::fs::write(
            dir.join(format!("lib{i}")).join("lib.go"),
            format!("package lib{i}\n\nfunc F() string {{ return \"x\" }}\n"),
        )
        .unwrap();
        imports.push_str(&format!("    \"example.com/agg/lib{i}\"\n"));
        uses.push_str(&format!("    _ = lib{i}.F()\n"));
    }
    std::fs::write(
        dir.join("main.go"),
        format!("package main\n\nimport (\n{imports})\n\nfunc main() {{\n{uses}}}\n"),
    )
    .unwrap();

    let init = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("init")
        .arg(&dir)
        .output()
        .expect("run compass init");
    assert!(
        init.status.success(),
        "init stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    dir
}

/// A Claude Code `PreToolUse` payload for `tool` targeting `file_path`, with an optional session
/// id. `serde_json` handles path escaping (Windows backslashes) for us.
fn pre_tool_use(tool: &str, file_path: &std::path::Path, session: Option<&str>) -> String {
    serde_json::json!({
        "session_id": session,
        "hook_event_name": "PreToolUse",
        "cwd": file_path.parent().map(|p| p.to_string_lossy().into_owned()),
        "tool_name": tool,
        "tool_input": { "file_path": file_path.to_string_lossy() },
    })
    .to_string()
}

/// Run `compass guard <dir>`, feed `stdin`, and return the process output. The two tuning env vars
/// are cleared so the test is deterministic regardless of the developer's environment.
fn run_guard_hook(dir: &std::path::Path, stdin: &str) -> std::process::Output {
    run_guard_hook_env(dir, stdin, &[])
}

/// Like [`run_guard_hook`] but with explicit env overrides (e.g. `COMPASS_GUARD_BLOCK=1` for hard
/// `deny`, or `COMPASS_GUARD_MIN_DEGREE` to tune the threshold). Both vars are first cleared, then
/// the overrides applied, so a developer's environment can't perturb the result.
fn run_guard_hook_env(
    dir: &std::path::Path,
    stdin: &str,
    env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_compass"));
    cmd.arg("guard")
        .arg(dir)
        .env_remove("COMPASS_GUARD_MIN_DEGREE")
        .env_remove("COMPASS_GUARD_BLOCK");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn compass guard");
    {
        let mut si = child.stdin.take().expect("stdin");
        si.write_all(stdin.as_bytes()).expect("write guard stdin");
    }
    child.wait_with_output().expect("wait for compass guard")
}

/// Run `compass guard` with NO path argument — the way the installed hook actually runs it. The
/// guard must then resolve the repo root from the payload's `cwd` (searching upward for `.compass`).
fn run_guard_hook_no_path(stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("guard")
        .env_remove("COMPASS_GUARD_MIN_DEGREE")
        .env_remove("COMPASS_GUARD_BLOCK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn compass guard");
    {
        let mut si = child.stdin.take().expect("stdin");
        si.write_all(stdin.as_bytes()).expect("write guard stdin");
    }
    child.wait_with_output().expect("wait for compass guard")
}

#[test]
fn guard_asks_before_editing_a_hub_file() {
    let dir = make_guard_repo("guard-hub");
    let util = dir.join("util").join("util.go");
    let out = run_guard_hook(&dir, &pre_tool_use("Edit", &util, None));
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "guard must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(r#""permissionDecision":"ask""#),
        "editing the hub should ask:\n{stdout}"
    );
    assert!(
        stdout.contains("util/util.go"),
        "the reason should name the file:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_allows_editing_a_leaf_file() {
    let dir = make_guard_repo("guard-leaf");
    // `a.go` only imports util (degree 1) — well below the high-centrality bar.
    let leaf = dir.join("a.go");
    let out = run_guard_hook(&dir, &pre_tool_use("Edit", &leaf, None));
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success(), "guard must exit 0");
    assert!(
        stdout.trim().is_empty(),
        "a low-centrality edit must be allowed silently:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_allows_non_edit_tools() {
    let dir = make_guard_repo("guard-nonedit");
    let util = dir.join("util").join("util.go");
    // A Read of the hub file is not a mutation → allowed silently.
    let out = run_guard_hook(&dir, &pre_tool_use("Read", &util, None));
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success(), "guard must exit 0");
    assert!(
        stdout.trim().is_empty(),
        "a non-mutating tool must be allowed silently:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_allows_on_garbage_and_empty_stdin() {
    let dir = make_guard_repo("guard-garbage");
    // Empty, non-JSON, and structurally-wrong-but-valid JSON must all fail open (no panic).
    for stdin in ["", "not json at all {{{", "[1, 2, 3]"] {
        let out = run_guard_hook(&dir, stdin);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "must exit 0 on input `{stdin}`");
        assert!(
            stdout.trim().is_empty(),
            "must allow silently on input `{stdin}`:\n{stdout}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_allows_when_there_is_no_cache() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guard-nocache");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.go");
    std::fs::write(&file, "package main\n\nfunc A() {}\n").unwrap();
    // No `compass init`, so there is no `.compass` cache to load.

    let out = run_guard_hook(&dir, &pre_tool_use("Edit", &file, None));
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success(), "guard must exit 0");
    assert!(
        stdout.trim().is_empty(),
        "no cache → allow silently:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_asks_every_time_in_default_mode() {
    // Default (ask) mode must NOT silently suppress a repeat edit of the same hub in one session:
    // the hook can't see how the user answered the first prompt, so suppressing would let an edit
    // through right after the user DECLINED. It should ask every time (the host's own prompt offers
    // "don't ask again" to quiet repeats).
    let dir = make_guard_repo("guard-ask-repeat");
    let util = dir.join("util").join("util.go");
    let payload = pre_tool_use("Edit", &util, Some("sess-ask-1"));

    for nth in ["first", "second"] {
        let out = run_guard_hook(&dir, &payload);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success());
        assert!(
            stdout.contains(r#""permissionDecision":"ask""#),
            "the {nth} edit of a hub should ask (no silent suppression):\n{stdout}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_block_mode_denies_once_then_allows_within_session() {
    // In opt-in block mode the first edit of a hub is DENIED, but a given hub is denied at most once
    // per session and allowed thereafter — so a hard block can never permanently wedge you off a
    // file. (This is the fail-safe the de-dup is reserved for.)
    let dir = make_guard_repo("guard-block");
    let util = dir.join("util").join("util.go");
    let payload = pre_tool_use("Edit", &util, Some("sess-block-1"));

    let first = run_guard_hook_env(&dir, &payload, &[("COMPASS_GUARD_BLOCK", "1")]);
    let first_out = String::from_utf8_lossy(&first.stdout);
    assert!(
        first.status.success(),
        "guard must exit 0 even when denying"
    );
    assert!(
        first_out.contains(r#""permissionDecision":"deny""#),
        "block mode should deny the first hub edit:\n{first_out}"
    );

    let second = run_guard_hook_env(&dir, &payload, &[("COMPASS_GUARD_BLOCK", "1")]);
    let second_out = String::from_utf8_lossy(&second.stdout);
    assert!(second.status.success());
    assert!(
        second_out.trim().is_empty(),
        "a hub denied once this session must then be allowed (never wedged):\n{second_out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_allows_a_pure_aggregator_with_zero_dependents() {
    // Regression: a top-level aggregator (high OUT-degree, zero IN-degree) — e.g. a main.go that
    // imports many packages but is imported by nobody — has zero blast radius and must be allowed
    // silently. The old in+out metric flagged it with a self-contradictory "0 file(s) import it
    // (import degree N)" reason; the in-degree metric must not.
    let dir = make_aggregator_repo("guard-aggregator");
    let main = dir.join("main.go");

    // Sanity: the fixture really is a zero-dependent aggregator.
    let deps = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("deps")
        .arg(&dir)
        .arg("main.go")
        .output()
        .expect("run compass deps");
    let deps_out = String::from_utf8_lossy(&deps.stdout);
    assert!(
        deps_out.contains("depended on by (0)"),
        "fixture should have zero dependents:\n{deps_out}"
    );

    let out = run_guard_hook(&dir, &pre_tool_use("Edit", &main, None));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "guard must exit 0");
    assert!(
        stdout.trim().is_empty(),
        "an aggregator with zero dependents must be allowed silently:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_threshold_override_can_silence_a_hub() {
    // The documented COMPASS_GUARD_MIN_DEGREE override must actually reach the hook: a high enough
    // threshold silences a file the default would flag (util's in-degree 6 < 100).
    let dir = make_guard_repo("guard-threshold");
    let util = dir.join("util").join("util.go");
    let payload = pre_tool_use("Edit", &util, None);

    let asked = run_guard_hook(&dir, &payload);
    assert!(
        String::from_utf8_lossy(&asked.stdout).contains(r#""permissionDecision":"ask""#),
        "the default threshold should flag the hub"
    );

    let silenced = run_guard_hook_env(&dir, &payload, &[("COMPASS_GUARD_MIN_DEGREE", "100")]);
    let silenced_out = String::from_utf8_lossy(&silenced.stdout);
    assert!(silenced.status.success());
    assert!(
        silenced_out.trim().is_empty(),
        "a high COMPASS_GUARD_MIN_DEGREE must silence the hub:\n{silenced_out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_covers_multiedit_and_notebookedit_path_shapes() {
    // The path-extraction branches for `edits[].file_path` (MultiEdit) and `notebook_path`
    // (NotebookEdit) must both map to the hub and ask.
    let dir = make_guard_repo("guard-toolshapes");
    let util = dir.join("util").join("util.go");
    let util_s = util.to_string_lossy().replace('\\', "/");

    let multiedit = format!(
        r#"{{"tool_name":"MultiEdit","tool_input":{{"edits":[{{"file_path":"{util_s}"}}]}}}}"#
    );
    let me = run_guard_hook(&dir, &multiedit);
    assert!(
        String::from_utf8_lossy(&me.stdout).contains(r#""permissionDecision":"ask""#),
        "MultiEdit via edits[].file_path should ask:\n{}",
        String::from_utf8_lossy(&me.stdout)
    );

    let notebook =
        format!(r#"{{"tool_name":"NotebookEdit","tool_input":{{"notebook_path":"{util_s}"}}}}"#);
    let nb = run_guard_hook(&dir, &notebook);
    assert!(
        String::from_utf8_lossy(&nb.stdout).contains(r#""permissionDecision":"ask""#),
        "NotebookEdit via notebook_path should ask:\n{}",
        String::from_utf8_lossy(&nb.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_maps_a_relative_target_path() {
    // A relative `file_path` is joined onto the repo root and must still map to the hub.
    let dir = make_guard_repo("guard-relpath");
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"util/util.go"}}"#;
    let out = run_guard_hook(&dir, payload);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains(r#""permissionDecision":"ask""#),
        "a relative path to the hub should ask:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_allows_a_sibling_dir_name_prefix_collision() {
    // The repo-boundary check must fall on a path separator: a sibling directory that merely shares
    // a name prefix (`guard-sibling` vs `guard-sibling-other`) is NOT inside the repo → allow.
    let dir = make_guard_repo("guard-sibling");
    let sibling = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guard-sibling-other");
    let _ = std::fs::remove_dir_all(&sibling);
    std::fs::create_dir_all(&sibling).unwrap();
    let outside = sibling.join("util.go");
    std::fs::write(&outside, "package main\n\nfunc X() {}\n").unwrap();

    let out = run_guard_hook(&dir, &pre_tool_use("Edit", &outside, None));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.trim().is_empty(),
        "a sibling-dir prefix collision must be treated as outside the repo:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&sibling);
}

#[test]
fn guard_resolves_repo_root_from_payload_cwd() {
    // Launched WITHOUT a path argument (as the installed hook is) and from a SUBDIRECTORY of the
    // repo, the guard must resolve the root by searching upward for `.compass` and still flag the
    // hub — rather than silently no-opping for the whole session.
    let dir = make_guard_repo("guard-cwd-discovery");
    let util = dir.join("util").join("util.go");
    // `pre_tool_use` sets `cwd` to the file's parent dir (`<repo>/util`), a subdirectory.
    let payload = pre_tool_use("Edit", &util, None);
    let out = run_guard_hook_no_path(&payload);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains(r#""permissionDecision":"ask""#),
        "guard should find the repo root from the payload cwd and ask:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn install_guard_adds_pretooluse_hook_and_all_does_not() {
    // `install --guard` adds ONLY the opt-in PreToolUse guard hook.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-guard-hook");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("install")
        .arg("--guard")
        .arg(&dir)
        .output()
        .expect("run compass install --guard");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let settings =
        std::fs::read_to_string(dir.join(".claude/settings.json")).expect("settings.json written");
    assert!(
        settings.contains("PreToolUse"),
        "guard hook missing:\n{settings}"
    );
    assert!(
        settings.contains("guard"),
        "guard command missing:\n{settings}"
    );
    // Opt-in means ONLY the guard — `--guard` must not also wire the context hook.
    assert!(
        !settings.contains("UserPromptSubmit"),
        "install --guard should add only the guard hook:\n{settings}"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // Plain `install --all` must NOT add the guard hook.
    let dir2 = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-all-plain");
    let _ = std::fs::remove_dir_all(&dir2);
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::write(dir2.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();

    let out2 = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("install")
        .arg("--all")
        .arg(&dir2)
        .output()
        .expect("run compass install --all");
    assert!(
        out2.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let settings2 =
        std::fs::read_to_string(dir2.join(".claude/settings.json")).expect("settings.json written");
    assert!(
        settings2.contains("UserPromptSubmit"),
        "install --all should add the context hook:\n{settings2}"
    );
    assert!(
        !settings2.contains("PreToolUse"),
        "install --all must NOT add the guard hook:\n{settings2}"
    );
    assert!(
        !settings2.contains("guard"),
        "install --all must NOT mention the guard:\n{settings2}"
    );

    let _ = std::fs::remove_dir_all(&dir2);
}

#[test]
fn install_guard_is_idempotent_and_preserves_existing_hooks() {
    // Running `install --guard` twice adds the PreToolUse guard hook exactly once, and merging into
    // a settings.json that already has an unrelated hook + key preserves them.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-guard-idempotent");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();
    // Pre-existing settings: a custom model + an unrelated UserPromptSubmit hook.
    std::fs::write(
        dir.join(".claude").join("settings.json"),
        r#"{"model":"opus","hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    )
    .unwrap();

    let install = || {
        let out = Command::new(env!("CARGO_BIN_EXE_compass"))
            .arg("install")
            .arg("--guard")
            .arg(&dir)
            .output()
            .expect("run compass install --guard");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    install();
    install();

    let settings =
        std::fs::read_to_string(dir.join(".claude/settings.json")).expect("settings.json");
    let v: serde_json::Value = serde_json::from_str(&settings).expect("settings is JSON");
    let pre = v["hooks"]["PreToolUse"]
        .as_array()
        .expect("PreToolUse array");
    assert_eq!(
        pre.len(),
        1,
        "guard hook must not be duplicated:\n{settings}"
    );
    // Pre-existing settings preserved.
    assert_eq!(
        v["model"].as_str(),
        Some("opus"),
        "model key dropped:\n{settings}"
    );
    assert!(
        settings.contains("echo hi"),
        "pre-existing UserPromptSubmit hook dropped:\n{settings}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn install_guard_without_a_map_warns_to_run_init() {
    // `install --guard` only writes the hook; with no `.compass` map the guard would silently do
    // nothing, so the command must tell the user to run `compass init` (no false sense of security).
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("install-guard-nomap");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.go"), "package m\n\nfunc A() {}\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_compass"))
        .arg("install")
        .arg("--guard")
        .arg(&dir)
        .output()
        .expect("run compass install --guard");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("compass init"),
        "install --guard with no map should point at `compass init`:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

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

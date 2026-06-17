//! The single cross-crate end-to-end smoke test: run the real `mapai` binary against the
//! Go fixture repo and assert the rendered overview. Proves walk → extract → resolve →
//! graph → query through the actual CLI. Per-language extraction is tested inside each
//! `mapai-lang-*` crate; this only checks the wiring.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture")
}

fn fixture_py() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture-py")
}

#[test]
fn overview_of_go_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_mapai"))
        .arg("overview")
        .arg(fixture())
        .output()
        .expect("run mapai");

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
fn overview_of_python_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_mapai"))
        .arg("overview")
        .arg(fixture_py())
        .output()
        .expect("run mapai");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "non-zero exit\nstderr:\n{stderr}");

    // 3 .py files; `from app.util import helper` resolves to one edge; `os` is external.
    assert!(stdout.contains("files:        3"), "stdout:\n{stdout}");
    assert!(stdout.contains("import edges:  1"), "stdout:\n{stdout}");
    assert!(stdout.contains("diagnostics:  0"), "stdout:\n{stdout}");
    assert!(stdout.contains("python"), "stdout:\n{stdout}");
}

/// Drive a real MCP handshake over stdio and confirm the `overview` tool returns the map.
/// Proves the MCP seam (rmcp stdio → query port → core).
#[test]
fn mcp_overview_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mapai"))
        .arg("serve")
        .arg(fixture())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mapai serve");

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

    let output = child.wait_with_output().expect("wait for mapai serve");
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

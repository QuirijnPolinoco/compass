//! The single cross-crate end-to-end smoke test: run the real `mapai` binary against the
//! Go fixture repo and assert the rendered overview. Proves walk → extract → resolve →
//! graph → query through the actual CLI. Per-language extraction is tested inside each
//! `mapai-lang-*` crate; this only checks the wiring.

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e/fixture")
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

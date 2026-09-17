//! `compass watch` and `compass map` — live re-mapping, and the interactive visual map.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use compass_core::MapQuery;

use crate::build_graph;

pub(crate) fn run_watch(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    let _ = compass_engine::cache::save(path, &graph);
    let ov = graph.overview();
    println!(
        "Watching {} — {} files, {} symbols, {} import edges, {} diagnostics",
        path.display(),
        ov.file_count,
        ov.symbol_count,
        ov.import_edge_count,
        ov.diagnostic_count
    );
    println!("Editing files re-maps automatically. Press Ctrl+C to stop.");

    let result =
        compass_engine::watch::watch(path, std::time::Duration::from_millis(500), |paths| {
            if let Some(graph) = build_graph(path) {
                let _ = compass_engine::cache::save(path, &graph);
                let ov = graph.overview();
                println!(
                    "~ {} change(s) -> {} files, {} symbols, {} import edges, {} diagnostics",
                    paths.len(),
                    ov.file_count,
                    ov.symbol_count,
                    ov.import_edge_count,
                    ov.diagnostic_count
                );
            }
        });
    if let Err(e) = result {
        eprintln!("compass: watch error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// `compass map` — serve the interactive visual map on localhost and keep it live as you
/// edit (ADR-0005). `--snapshot` instead writes a self-contained `.compass/map.html` and
/// exits. The viz server is the composition point that wires the engine into `compass-viz`
/// and republishes a fresh map on every watch event.
pub(crate) fn run_map(args: &[String]) -> ExitCode {
    let mut path = PathBuf::from(".");
    let mut port: Option<u16> = None;
    let mut open = true;
    let mut snapshot = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--no-open" => open = false,
            "--snapshot" => snapshot = true,
            "--port" => match iter.next().and_then(|p| p.parse::<u16>().ok()) {
                Some(p) => port = Some(p),
                None => {
                    eprintln!("compass: --port needs a number, e.g. `--port 62049`");
                    return ExitCode::FAILURE;
                }
            },
            other if other.starts_with("--port=") => {
                match other["--port=".len()..].parse::<u16>() {
                    Ok(p) => port = Some(p),
                    Err(_) => {
                        eprintln!("compass: --port needs a number, e.g. `--port=62049`");
                        return ExitCode::FAILURE;
                    }
                }
            }
            other if other.starts_with('-') => {
                eprintln!("compass: unknown option `{other}` for `map`");
                return ExitCode::FAILURE;
            }
            other => path = PathBuf::from(other),
        }
    }

    let Some(graph) = build_graph(&path) else {
        return ExitCode::FAILURE;
    };
    if let Err(e) = compass_engine::cache::save(&path, &graph) {
        eprintln!("compass: warning: could not write cache: {e:#}");
    }

    if snapshot {
        let query: compass_viz::Query = Arc::new(graph);
        let out = path.join(".compass").join("map.html");
        match std::fs::write(&out, compass_viz::snapshot_html(&query)) {
            Ok(()) => {
                println!("compass: wrote {} (open it in a browser).", out.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("compass: could not write {}: {e:#}", out.display());
                ExitCode::FAILURE
            }
        }
    } else {
        let state = compass_viz::MapState::new(Arc::new(graph), path.clone());
        let server = match compass_viz::bind(port) {
            Ok(server) => server,
            Err(e) => {
                eprintln!("compass: could not start the map server: {e}");
                return ExitCode::FAILURE;
            }
        };
        let url = server.url();
        println!("compass: serving the map at {url}");
        println!("Editing files updates it live. Press Ctrl+C to stop.");

        let server_state = Arc::clone(&state);
        std::thread::spawn(move || server.run(server_state));
        if open {
            open_browser(&url);
        }

        let watch_path = path.clone();
        let watch_state = Arc::clone(&state);
        let result =
            compass_engine::watch::watch(&path, std::time::Duration::from_millis(500), move |_| {
                if let Some(graph) = build_graph(&watch_path) {
                    let _ = compass_engine::cache::save(&watch_path, &graph);
                    watch_state.publish(Arc::new(graph));
                }
            });
        if let Err(e) = result {
            eprintln!("compass: watch error: {e:#}");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    }
}

/// Best-effort: open `url` in the default browser. The URL is always printed too, so a
/// failure here is non-fatal.
fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();

    let _ = result;
}

//! ZoneBoard desktop app: a native window (Tauri / system WebView) around the local server.
//! The server and all logic are in the library; this file only opens the window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{WebviewUrl, WebviewWindowBuilder};

fn main() {
    let data_dir = zoneboard::db::ensure_data_dir().unwrap_or_else(|e| {
        eprintln!("ZoneBoard: {e}");
        std::process::exit(1);
    });
    zoneboard::logging::init(&data_dir);
    let port = zoneboard::server::spawn().unwrap_or_else(|e| {
        eprintln!("ZoneBoard: {e}");
        std::process::exit(1);
    });
    tauri::Builder::default()
        .setup(move |app| {
            let url: tauri::Url = format!("http://127.0.0.1:{port}/").parse()?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("ZoneBoard")
                .inner_size(1360.0, 860.0)
                .min_inner_size(900.0, 560.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running ZoneBoard");
}

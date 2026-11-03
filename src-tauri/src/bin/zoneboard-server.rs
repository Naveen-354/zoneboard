//! Headless ZoneBoard: the same server as the desktop app, for a normal browser tab.
//!   zoneboard-server            then open http://127.0.0.1:5000
//!   SAMPLE_DATA=1 zoneboard-server     made-up sample prices for development, no internet needed for NSE
use zoneboard::{journal, server};

#[tokio::main]
async fn main() {
    let run = async {
        let listeners = server::bind().map_err(|e| format!("can't start the local server: {e}"))?;
        let port = server::port_of(&listeners);
        let state = server::AppState::new(server::sample_from_env(), &journal::default_path())?;
        println!("ZoneBoard running at http://127.0.0.1:{port}  (Ctrl+C to stop){}", if server::sample_from_env() { "  [SAMPLE data]" } else { "" });
        server::serve(listeners, state).await.map_err(|e| e.to_string())
    };
    if let Err(e) = run.await {
        eprintln!("ZoneBoard: {e}");
        std::process::exit(1);
    }
}

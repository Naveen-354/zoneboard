//! Headless ZoneBoard: the same server as the desktop app, for a normal browser tab.
//!   zoneboard-server            then open http://127.0.0.1:5000
//!   DEMO=1 zoneboard-server     synthetic NSE data, no internet needed for that market
use zoneboard::{journal, server};

#[tokio::main]
async fn main() {
    let run = async {
        let listeners = server::bind().map_err(|e| format!("can't start the local server: {e}"))?;
        let port = server::port_of(&listeners);
        let state = server::AppState::new(server::demo_from_env(), &journal::default_path())?;
        println!("ZoneBoard running at http://127.0.0.1:{port}  (Ctrl+C to stop){}", if server::demo_from_env() { "  [DEMO data]" } else { "" });
        server::serve(listeners, state).await.map_err(|e| e.to_string())
    };
    if let Err(e) = run.await {
        eprintln!("ZoneBoard: {e}");
        std::process::exit(1);
    }
}

//! ZoneBoard core: the zone engine, market data, trade journal, backtest and the local web server.
//! The desktop window (`main.rs`), the headless server and the backtest CLI are thin shells around this.

pub mod backtest;
pub mod data;
pub mod db;
pub mod journal;
pub mod logging;
pub mod money;
pub mod server;
pub mod vault;
pub mod zones;

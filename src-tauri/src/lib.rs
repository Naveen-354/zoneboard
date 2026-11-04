//! ZoneBoard core: the zone engine, market data, trade journal, backtest and the local web server.
//! The desktop window (`main.rs`), the headless server and the backtest CLI are thin shells around this.

pub mod accounts;
pub mod ai;
pub mod approvals;
pub mod audit;
pub mod backtest;
pub mod backtest2;
pub mod botapi;
pub mod botengine;
pub mod bots;
pub mod connections;
pub mod data;
pub mod db;
pub mod exchange;
pub mod execution;
pub mod indicators;
pub mod journal;
pub mod logging;
pub mod marketdata;
pub mod money;
pub mod orders;
pub mod portfolio;
pub mod risk;
pub mod scanner;
pub mod server;
pub mod settings;
pub mod strategies;
pub mod strategy;
pub mod timezones;
pub mod vault;
pub mod zones;

#![cfg_attr(feature = "e2e", allow(dead_code, unused_imports))]

#[cfg(all(feature = "e2e", not(debug_assertions)))]
compile_error!("the e2e feature is forbidden in production/release builds");

mod app;
mod commands;
mod constants;
mod error;
mod logger;
mod models;
mod services;
mod state;
mod utils;

pub fn run() {
    app::run();
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod metadata;
mod delivery;
mod history;
mod model;
mod net;
mod storage;
mod time_sync;
mod transfer;
mod ui;

pub const APP_VERSION: &str = "v1.2.0";

#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => {
        let msg = format!($($arg)*);
        eprintln!("{}", msg);
        crate::storage::write_log("DEBUG", &msg);
    };
}

#[macro_export]
macro_rules! info_println {
    ($($arg:tt)*) => {
        let msg = format!($($arg)*);
        eprintln!("{}", msg);
        crate::storage::write_log("INFO", &msg);
    };
}

#[macro_export]
macro_rules! warn_println {
    ($($arg:tt)*) => {
        let msg = format!($($arg)*);
        eprintln!("{}", msg);
        crate::storage::write_log("WARN", &msg);
    };
}

#[macro_export]
macro_rules! error_println {
    ($($arg:tt)*) => {
        let msg = format!($($arg)*);
        eprintln!("{}", msg);
        crate::storage::write_log("ERROR", &msg);
    };
}

fn main() -> eframe::Result<()> {
    storage::cleanup_old_logs(7);
    storage::write_log("INFO", &format!("Rustle {} starting...", crate::APP_VERSION));

    std::panic::set_hook(Box::new(|info| {
        let msg = format!("PANIC: {}", info);
        eprintln!("{}", msg);
        crate::storage::write_log("ERROR", &msg);
    }));

    time_sync::sync_system_time_at_startup();

    ui::run()
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 在 release 模式下禁用调试输出的宏
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => {
        eprintln!($($arg)*);
    };
}

#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => {};
}

mod model;
mod storage;
mod transfer;
mod net;
mod ui;

pub const APP_VERSION: &str = "v1.1.1";

fn main() -> eframe::Result<()> {
    ui::run()
}

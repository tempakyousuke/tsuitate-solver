mod commands;
pub mod shogi;
pub mod solver;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub type CancelFlag = Arc<AtomicBool>;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(CancelFlag::new(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![
            commands::validate_position,
            commands::solve,
            commands::cancel_solve,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

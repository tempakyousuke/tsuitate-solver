#[cfg(feature = "gui")]
mod commands;
pub mod shogi;
pub mod solver;

/// ソルバーの確保・解放パターン（小サイズ Vec・Position クローンの高頻度 churn）に
/// 対してシステム malloc より高速なため、全バイナリ（GUI/CLI/テスト）で mimalloc を使う
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub type CancelFlag = Arc<AtomicBool>;

#[cfg(feature = "gui")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(CancelFlag::new(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![
            commands::validate_position,
            commands::solve,
            commands::cancel_solve,
            commands::list_sample_questions,
            commands::load_sample_question,
            commands::list_kakurenbo_questions,
            commands::load_kakurenbo_question,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

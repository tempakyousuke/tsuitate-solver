fn main() {
    // gui feature 無効時（サーバーでの CLI ビルド）は tauri の設定検証をスキップする。
    // frontendDist (../dist) は gitignore されており、クローン直後には存在しないため。
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        tauri_build::build()
    }
}

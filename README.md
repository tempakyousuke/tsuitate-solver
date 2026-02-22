# 衝立詰将棋ソルバー (tsuitate-solver)

衝立詰将棋（ついたてつめしょうぎ）の解を探索するデスクトップアプリケーションです。

## 必要な環境

### Rust

Rust のツールチェインが必要です。未インストールの場合は [rustup](https://rustup.rs/) からインストールしてください。

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Node.js

Node.js (v18 以上) と npm が必要です。[公式サイト](https://nodejs.org/) からインストールしてください。

### OS ごとの追加依存

Tauri v2 のビルドには OS ごとにシステムライブラリが必要です。

- **macOS**: Xcode Command Line Tools (`xcode-select --install`)
- **Linux**: `build-essential`, `libwebkit2gtk-4.1-dev`, `libssl-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` など（ディストリビューションにより異なります。詳細は [Tauri 公式ドキュメント](https://v2.tauri.app/start/prerequisites/) を参照してください）
- **Windows**: Microsoft C++ Build Tools、WebView2（詳細は [Tauri 公式ドキュメント](https://v2.tauri.app/start/prerequisites/) を参照してください）

## セットアップ

```bash
git clone <リポジトリURL>
cd tsuitate-resolver
npm install
```

## 開発

開発サーバーを起動してアプリをホットリロード付きで実行します。

```bash
npm run tauri dev
```

## ビルド

リリース用のバイナリを生成します。

```bash
npm run tauri build
```

成果物は `src-tauri/target/release/bundle/` 以下に生成されます。

## テスト

```bash
# Rust ユニットテスト
cd src-tauri && cargo test

# リリースビルドでのテスト
cd src-tauri && cargo test --release

# ベンチマークテスト（時間がかかります）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_all -- --ignored --nocapture
```

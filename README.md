# sample-vsti

RustでVST3のAPI・ABIを直接実装する、最小の音源プラグインです。
現在はホストからの認識・生成・終了に対応し、音声出力は常に無音です。
VST3用のバインディング、C/C++ラッパー、依存クレートは使用しません。

## 現在の対応範囲

| 項目 | 内容 |
| --- | --- |
| OS・アーキテクチャ | Linux x86_64、GNU環境 |
| ホスト上の表示名 | Sample VSTi |
| 分類 | Instrument / Synth |
| 音声入力・出力 | 入力なし、ステレオ出力1バス |
| イベント | 入力1バス・16チャンネル。受信イベントはすべて無視 |
| サンプル形式 | float32 / float64 |
| パラメーター・独自GUI | なし |
| 状態保存 | 設定を持たないため空の状態を保存・復元 |
| レイテンシー・テール | ともに0サンプル |

Processorとパラメーター0個のControllerは、同一インスタンスから取得できます。
処理コールバックでは、ホストから渡された有効な出力バッファをゼロで埋めます。
発音、MIDI制御、オートメーション、音色の読み込みはまだ実装していません。
Windows・macOS向けのABIとバンドルは未対応です。

## ビルド

Rust 1.85以上とLinuxのCリンカーが必要です。プラグイン本体のビルドにVST3 SDKは不要です。

```sh
sh scripts/build-vst3.sh
```

生成物:

```text
target/vst3/SampleVsti.vst3/
└── Contents/
    └── x86_64-linux/
        └── SampleVsti.so
```

ビルドスクリプトはリリースビルドを行い、ホストが読み込めるバンドルを作成します。
開発用の共有ライブラリだけが必要な場合は `cargo build` を使用します。

## ホストへの導入

ホストの探索パスに `target/vst3` を追加するか、生成した `SampleVsti.vst3` ディレクトリ全体を
ユーザー用のVST3ディレクトリへコピーします。

```sh
mkdir -p "$HOME/.vst3"
cp -R target/vst3/SampleVsti.vst3 "$HOME/.vst3/"
```

ホストで再スキャンし、音源一覧の **Sample VSTi** を読み込みます。
MIDIを送っても発音せず、操作用の独自画面も開きません。
更新時はホストを終了してから、配置済みバンドルを差し替えてください。

## 検証

Rust側のレイアウト・識別子・参照管理のテストと静的解析:

```sh
rustup component add rustfmt clippy
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

ABIを変更した場合は、公式VST3 SDKのヘッダを用いる検証も実施します。
[公式VST3開発ドキュメント](https://steinbergmedia.github.io/vst3_dev_portal/) からSDKの取得方法を確認し、
`pluginterfaces` を含む展開先を指定してください。C++17対応コンパイラーも必要です。

```sh
export VST3_SDK_DIR=/path/to/vst3-sdk
sh scripts/check-abi.sh
```

この検証はSDKのインターフェースで共有ライブラリをロードし、100回のインスタンス生成・解放、
複数のブロック長、32/64ビットの無音出力、バッファ境界、同一バッファを指す出力、停止中のフラッシュを確認します。
検証用C++コードはプラグインにリンクされません。

公式Validatorのビルド・実行には、完全なVST3 SDKとCMake 3.25以上が必要です。

```sh
cmake -S "$VST3_SDK_DIR" -B target/vst3-sdk \
    -DCMAKE_BUILD_TYPE=Release \
    -DSMTG_ENABLE_VSTGUI_SUPPORT=OFF \
    -DSMTG_ENABLE_VST3_PLUGIN_EXAMPLES=OFF \
    -DSMTG_ENABLE_VST3_HOSTING_EXAMPLES=OFF \
    -DSMTG_CREATE_PLUGIN_LINK=OFF
cmake --build target/vst3-sdk --target validator --parallel 4
sh scripts/build-vst3.sh
target/vst3-sdk/bin/Release/validator target/vst3/SampleVsti.vst3
```

確認済み環境: Linux x86_64、Rust 1.85.0 / 1.99.1、GCC 12.2、VST3 SDK 3.8.1配布構成
（インターフェース定義3.8.0）。Rustテスト3件、ABI検証、Validator 47件が成功しています。
GUIを持つDAW上での認識・操作は未確認です。

## Rust CI

GitHub Actionsの `.github/workflows/rust.yml` で、push・プルリクエスト・手動実行・毎週月曜03:23 UTCに以下を検証します。
実装対象に合わせ、ランナーはLinux x86_64に限定しています。

| ジョブ | 内容 |
| --- | --- |
| Format, Clippy, docs | rustfmt、警告をエラーにするClippyとドキュメント生成 |
| Build and test | MSRV（Rust 1.85.0）と最新stableでcheck・debug/releaseテスト・releaseビルド |
| Miri | 固定したnightlyでRustテストの未定義動作を検査 |
| Coverage | RustテストのLCOVレポートを生成・保存 |
| Dependency audit | cargo-denyで脆弱性・ライセンス・依存元・依存バージョンを監査 |
| VST3 ABI and Validator | 公式SDKの固定版でABI検証・Validatorを実行し、バンドルとログを保存 |
| CI result | 上記すべてのジョブの成功を確認 |

CIは読み取り専用のリポジトリ権限で動作します。成果物は14日間保持します。
Miriとカバレッジの対象はRustテストです。外部ホストとのABI互換性は別のVST3ジョブで検証します。
現時点ではカバレッジ率の最低値は設けていません。

CIと同じRust用ツールをローカルへ一括インストールするには、rustup導入済みの環境で次を実行します。
stableの更新、MSRV・nightlyの追加、rustfmt・Clippy・LLVMツール・Miri・cargo-deny・cargo-llvm-covの導入を行います。

```sh
sh scripts/install-ci-tools.sh
```

追加ツールとnightlyのバージョン、検証用SDKのリビジョンは `scripts/ci-versions.sh` で管理します。
更新時はCI全体を再検証してください。SDK・CMake・C++コンパイラーは上記のVST3検証手順で別途準備します。

追加された検証のローカル実行例:

```sh
. scripts/ci-versions.sh
cargo +"$CI_NIGHTLY" miri test --locked --lib
cargo +stable deny --locked check
mkdir -p target/coverage
cargo +stable llvm-cov --locked --workspace --all-features --lcov --output-path target/coverage/lcov.info
```

現在は依存クレートがないため、`deny.toml` の依存ライセンス許可リストは空です。
依存を追加する際は、利用条件を確認して許可リストも更新してください。本体の配布ライセンスはこの設定では決定しません。
ブランチ保護で全ジョブを必須にする場合は、集約チェック **CI result** を指定できます。

## 構成

- `src/abi.rs`: 必要なABI型、インターフェースID、vtable定義。
- `src/lib.rs`: モジュールの公開関数とファクトリー。
- `src/plugin.rs`: インスタンス、バス、無音処理、空のController。
- `src/tests.rs`: Rust側のABIレイアウトと参照管理のテスト。
- `tests/abi-smoke.cpp`: 公式SDKの型を使う独立したABI検証。
- `scripts/`: バンドル生成とABI検証のスクリプト。
- `.github/workflows/rust.yml`: RustとVST3のCI。
- `scripts/install-ci-tools.sh`: CI用Rustツールの一括インストーラー。
- `scripts/ci-versions.sh`: CI用ツールと検証SDKの固定バージョン。
- `deny.toml`: 依存関係の監査ポリシー。

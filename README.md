# sample-vsti

RustでVST3のAPI・ABIを直接実装する、最小の音源プラグインです。
単一のサイン波オシレータで、ノートオン・オフに応じて発音・消音します。
VST3用のバインディング、C/C++ラッパー、依存クレートは使用しません。

## 現在の対応範囲

| 項目 | 内容 |
| --- | --- |
| OS・アーキテクチャ | Linux x86_64（GNU）、Windows x86_64（MSVC） |
| ホスト上の表示名 | Sample VSTi |
| 分類 | Instrument / Synth |
| 音声入力・出力 | 入力なし、ステレオ出力1バス |
| イベント | 入力1バス・16チャンネル。Note On / Note Offを処理（全チャンネルで最大1音） |
| サンプル形式 | float32 / float64 |
| ホスト公開パラメーター・独自GUI | なし |
| 状態保存 | 設定を持たないため空の状態を保存・復元 |
| レイテンシー・テール | ともに0サンプル |

Processorとパラメーター0個のControllerは、同一インスタンスから取得できます。
左右に同じ波形を出力します。エンベロープ、多重化、CC、ピッチベンド、
ノートエクスプレッション、オートメーション、音色の読み込みは未実装です。
macOS・Windows ARM64・32ビット環境は未対応です。

## 発音と位相

- 後から届いたNote Onを優先し、単一オシレータの周波数と振幅を更新します。古いノートには戻りません。
- 音程は12平均律（ノート番号69 = 440 Hz）とイベントのtuning（セント）で決めます。
  振幅は `0.2 × velocity` です。velocityが0のNote Onもノートを更新し、出力振幅が0になります。
- Note Offは、両イベントにnoteIdがあればIDで照合します。どちらかが `-1` の場合はチャンネルと音高で照合します。
  IDがない同チャンネル・同音高の重複は識別できません。
- `OscillatorParams` が周波数・振幅を管理し、`Oscillator` 自身が次のサンプルの位相を保持します。
  初期位相は0。ノートの追加・音程変更・ブロック境界で位相をリセットしません。
- Note Offでは即座に消音し、消音中は位相を止めます。次のNote Onで保持した位相から再開します。
  エンベロープやスムージングを持たないため、発音・消音・振幅変更でクリックが発生し得ます。
- `sampleOffset` の位置でイベントを反映します。同位置ではリスト順です。取得失敗、未対応の種類・バス、
  不正な値、範囲外または処理済み位置より前のイベントは無視します。
- イベントがないブロックでも発音を継続します。処理中は出力バッファがなくてもイベントを処理し、
  発音中の位相を進めます。0サンプルの場合はoffset 0のイベントだけを処理します。
- 処理停止・無効化では消音しますが位相は保持します。処理停止中の0サンプルフラッシュではノートを受け付けません。
  ホストのトランスポート再生状態は参照しないため、停止中でもホストが処理とノートを送れば演奏できます。

オシレータの設定は内部構造体で管理し、現段階ではホスト公開パラメーターや保存対象にはしていません。

## ビルド

Rust 1.85以上が必要です。LinuxではCリンカー、WindowsではVisual Studio Build Toolsの
「C++によるデスクトップ開発」とWindows SDKを導入してください。
プラグイン本体のビルドにVST3 SDKは不要です。バンドル・ZIP生成にはPython 3とGitも使用します。

Linux:

```sh
sh scripts/build-vst3.sh
```

Windows（PowerShell）:

```powershell
rustup target add x86_64-pc-windows-msvc
python scripts/build-vst3.py
```

生成物（Linux）:

```text
target/vst3/SampleVsti.vst3/
└── Contents/
    ├── x86_64-linux/
    │   └── SampleVsti.so
    └── Resources/
        ├── build-info.json
        └── Licenses/
            ├── LICENSE
            ├── COPYRIGHT
            └── THIRD_PARTY_NOTICES.txt
```

Windowsではバイナリの位置が `Contents/x86_64-win/SampleVsti.vst3` になります。
`target/dist/` にもバンドル全体を含むZIPを生成します。Windows版はCRTを静的リンクします。

ビルドスクリプトは最適化ビルドを行い、ホストが読み込めるバンドルを作成します。
開発用の共有ライブラリだけが必要な場合は `cargo build` を使用します。

## バージョン情報

ホストへ返す `PClassInfo2.version` は次の形式です。

- 開発版: `dev-<40桁のコミットID>`。GitのHEADを使用します。
- 正式リリース版: `Cargo.toml` の `version` に指定したSemantic Versioning（例: `0.1.0`）。

`cargo build --release` の最適化指定だけでは正式リリース版になりません。
正式版は `SAMPLE_VSTI_RELEASE_VERSION=0.1.0` を明示してビルドします。
値がCargo.tomlと異なる場合、またはGitの作業ツリーに変更がある場合は失敗します。
CIでは `v0.1.0` のようなタグのpush時に正式版を生成し、タグとCargo.tomlの一致を検証します。

`Contents/Resources/build-info.json` にバージョン、完全なコミットID、変更の有無、ターゲットを記録します。
ローカルの未コミット変更は `dirty: true` で識別し、CIでは変更のないチェックアウトだけを配布します。
Git情報のないソースからビルドする場合は、出典の完全なIDを `SAMPLE_VSTI_COMMIT` に指定してください。
Gitチェックアウトでこの値を指定した場合、HEADとの不一致は拒否します。

## ホストへの導入

ホストの探索パスに `target/vst3` を追加するか、生成した `SampleVsti.vst3` ディレクトリ全体を
ユーザー用のVST3ディレクトリへコピーします。

```sh
mkdir -p "$HOME/.vst3"
cp -R target/vst3/SampleVsti.vst3 "$HOME/.vst3/"
```

Windowsでは展開した `SampleVsti.vst3` フォルダを、ホストの探索先または
`C:\Program Files\Common Files\VST3` に配置します。システム共通フォルダへの配置には管理者権限が必要です。

ホストで再スキャンし、音源一覧の **Sample VSTi** を読み込みます。
ホストの鍵盤やピアノロールからノートを送り、単音の発音と消音を確認してください。操作用の独自画面はありません。
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

Windowsでは「x64 Native Tools Command Prompt for VS」から次を実行します。

```bat
set VST3_SDK_DIR=C:\path\to\vst3-sdk
python scripts/check-abi.py
```

この検証はSDKのインターフェースで共有ライブラリをロードし、バージョン・ID・戻り値、100回のインスタンス生成・解放、
複数のブロック長、32/64ビットの発音・消音、音程変更・再発音での位相継承、
イベントのサンプル位置とノート照合、バッファ境界、同一バッファを指す出力、停止・再開とフラッシュを確認します。
音声テストは44.1 / 48 / 96 kHzとリアルタイム・プリフェッチ・オフラインモードを含みます。
検証用C++コードはプラグインにリンクされません。

公式Validatorのビルド・実行には、完全なVST3 SDKとCMake 3.25以上が必要です。

```sh
cmake -S "$VST3_SDK_DIR" -B target/vst3-sdk \
    -DCMAKE_BUILD_TYPE=Release \
    -DSMTG_ENABLE_VSTGUI_SUPPORT=OFF \
    -DSMTG_ENABLE_VST3_PLUGIN_EXAMPLES=OFF \
    -DSMTG_ENABLE_VST3_HOSTING_EXAMPLES=OFF \
    -DSMTG_CREATE_PLUGIN_LINK=OFF
cmake --build target/vst3-sdk --config Release --target validator --parallel 4
sh scripts/build-vst3.sh
target/vst3-sdk/bin/Release/validator target/vst3/SampleVsti.vst3
```

Linuxでの確認済み環境: Linux x86_64、Rust 1.85.0 / 1.99.1、GCC 12.2、VST3 SDK 3.8.1
（インターフェース定義も3.8.1）。Rustテスト8件、ABI検証、Validator 47件が成功しています。
参照した公式SDKヘッダのライセンスはMITです。出典とライセンス全文は [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) を参照してください。
以前の無音版はFL Studioで正常に認識されたとの動作報告があります。今回の発音処理のDAW上での試聴は未確認です。

## PRのテスト用ZIP

PRの作成・更新ごとに、Linux版とWindows版をビルドし、ABI検証と公式Validatorに成功した
バンドルをGitHub ActionsのArtifactsへ保存します。PRのChecksから実行結果を開き、
サマリーのダウンロードリンクまたはArtifactsからZIPを取得してください。保持期間は14日です。

例: `SampleVsti-windows-x86_64-pr2-<40桁のコミットID>.zip`

PRでは先頭コミットを明示的にチェックアウトします。ZIP名・プラグインの開発版バージョン・
`build-info.json` は同じコミットを指します。ZIPを展開すると `SampleVsti.vst3` が得られます。
ダウンロードにはGitHubへのログインとリポジトリの読み取り権限が必要です。

## Rust CI

GitHub Actionsの `.github/workflows/rust.yml` で、mainへのpush・リリースタグのpush・プルリクエスト・手動実行・毎週月曜03:23 UTCに以下を検証します。
Linux x86_64とWindows x86_64のランナーを使用します。

| ジョブ | 内容 |
| --- | --- |
| Format, Clippy, docs | rustfmt、警告をエラーにするClippyとドキュメント生成 |
| Build and test | Linux/WindowsそれぞれでMSRV（Rust 1.85.0）と最新stableのcheck・debug/releaseテスト・releaseビルド |
| Miri | 固定したnightlyでRustテストの未定義動作を検査 |
| Coverage | RustテストのLCOVレポートを生成・保存 |
| Dependency audit | cargo-denyで脆弱性・ライセンス・依存元・依存バージョンを監査 |
| VST3 ABI and Validator | Linux/Windowsそれぞれで公式SDKの固定版を使ったABI検証・Validatorを実行し、バンドルZIPとログを保存 |
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

- `build.rs`: Gitコミットと正式版指定からバージョン・ビルド情報を生成。
- `src/abi.rs`: 必要なABI型、OS別のID表現・戻り値、vtable定義。
- `src/lib.rs`: モジュールの公開関数とファクトリー。
- `src/plugin.rs`: インスタンス、バス、イベント処理、音声出力、空のController。
- `src/oscillator.rs`: パラメーター構造体と位相を保持するサイン波オシレータ。
- `src/synth.rs`: 単音のノート照合と発音・消音。
- `src/tests.rs`: Rust側のABIレイアウトと参照管理のテスト。
- `tests/abi-smoke.cpp`: 公式SDKの型を使う独立したABI検証。
- `scripts/build-vst3.py` / `scripts/check-abi.py`: Linux/Windows共通のバンドル・ZIP生成とABI検証。
- `tests/build-metadata.py`: コミット変更時の再ビルド、開発版・正式版の切り替え、出典検証。
- `.github/workflows/rust.yml`: RustとVST3のCI。
- `scripts/install-ci-tools.sh`: CI用Rustツールの一括インストーラー。
- `scripts/ci-versions.sh`: CI用ツールと検証SDKの固定バージョン。
- `deny.toml`: 依存関係の監査ポリシー。

## ライセンス

Copyright (C) 2026 Yusuke Oda

本プロジェクトは **GNU Affero General Public License version 3（AGPL-3.0-only）** で提供します。
ライセンス全文は [LICENSE](LICENSE)、著作権表示と無保証の説明は [COPYRIGHT](COPYRIGHT) を参照してください。
ソースコードの取得先は [本プロジェクトのリポジトリ](https://github.com/predicatejp/sample-vsti) です。

ABI定義の参照・移植元であるSteinberg公式VST3ヘッダのMITライセンスと著作権表示は、
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) に保持しています。
ビルドスクリプトはこれら3つの通知ファイルをバンドルの `Contents/Resources/Licenses/` に同梱します。

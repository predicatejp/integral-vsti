# sample-vsti

RustでVST3のAPI・ABIを直接実装する、最小の音源プラグインです。
単一のオシレータで、ノートオン・オフに応じて発音・消音します。
VST3パラメーターからサイン波・三角波・矩形波・ノコギリ波を選択できます。
VST3用のバインディングやC/C++ラッパーは使用しません。GUIにはeguiを使用します。

## 現在の対応範囲

| 項目 | 内容 |
| --- | --- |
| OS・アーキテクチャ | Linux x86_64（GNU）、Windows x86_64（MSVC） |
| ホスト上の表示名 | Sample VSTi |
| 分類 | Instrument / Synth |
| 音声入力・出力 | 入力なし、ステレオ出力1バス |
| イベント | 入力1バス・16チャンネル。Note On / Note Offを処理（全チャンネルで最大1音） |
| サンプル形式 | float32 / float64 |
| ホスト公開パラメーター・独自GUI | Waveform（4択）を公開。eguiの4つのボタンでも選択可能 |
| 状態保存 | 波形の選択をVST3のIBStreamで保存・復元 |
| レイテンシー・テール | ともに0サンプル |

ProcessorとControllerは、同一インスタンスから取得できます。
左右に同じ波形を出力します。エンベロープ、多重化、CC、ピッチベンド、
ノートエクスプレッションは未実装です。
macOS・Windows ARM64・32ビット環境は未対応です。

## 発音と位相

- 後から届いたNote Onを優先し、単一オシレータの周波数と振幅を更新します。古いノートには戻りません。
- 音程は12平均律（ノート番号69 = 440 Hz）とイベントのtuning（セント）で決めます。
  基準実効値を `BASE_RMS_AMPLITUDE = 0.5` と定義し、全波形で目標実効値を
  `BASE_RMS_AMPLITUDE × velocity` とします。velocityが0のNote Onもノートを更新し、出力振幅が0になります。
- Note Offは、両イベントにnoteIdがあればIDで照合します。どちらかが `-1` の場合はチャンネルと音高で照合します。
  IDがない同チャンネル・同音高の重複は識別できません。
- `OscillatorInput` は発振器回路の定常入力で、波形・目標周波数・目標実効値・ゲートを持ちます。
  `OscillatorState` は内部状態として次のサンプルの位相を保持します。イベント処理は入力だけを書き換えます。
- 各サンプルで「現在の入力と状態からサンプル生成 → 入力を用いて状態更新」を繰り返します。
  現段階ではエンベロープがないため、周波数・振幅の目標値を直接使用します。
  将来エンベロープを追加する場合、その現在値は `OscillatorState` 側で保持します。
  初期位相は0。ノートの追加・音程変更・ブロック境界で位相をリセットしません。
- Note Offでは即座に消音し、消音中は位相を止めます。次のNote Onで保持した位相から再開します。
  エンベロープやスムージングを持たないため、発音・消音・振幅変更でクリックが発生し得ます。
- `sampleOffset` の位置でイベントを反映します。同位置ではリスト順です。取得失敗、未対応の種類・バス、
  不正な値、範囲外または処理済み位置より前のイベントは無視します。
- イベントがないブロックでも発音を継続します。処理中は出力バッファがなくてもイベントを処理し、
  発音中の位相を進めます。0サンプルの場合はoffset 0のイベントだけを処理します。
- イベント入力バスは、有効化通知を省略するホストへの互換性対応として初期化時から有効にします。
  これは[全バスを初期状態で無効とするVST3仕様](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__vstBus.html)からの意図的な逸脱です。
  明示的な無効化には従い、処理を再開しても無効のままです。再初期化時は初期状態へ戻ります。
  音声出力バスはホストによる有効化を必要とします。
- 処理停止・無効化では消音しますが位相は保持します。処理停止中の0サンプルフラッシュではノートを受け付けません。
  ホストのトランスポート再生状態は参照しないため、停止中でもホストが処理とノートを送れば演奏できます。

内部では `OscillatorInput.waveform` の `Waveform` で波形を指定します。既定は `Sine` です。
波形を変更しても位相を引き継ぎ、次のNote Onでも選択した波形を維持します。

| 波形 | 最大ベロシティ時のピーク振幅 | 位相0からの形状 |
| --- | --- | --- |
| サイン波（Sine） | `0.5 × √2` ≈ 0.707107 | 0から上昇 |
| 三角波（Triangle） | `0.5 × √3` ≈ 0.866025 | 0から上昇、1/4周期で正のピーク |
| 矩形波（Square） | `0.5` | 前半は正、後半は負（デューティ比50%） |
| ノコギリ波（Sawtooth） | `0.5 × √3` ≈ 0.866025 | 負のピークから直線的に上昇 |

倍率は理想波形の1周期の実効値が0.5となる値です。有限個のサンプルで測定した実効値は、
周波数・サンプルレート・測定区間によって誤差が生じます。帯域制限・アンチエイリアシングは未実装で、
特に高音域では折り返し成分が発生します。

## 波形の選択・オートメーション・保存

ホストのVST3パラメーター表示で **Waveform** を操作します。画面上の表示形式はホストに依存します。
`IEditController` で列挙する離散パラメーターで、固定ParamIDは `0x57415645`、
`stepCount = 3`、フラグは `kCanAutomate | kIsList`、既定値は0です。

| 表示名 | 実単位の番号 | 正規化値 |
| --- | --- | --- |
| Sine | 0 | 0 |
| Triangle | 1 | 1/3 |
| Square | 2 | 2/3 |
| Sawtooth | 3 | 1 |

正規化値は4つの等幅区間へ量子化し、上端の1はSawtoothに含めます。
音声処理への変更は `ProcessData.inputParameterChanges` から受け取り、各変更点の `sampleOffset` で切り替えます。
変更点までは直前の波形を保持し、離散値の補間はしません。同位置ではパラメーターをキュー順に適用した後、
ノートをイベントリスト順に処理します。変更がないブロックでも最後の選択を維持します。
波形の即時切り替えでクリックが発生する場合があります。

音声バッファがない場合や処理停止中の0サンプルフラッシュでも、offset 0のパラメーター変更を受け付けます。
未知のParamID、不正な値、範囲外・後戻りする位置、取得に失敗した変更点は無視します。
Controllerの `setParamNormalized` は表示値を更新し、DSPの更新はホストが渡す変更キューを通じて行います。

保存・復元には標準の `IComponent::getState/setState` と `IBStream` を使用します。
保存する内容は12バイト（`SVST` の4バイト、形式バージョン1のu32、波形番号のu32。整数はlittle endian）です。
旧版の空の状態はSineとして読み込みます。未対応バージョン・不正な番号・途中切れは適用しません。
復元値は次の音声処理ブロックから適用し、Controllerは `setComponentState` で同期します。
位相や発音中のノートは保存・復元の対象にせず、Controller固有の保存状態は空です。
状態の受け渡しはatomicで行い、音声スレッドをロックしません。

## GUI

ホストのプラグイン画面にSine / Triangle / Square / Sawtoothの4ボタンを表示します。
VST3の `IPlugView` を直接実装し、WindowsではHWND、LinuxではX11EmbedWindowIDへ埋め込みます。
Linuxではホストが `IPlugFrame` 経由で `Linux::IRunLoop` を提供する必要があります。Waylandへの直接埋め込みは未対応です。
サイズは360×180物理ピクセルで固定です。自由なサイズ変更と
`IPlugViewContentScaleSupport` による表示倍率変更は未対応です。
初版の操作はマウスのみです。VST3のキー・ホイール処理は未処理を返します。

描画にはegui 0.37.0、egui_glow 0.37.0、baseview 0.4.1のOpenGLバックエンドを使用します。
対応ドライバーがあれば内蔵GPU・外付けGPUを使います。音声処理はCPUで行います。
ソフトウェアOpenGLでも動作検証していますが、ソフトウェアドライバーはバンドルへ同梱しません。
描画環境を初期化できない場合は画面の取り付けに失敗し、ホストのパラメーター操作と音声処理は引き続き使用できます。

接続処理は左マウスボタンと描画に限定し、クリップボード・URL起動・キーボード編集は実装しません。
GUIの操作はUI専用の32件キューからホストのUIスレッドへ配送し、
`IComponentHandler::beginEdit / performEdit / endEdit` を順に通知します。キュー満杯時は新しいクリックを破棄します。
DSPへの適用は従来どおりホストの変更キュー経由です。ホストからの値更新や状態復元は表示へ反映し、編集通知を送り返しません。
Windowsはbaseviewの子ウィンドウのタイマー、LinuxはVST3のIRunLoopから編集を配送します。
画面を閉じるとタイマーと描画資源を解放し、音声処理は継続します。

## ビルド

Rust 1.95以上が必要です。LinuxではCリンカー、WindowsではVisual Studio Build Toolsの
「C++によるデスクトップ開発」とWindows SDKを導入してください。
GUIの依存クレートはCargoがcrates.ioから取得し、バージョンはCargo.lockで固定します。
LinuxでのGUI実行にはX11、libxkbcommon-x11、OpenGL/EGLのランタイムが必要です。
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
            ├── THIRD_PARTY_NOTICES.txt
            └── dependencies/  # 依存ライブラリ・付属フォントの通知とindex.json
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
ホストの鍵盤やピアノロールからノートを送り、単音の発音と消音を確認してください。プラグインの画面を開くと、4つのボタンで波形を選択できます。
更新時はホストを終了してから、配置済みバンドルを差し替えてください。

## 無音時の診断

FL Studioでイベント入力バスの有効化通知を受け取らず、ノート処理を省略していた診断結果に対し、
通知なしでも受信する互換性対応を追加しました。修正後のサイン波はFL Studioで発音の動作報告があります。
通常は診断を無効にしています。診断時も音声処理中はatomicで回数・直近値を記録するだけで、
ファイル出力は処理停止後の `terminate` で行います。外部への送信は行いません。

Windowsでは新しいバンドルへ差し替え、FL Studioを完全に終了してから、
バンドルの `Contents/Resources` にある補助スクリプトをPowerShellで実行します。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\collect-diagnostics.ps1
```

通常のインストール先が1つなら自動で起動します。複数ある場合や独自の場所へインストールしている場合は、
表示された入力欄へ `FL64.exe` のフルパスを指定してください。
起動したFL StudioでSample VSTiを読み込み、ノートを鳴らしてからプラグインを削除し、FL Studioを終了します。
表示された一時フォルダ内の `sample-vsti-*.txt` が診断結果です。
プラグインのバージョン、ホスト呼び出し回数、バスの有効状態、処理を拒否した理由別の回数、
ノート受信数、最後のノート情報、非ゼロの音声を出力したブロック数を記録します。
`event_active` はプラグインが扱う有効状態で、初期化時の互換性対応も含みます。
ホストからの有効・無効通知の回数は `event_activation_calls` で区別できます。
音声データやプロジェクト名は記録しません。スクリプト以外から通常起動したFL Studioでは診断は有効になりません。

任意のホストでも、起動前に `SAMPLE_VSTI_DIAGNOSTICS_DIR` を既存の出力先ディレクトリへ設定すれば診断できます。
レポートがない場合は、診断対応版を読み込んでいるか、環境変数を設定した後にホストを新規起動したか、
プラグインを正常に削除・終了したかを確認してください。クラッシュや強制終了時のレポート保存は保証しません。

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

GUIの表示・クリックも検証する場合は `python scripts/check-abi.py --gui` を使用します。
LinuxではABIテストのコンパイルに `libx11-dev` が必要です。画面のない環境では次のように実行できます。

```sh
sudo apt-get install libx11-dev libxkbcommon-x11-0 libgl1-mesa-dri libegl1 xvfb xauth
LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a python3 scripts/check-abi.py --gui
```

WindowsのGUI検証にはOpenGL対応ドライバーが必要です。CIでは固定版のMesaをABIテスト実行ファイルの隣にだけ配置し、
ソフトウェア描画で検証します。このDLLは配布ZIPへ含めません。
GUIテストは表示・クリック・UIスレッドでの標準編集通知・保存復元・閉じて再表示する操作を確認します。

この検証はSDKのインターフェースで共有ライブラリをロードし、バージョン・ID・戻り値、100回のインスタンス生成・解放、
複数のブロック長、32/64ビットの発音・消音、音程変更・再発音での位相継承、
イベントのサンプル位置とノート照合、バッファ境界、同一バッファを指す出力、停止・再開とフラッシュを確認します。
波形パラメーターの列挙・表示・値変換、変更時刻と位相を含む実際の出力サンプル、
停止中のパラメーターフラッシュ、IBStreamの部分読み書き・保存復元・不正データの拒否も検査します。
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

Linuxでの確認済み環境: Linux x86_64、Rust 1.95.0 / 1.99.1、GCC 12.2、VST3 SDK 3.8.1
（インターフェース定義も3.8.1）。Rustテスト17件、ABI検証、Validator 47件が成功しています。
参照した公式SDKヘッダのライセンスはMITです。出典とライセンス全文は [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) を参照してください。
イベントバス有効化通知への互換性対応後、サイン波はFL Studioで発音の動作報告があります。
GUIの表示・クリック・復元・再表示はLinuxのXvfbとMesaで検証しています。
GUI追加後の実際のDAWでの表示・操作、および実GPUでの描画は未確認です。

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
| Build and test | Linux/WindowsそれぞれでMSRV（Rust 1.95.0）と最新stableのcheck・debug/releaseテスト・releaseビルド |
| Miri | 固定したnightlyでRustテストの未定義動作を検査 |
| Coverage | RustテストのLCOVレポートを生成・保存 |
| Dependency audit | cargo-denyで脆弱性・ライセンス・依存元・依存バージョンを監査 |
| VST3 ABI and Validator | Linux/Windowsそれぞれで公式SDKの固定版を使ったABI・GUI検証・Validatorを実行し、バンドルZIPとログを保存 |
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

GUI・ウィンドウ・描画・フォントの依存ライセンスは `deny.toml` で監査します。
依存を追加する際は、利用条件と通知ファイルを確認して許可リストも更新してください。本体の配布ライセンスはこの設定では決定しません。
ブランチ保護で全ジョブを必須にする場合は、集約チェック **CI result** を指定できます。

## 構成

- `build.rs`: Gitコミットと正式版指定からバージョン・ビルド情報を生成。
- `src/abi.rs`: 必要なABI型、OS別のID表現・戻り値、vtable定義。
- `src/lib.rs`: モジュールの公開関数とファクトリー。
- `src/plugin.rs`: インスタンス、バス、イベント・パラメーター処理、音声出力、Controller。
- `src/gui.rs`: VST3画面の直接実装とホストへの編集通知。
- `src/editor.rs`: 4つの波形ボタンのマウス入力とegui/OpenGL描画。
- `src/host.rs`: UIスレッドに限定したホスト参照管理。
- `src/parameters.rs`: 波形パラメーターの値変換、VST3変更キューとIBStreamの読み書き。
- `src/oscillator.rs`: 定常入力 `OscillatorInput` と内部状態 `OscillatorState` を分離した4波形オシレータ。
- `src/synth.rs`: 単音のノート照合と発音・消音。
- `src/diagnostics.rs`: 明示的に有効化するホスト呼び出し診断。
- `scripts/collect-diagnostics.ps1`: Windowsで診断を有効化してFL Studioを起動する補助スクリプト。
- `src/tests.rs`: Rust側のABIレイアウトと参照管理のテスト。
- `tests/gui-smoke.h`: 埋め込み画面とマウス操作の検証専用コード。
- `scripts/bundle-licenses.py`: 対象OSの依存ライブラリと付属フォントの通知収集。
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

依存ライブラリと付属フォントの通知全文・バージョン・取得元は、同ディレクトリの `dependencies/` と `index.json` に含めます。
公開クレートに通知ファイルがない場合の補完元は `licenses/upstream/` に記録しています。

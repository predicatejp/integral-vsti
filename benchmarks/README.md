# DSP速度検証

`dsp.rs`は本体のDSPモジュールを直接コンパイルする計測専用プログラムです。
プラグインの動作・公開API・依存関係は変更しません。測定結果は[リポジトリのWiki](https://github.com/predicatejp/sample-vsti/wiki)へ記録します。

Linux x86_64での実行例（Rust 1.95以上）:

```sh
mkdir -p target
rustc --edition=2024 -D warnings -C opt-level=3 -C panic=abort -C codegen-units=16 benchmarks/dsp.rs -o target/bench-dsp
taskset -c 0 target/bench-dsp 48000 > target/dsp-48000.csv
taskset -c 0 target/bench-dsp 96000 > target/dsp-96000.csv
taskset -c 0 target/bench-dsp 48000 '' --flush-subnormals > target/dsp-48000-ftz.csv
```

CPUを固定できない環境では`taskset -c 0`を外せますが、結果にその条件を記録してください。
第2引数でケース名の前方一致を指定できます（例: `48000 filter/`）。同時に複数の計測を走らせないでください。

ウォームアップ後、1バッチ20ms以上になる反復数を求め、7バッチの最小・中央値・最大をns/操作で出力します。
`unit=sample`は出力サンプル当たり、`operation`はノート処理・設定更新など1回当たりです。
最適化による計算削除を防ぐため`black_box`を使用し、測定区間内で確保・出力・時刻取得を行いません。
最小/最大もバッチの平均値であり、個々のサンプルの最悪時間ではありません。

`--flush-subnormals`は計測プロセスのスレッドだけでFTZ/DAZを有効にする比較実験です。
通常は開始時のMXCSRを保持し、条件を標準エラーへ表示します。本体には変更を加えません。

これはDSP単体のマイクロベンチマークです。VST3キュー走査、音声バッファ書き込み、GUI、DAW、
OSのスケジューリングを含めた実機オーディオの締め切りを保証するものではありません。

## 数値テーブルの比較実験

```sh
python3 benchmarks/run-lookups.py --cpu 0
```

RustとPython標準ライブラリだけを使います。`--cpu`を省略するとCPU固定を行いません。
`lookups.rs`は128ノートの基本周波数（f64、1KiB）と、正弦波の1/4周期を
64・256・1024・4096区間で線形補間する方式を比較します。各テーブルは区間数+1個の端点を保持します。
調律補正が0 centなら直接参照、それ以外は基本周波数に`2^(cent/1200)`を掛けます。

出力先は`target/lookup-experiment/`です。単体速度・誤差に加え、本体のコピーを作り、
ノート周波数の式と正弦波の式だけを差し替えてSynthesizer全体・Note Onを測定します。
元のソースが変わって置換できなくなった場合はエラーで停止します。本体のファイルは変更しません。
テーブルは同じRust実行環境で生成した定数配列をコピー側へ埋め込み、計測中の生成・初期化は行いません。

各ケースは4096回のウォームアップ、20ms以上×7バッチの最小・中央値・最大を記録し、
全体を2回測ります。シンセの入力はサイン波・Detune=30 cent・LowPass2・既定AHDSRで、
MIDI 48から連続する1音または32音を1秒進めてから計測します。48/96kHzを比較します。
`micro-*.csv`は単体、`<variant>-<rate>-<repeat>.csv`は音源全体の速度です。
`micro-*.meta`は1,048,613位相点と四半周期境界、全128ノート×−100〜100 cent（0.1刻み）の誤差です。
`audio-errors.csv`は各条件8192サンプルの差分、`*.f64le`はその比較に使った出力です。
誤差の基準は同環境の直接計算で、厳密解・聴感評価・THD測定ではありません。
非ゼロ調律の全有限f32範囲や、DAW・Windows/MSVCでの速度はこの実験では検証しません。

## 全7候補の追加・除外比較

```sh
python3 benchmarks/run-lookup-ablation.py --cpu 0
```

`none`（数値テーブルなし）、`all`（全適用）、`only-<候補>`（1候補だけ適用）、
`without-<候補>`（全適用から1候補だけ除外）の16構成を同じ本体のコピーから作ります。
候補はnotes=128、sine=1025、gain=7、cutoff=1201、detune=201、prewarp=4097、time=372値。
すべてf64で、sineは1/4周期、detuneは−100〜100 centの1 cent刻み、prewarpは0〜0.49の
正規化周波数を線形補間します。UI時間表は整数tickの換算だけに使用し、ホストの連続値の換算は維持します。

`lookup-ablation.rs`は`lookups.rs`の既存計測関数へ連結する追加実験用コードです。
48/96kHzで定常発音3条件、Note On2条件、毎サンプルのDetune/Cutoff/Resonance変更を伴う発音、
設定更新のみ、UI時間換算の計8ケースを測ります。各ケースの7バッチ中央値を、
実行順を固定seedでシャッフルした3ラウンドで比較します。初期化・テーブル生成・ノート設定・
1秒の予備発音は計測外で、ベンチマークの反復内では確保・出力を行いません。

生成物は`target/lookup-ablation/`です。`variants.json`が各構成の適用候補、`orders.json`が順序、
`<構成>-<レート>-<ラウンド>.csv`が速度、`audio-errors.csv`が直接計算との差分、
各構成の`verification.txt`が補間誤差の確認結果です。出力比較は48/96kHz×定常3条件・変調1条件で
各8192サンプルを使い、全フィルタタイプ・Qの両端・44.1/48/96kHzで有限出力も確認します。
近似係数の聴感・THD・安定性全般を保証する検証ではありません。
異なる構成のビルドによるコード配置・最適化の差も結果に含まれるため、対象を使用しないケースの
小さな速度差をテーブルの直接効果と解釈しないでください。

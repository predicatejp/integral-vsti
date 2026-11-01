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
`unit=sample`は出力1サンプル時刻当たり（ステレオのコンポーネントは左右1組）、`operation`はノート処理・設定更新など1回当たりです。
最適化による計算削除を防ぐため`black_box`を使用し、測定区間内で確保・出力・時刻取得を行いません。
最小/最大もバッチの平均値であり、個々のサンプルの最悪時間ではありません。

`--flush-subnormals`は計測プロセスのスレッドだけでFTZ/DAZを有効にする比較実験です。
通常は開始時のMXCSRを保持し、条件を標準エラーへ表示します。本体には変更を加えません。

これはDSP単体のマイクロベンチマークです。VST3キュー走査、音声バッファ書き込み、GUI、DAW、
OSのスケジューリングを含めた実機オーディオの締め切りを保証するものではありません。

## 改善ごとの比較

`dsp.rs`の第3引数`--scoped-ftz`は、本体と同じMXCSR保存・復元を256操作ごとに行います。
サンプル単位のケースでは256サンプルのブロック相当です。設定切替の費用を含めて計測します。
比較前後のコミットで同じケース・レート・CPU・フラグを指定し、逐次実行してください。
`--flush-subnormals`は従来どおり測定全体のFTZ/DAZ切替です。

Detuneの連続変調は`oscillator/Sine/u7/detune-changing`と`synth/p32/u7/detune`で測定します。
どちらも5〜79.925 centを0.075 centずつ毎サンプル更新します。後者は正弦波・LP2です。
定常入力のキャッシュ後に残るテーブル参照の効果を、同じ計測コードで比較してください。

本体の固定テーブルは`src/oscillator_tables.rs`（f64、Detune 201値・四半周期正弦波1025値、合計9808 bytes）です。
初期化や確保は音声処理内で行いません。正弦波の実効値補正は行いません。
再生成が必要なときだけ次を実行します（通常のビルドには不要）:

```sh
rustc --edition=2024 scripts/generate-oscillator-tables.rs -o target/generate-oscillator-tables
target/generate-oscillator-tables > src/oscillator_tables.rs
```

一度限りのテーブル比較実験の条件・結果は[Wiki](https://github.com/predicatejp/sample-vsti/wiki)に記録しています。
実験用プログラムはこのディレクトリには含めません。

Width/Panの計測は`synth/p32/u7/width`（Width=100%固定）、`width-changing`（1000サンプルで0〜100%）、`pan-changing`（Width=100%、1000サンプルでPanを−100〜＋100%）です。1/8/32ポリフォニー・1/7ユニゾンの各組み合わせで同じケースを実行できます。

Noiseは`oscillator/Noise/`と`synth/p32/u7/noise`などで測定します。後者は32音・7ユニゾン・Detune=30 cent・LP2です。乱数の初期化は測定区間の前に行います。

ディストーション単体は`distortion/`で計測します。ステレオ入力、Gain=12 dB、None/Soft/Hard、ADAAなし／1次／2次のWet処理を比較します。

SoftのADAA原始関数は`src/distortion/table.rs`（1025点×3列のf64、24,600 bytes）を全Voiceで共有します。
再生成は次のコマンドで行います。

```sh
rustc --edition=2024 scripts/generate-distortion-table.rs -o target/generate-distortion-table
target/generate-distortion-table > src/distortion/table.rs
```

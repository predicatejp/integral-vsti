# DSP速度検証

`dsp.rs`は本体のDSPモジュールを直接コンパイルする計測専用プログラムです。
プラグインの動作・公開API・依存関係は変更しません。測定結果は[リポジトリのWiki](https://github.com/predicatejp/sample-vsti/wiki)へ記録します。

## 標準性能回帰スイート

`standard.rs`は通常の演奏を模した固定21ケースです。全パラメーターを明示し、製品の初期値変更から独立させています。
既定条件は48 kHz・256サンプル、起動後に実時間30秒以上のburn-inを行い、全ケースの巡回を10試行実施します。
burn-in中も選択したケースのDSPを繰り返し動かします。試行間は5秒間待機します。
各試行の各ケースでは、新しい音源で同じ2秒分の演奏をウォームアップし、続く4秒分を計測します。
この演奏時間のウォームアップと、実時間で行う起動後のburn-inは別の処理です。
同じCPU・ビルド条件で変更前後を逐次実行してください。

```sh
# ケース一覧
python3 scripts/bench-standard.py --list
# 変更前の基準値を保存（Linuxでは論理CPUを固定）
python3 scripts/bench-standard.py --cpu 0 --output target/performance/baseline.json
# 変更後を同じ条件で比較
python3 scripts/bench-standard.py --cpu 0 --baseline target/performance/baseline.json --output target/performance/candidate.json
```

`--rate 44100|48000|96000`、`--block 1..2048`、`--rounds 10..100`、`--seconds 4..60`で条件を指定できます。
`--burn-in-seconds`は30秒以上、`--interval-seconds`は1秒以上を指定できます。試行は最低10回です。
既定の実行時間にはburn-inと試行間待機だけで最低75秒が必要で、これにDSPの実行・検査時間が加わります。
`--case auto-`などの接頭辞で対象を絞れます。Windowsでは`--cpu`を省略して実行し、必要に応じてOS側でCPUを固定してください。

結果JSONにはコミット・作業ツリーの変更有無・ケース定義のハッシュ・Rust/OS/CPU・CPU affinity・計測条件を保存します。
`cases.*.samples_ns`には試行順の全測定値、`trials`には起動からの試行開始・終了時刻（ns）を保存します。
不足した試行、burn-in不足、待機不足、不一致のケース列や出力は拒否します。古い測定方式のJSONとは比較できないため、基準値を取り直してください。
比較時は同じケース定義、環境、コンパイルオプション、計測条件を要求し、異なれば計測前に終了します。
1ケースでも中央値が基準値より増えた場合は、要再検証として終了コード1を返します。全ケースが同等以下なら0です。
固定の悪化許容率は設けず、ほかのケースの改善との相殺や、ケース全体の平均による合格判定を行いません。
わずかな増加でも自動合格にはしませんが、それだけで真の性能低下と断定せず、全試行のばらつきと再測定結果を確認してください。
計測時間はステレオ1サンプル（左右1組）あたりのnsで表示します。
偶数試行の中央値は中央2値の平均とし、外れ値を自動除外しません。
最小・最大も併記するので、負荷増加時は同じ条件で再実行し、ばらつきと区別してください。
CPUクロック・温度・他プロセスの負荷は実行間で揃えます。失敗した計測を新しい基準値へ自動で置き換えません。
集計・試行検証の回帰テストは`python3 tests/bench-standard.py`で実行します。

### 音色と演奏の固定ケース

全ケースのPhaseはZero。ベロシティは0.65〜0.8、Masterは−6 dBです。
基本AHDSRはA=10 ms、H=0、D=120 ms、S=0.7、R=100 ms。Osc1のVolは0 dB、表で変更していないOsc2/3はSine・Tune=0・Vol=−∞、変調量は0です。
Cutoffはノート基準のcentで指定します。左右の音声出力と、ノートオン／オフによるエンベロープ・フィルタ残響を計測に含めます。

| ケース | 音色・設定 | 演奏 |
| --- | --- | --- |
| `sine-mono` | Sine、Unison 1、Filterなし | モノのメロディー |
| `saw-bass` | Saw、LP2、Cutoff +1200、Q=1.2、カットオフAHDSRのD=250 ms・S=0・Amount +2400、音量A=5 ms・D=180 ms・S=0.45 | 低音リフ |
| `triangle-pluck` | Triangle、Poly 4、LP1、Cutoff +3600、音量A=2 ms・D=250 ms・S=0・R=180 ms | アルペジオ |
| `square-lead` | Square、Unison 2、Detune 7 cent、LP2、Cutoff +2400、Q=0.9 | モノのメロディー |
| `saw-chords` | Saw、Poly 8、Unison 3、Detune 9 cent、Width 60%、LP2、Cutoff +3600、Q=0.8、音量A=15 ms・D=200 ms・S=0.6・R=400 ms | 4音コード、releaseの重なりあり |
| `slow-pad` | Saw、Poly 8、Unison 5、Detune 12 cent、Width 80%、LP2、Cutoff +2400、音量A=300 ms・D=500 ms・R=800 ms、カットオフA=600 ms・D=500 ms・Amount +1200 | 4音コード、releaseの重なりあり |
| `three-osc-keys` | Poly 8、LP1、Saw −6 dB＋Triangle −12 dB/−1200 cent＋Sine −18 dB、音量R=400 ms | 4音コード |
| `phase-mod-bell` | Sine、Poly 4、Osc2 Tune +1200・変調量30%、Osc3 Tune +1900・変調量10%、音量A=2 ms・D=1000 ms・S=0・R=300 ms | アルペジオ |
| `soft-x1-off` | ベース＋Soft、Gain +9 dB、Mix 40%、Rate x1、Smooth None | 低音リフ |
| `soft-x2-first` | 同上、Rate x2、Smooth 1st | 低音リフ |
| `soft-x4-second` | 同上、Rate x4、Smooth 2nd | 低音リフ |
| `hard-x2-second` | リード＋Hard、Gain +6 dB、Mix 25%、Rate x2、Smooth 2nd | モノのメロディー |
| `idle-pad` | パッドの設定、ノートなし | 無音時の処理 |

120 BPM相当で、メロディーは500 msごとに400 ms発音、低音リフは250 msごとに190 ms発音、アルペジオは250 msごとに160 ms発音します。
コードはC系 `[48,55,60,64]` とF系 `[53,60,65,69]` を2000 msごとに切り替え、1750 msでノートオフします。
Monoは1枠、プラック・ベルは4枠、コード・パッドは8枠の循環割り当てです。同時刻のノートはNote Off→Note Onとし、パラメーターを先に適用します。

### オートメーションケース

各ケースは1パラメーターだけを4秒周期の三角波で往復させ、何の更新が負荷を生むか比較できます。

| ケース | 対象・範囲 | 基本音色 |
| --- | --- | --- |
| `auto-cutoff` | Cutoff +1200〜+4800 cent | コード |
| `auto-resonance` | Q=0.7〜2.5 | コード |
| `auto-drive` | Gain +3〜+15 dB | コード＋Soft、Rate x2、Smooth 1st、Mix 40% |
| `auto-mix` | Mix 10〜50% | コード＋Soft、Rate x2、Smooth 1st、Gain +9 dB |
| `auto-pan` | Pan −40〜+40% | コード |
| `auto-volume` | Master −12〜0 dB | コード |
| `auto-width` | Width 20〜80% | コード |
| `auto-phase-mod` | Osc2の位相変調量10〜40% | ベル |

ブロック末尾に1変更点が届く条件をモデル化します。連続パラメーターは前ブロック末尾の値から正規化値を線形補間し、
Cutoffは本体の`holds_between_points`に従い変更点まで保持します。Gainの内部サンプル用の次境界値も、現在ブロックの既知点までで評価します。
曲線とノート列は計測前に準備し、計測中は設定更新・ノート処理・DSP・ステレオ出力バッファ書き込みを行います。
これはSynthesizer入口での回帰試験で、VST3キューの走査・GUIは`check-abi.py`などの別の検証経路で扱います。

毎回の出力について有限性・発音／無音・反復のビット一致を検査し、ハッシュ・ピーク・RMSも保存します。
比較時の`audio=changed`は波形差の確認を促す表示で、性能の合否とは分けます。数値計算を変更した場合は音声側の回帰テストも実行します。

## コンポーネント別マイクロベンチマーク

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

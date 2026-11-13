# DSP速度検証

`dsp.rs`は本体のDSPモジュールを直接コンパイルする計測専用プログラムです。
プラグインの動作・公開API・依存関係は変更しません。測定結果は[リポジトリのWiki](https://github.com/predicatejp/integral-vsti/wiki)へ記録します。

## 標準性能回帰スイート

`standard.rs`は現在の音源構成を対象とする41ケース（固定設定25・連続操作16）です。発振器・音量／カットオフAHDSR・各Voiceのフィルタ、Voice合成後の3エフェクトスロット・EQ・Limiter・Masterを通して測定します。全パラメーターを明示し、製品の初期値変更から独立させています。単体エフェクトは先頭スロット、連結ケースは3スロットをすべて使います。
変更前・変更後のGit worktreeを指定し、同一のハーネス・コンパイラー・オプションでそれぞれの`src/`のコピーをビルドします。
基準値JSONを後日の測定と比較する方式は使わず、同じ実験の同じブロック内で変更前後を対応付けます。
固定設定のケースはノート境界で区切って`Synthesizer::generate_samples`を呼び、オートメーション中は各サンプルの設定を適用して`generate_sample`を呼びます。
Pitch Bendは`set_pitch_bend`を使い、テンポは各音声ブロックで120 BPMを通知します。

[現行41ケースの構成と検証結果](https://github.com/predicatejp/integral-vsti/wiki/Standard-performance-suite-2026-11-13)をWikiにまとめています。

### 準備・実行

統計解析にはPython 3.11以上、SciPy 1.17.1とNumPy 2.4.6を使います。
`benchmarks/requirements.txt`で固定し、PyPIから取得します。追加のoptional extrasは使用しません。
[SciPy](https://scipy.org/about/)・[NumPy](https://numpy.org/doc/stable/license.html)本体はBSD-3-Clauseです。
ホイール同梱の数値ライブラリの通知は各パッケージのライセンスファイルに記載されています。

```sh
python3 -m venv target/bench-venv
target/bench-venv/bin/python -m pip install -r benchmarks/requirements.txt
# 比較元を固定したworktreeを用意する（<before-commit>は比較元のコミットID）
git worktree add --detach target/perf-before <before-commit>
# ケース一覧
target/bench-venv/bin/python scripts/bench-standard.py --list
# 変更前後をまとめて測定
target/bench-venv/bin/python scripts/bench-standard.py --before target/perf-before --after . --cpu 0 --output target/performance/paired.json
```

Windowsでは仮想環境の`Scripts/python.exe`を使用し、`--cpu`を省略します。必要に応じてOS側でCPUを固定してください。
CPUクロック・温度・他プロセスの負荷をできるだけ一定に保ち、測定は逐次実行します。

### ブロックと実行順

- 既定は48 kHz・256サンプル、10ブロック。各ブロックに、選択した全ケースの変更前・変更後を1回ずつ含めます。
- ブロック内の「ケース×変更前後」全実行をシャッフルします。順序を選り好みせず、seedを実行前に決めて保存します。
- 各実行は独立したプロセスで、音源準備2秒分＋測定と同じ演奏4秒分を最低1周処理してburn-inします。測定区間で使う処理経路も事前に実行します。
- 音源を初期化し直し、2秒分を生成して同じ演奏位置・履歴を作ります。この最終準備もburn-inの実時間に含めます。実時間の下限は既定10msで、短い場合は全体をもう1周処理します。
- 続く4秒分の演奏を計測します。**音声の4秒分を生成する時間は実時間4秒ではありません。** 48 kHzなら192,000個のステレオサンプルを生成する所要時間です。burn-in・音源初期化・検査は計測区間の外です。
- ブロック終了後に5秒間クールタイムを設けます。最初のブロックの前と最後のブロックの後には待機しません。
- 全ブロックが完了してからケースごとに解析します。途中のp値を見て打ち切る、外れ値を自動で除外する、都合のよいseedを選ぶ運用は行いません。

`--blocks`は10以上、`--burn-in-ms`は10ms以上、`--cooldown-seconds`は3秒以上を指定できます。
`--block 1..2048`はホストの音声バッファ長、`--blocks`は統計上のブロック数です。
`--rate 44100|48000|96000`、`--audio-seconds 4..60`で演奏条件を指定します。
`--case auto-mix --case hard-lead`のように接頭辞を繰り返して対象を選べます。対象ケースとブロック数は測定前に決めます。
`--seed`で同じ順序を再現できます。省略時は新しいseedを生成します。実際の全順序も保存します。
burn-inの10msは実時間の下限で、上限ではありません。最低1周と最終準備を終えるまで処理するため、重いケースほど長くなります。
音声の4秒分はノートオン／オフ・リリース・4秒周期のオートメーションを測定範囲に含めるため維持します。
[burn-in短縮の比較実験と実時間の内訳](https://github.com/predicatejp/integral-vsti/wiki/Performance-duration-2026-11-02)を参照してください。

### 統計量と解釈

ケースごとに、ブロック番号をキーとして `d_i = after_i - before_i` を計算します。実行順や速度順で値を並べ替えて対応付けません。
表示・JSON保存する内容は次のとおりです。

- ブロック数N、変更前・変更後・対応差それぞれの平均、標本標準偏差、中央値、最小・最大（ns／ステレオ1サンプル）。
- 対応のある両側t検定の `t = mean(d) / (sd(d) / sqrt(N))`、自由度N−1、未補正p値。
- ケースごとの平均差の95%信頼区間（多重比較補正前のpointwise区間）。
- 全選択ケースを1つの比較族としたHolm補正p値。有意水準は5%。補正p値が0.05未満なら、差の符号から`slower`／`faster`を表示。
- 差のShapiro–Wilk検定p値とラグ1相関係数を、分布・時系列の診断情報として表示。

`not_detected`は「差を検出できず」であり、「同等」「悪化なし」「採用可」を証明しません。
差の標準偏差が0のときはt検定を定義できないため、t・p・信頼区間をnull、結果を`undefined`とします。
`slower`または`undefined`が1件でもあれば終了コード1。それ以外の0は測定・解析の完了を表し、採用の自動承認ではありません。
他ケースの改善で有意な悪化を相殺しません。効果量と信頼区間を確認し、対象とした改善より大きな悪化は採用前に再検討します。

対応t検定は**ブロックごとの差の独立性と、おおむね正規的な分布**を前提とします。
元の前後データに同じ分散を要求する検定ではありません。クールタイムと無作為化は偏りを減らす設計であり、独立性の証明ではありません。
ケース数が多いブロックでは対応する2実行の時刻が離れます。実行時刻・順序・差の系列から時間変動や持ち越し効果を確認してください。
10ブロックでは分布や相関を見抜く力にも限界があります。Shapiroの大きなp値や小さなラグ1相関だけで前提を成立済みとせず、
診断結果を使って有意になるまでデータや検定を選び直さないでください。必要なブロック数は別の予備実験で決め、確認実験は固定した計画で実行します。

数式と設計は[NISTの対応比較](https://www.itl.nist.gov/div898/handbook/prc/section3/prc311.htm)、
[NISTの無作為化ブロック](https://www.itl.nist.gov/div898/handbook/pri/section3/pri332.htm)、
[Holm補正のR公式解説](https://stat.ethz.ch/R-manual/R-devel/library/stats/html/p.adjust.html)を参照。
検定と信頼区間は[SciPyのttest_rel](https://docs.scipy.org/doc/scipy-1.17.0/reference/generated/scipy.stats.ttest_rel.html)を使用します。

### 保存・検証

JSONにはソースのコミット・変更有無・ソースとバイナリのハッシュ、共通ハーネスのハッシュ、Python／SciPy／NumPy／Rust・OS・CPU、
CPU affinity、seed、全実行の順序・開始終了時刻・burn-in実時間／周回数・本測定の実時間（`measurement_ns`）、ブロックごとの測定値、音声ハッシュ・ピーク・RMS、全解析結果を保存します。
同じ実装・ケースの反復出力は一致を要求し、変更前後の音声一致は`audio_same`で別に記録します。
ペア不足・重複・不正値は検定前に拒否します。旧方式の独立した測定JSONを対応ペアとして再利用しません。
回帰テストは次のコマンドで実行します。Rust側ではケース設定、レガート／残響のイベント列、値保持と補間、分割生成の時刻継続を検査します。Python側では無作為化・対応付け・統計解析・burn-inを検査します。

```sh
rustc --edition=2024 -D warnings -C opt-level=2 --test benchmarks/standard.rs -o target/standard-tests
target/standard-tests suite_tests:: --test-threads=1
target/bench-venv/bin/python tests/bench-standard.py
```

### 音色と演奏の固定ケース

PhaseはZero、PortamentoのケースはKeepです。ベロシティは0.65〜0.8、Masterは−6 dB、Pitchの適用幅は200 centです。Amp Sensitivityは100%、Filter Amount Sensitivityは`saw-bass`で75%、その他は0%です。
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
| `portamento-lead` | リード＋Portamento 120 ms、Phase Keep、Poly 1 | 500 msごとに550 ms発音し、50 ms重ねる |
| `soft-bass` | ベース＋Soft、Gain +9 dB、Mix 40%、ADAA2 | 低音リフ |
| `hard-lead` | リード＋Hard、Gain +6 dB、Mix 25%、ADAA2 | モノのメロディー |
| `compressor-bass` | ベース＋Compressor：Threshold −18 dB、Ratio 4、A=10 ms、R=100 ms、Knee 6 dB、Makeup 0 dB、Mix 100% | 低音リフ |
| `compressor-chords` | コード＋同じCompressor | 4音コード |
| `delay-lead` | リード＋Delay：1/8、Feedback 35%、Mix 25%、Dual | モノのメロディー |
| `ping-pong-keys` | ベル＋Delay：1/8 D、Feedback 40%、Mix 30%、PP-L | アルペジオ |
| `reverb-pad` | パッド＋Reverb：Decay 2.5 s、Damping 5000 Hz、Size 110%、Mix 25% | 4音コード |
| `chorus-pad` | パッド＋Chorus：Time 18 ms、Depth 2.5 ms、Rate 0.6 Hz、Detune 0.03、Feedback 0%、Mix 35% | 4音コード |
| `flanger-lead` | リード＋Chorus：Time 2 ms、Depth 1 ms、Rate 0.3 Hz、Detune 0.02、Feedback 65%、Mix 40% | モノのメロディー |
| `phaser-keys` | コード＋Phaser：Center 1000 Hz、Depth 1200 cent、Rate 0.5 Hz、Feedback 50%、Mix 50% | 4音コード |
| `eq-keys` | コード＋EQ：Low 180 Hz/+3 dB、Mid 1200 Hz/−4 dB/Q=1.2、High 6000 Hz/+2 dB | 4音コード |
| `limiter-chords` | コード＋Osc1 Vol +6 dB、Limiter On | 4音コード、ゲイン抑制が働く入力 |
| `chain-pad` | パッド＋Chorus → Delay → Reverb → EQ | 4音コード、releaseの重なりあり |
| `chain-lead` | リード＋Distortion → Phaser → Compressor → EQ → Limiter | モノのメロディー |
| `release-tail` | パッド＋Reverb（`reverb-pad`と同設定） | 最初の4音コードを1.75秒で離し、2秒以降のreleaseと残響を測定 |
| `idle-pad` | パッドの設定、ノートなし | 無音時の処理 |

連結ケースの設定は以下のとおりです。各処理の順序・パラメーターはスロット位置に固定します。

- `chain-pad`：Chorus 20 ms/Depth 2 ms/Rate 0.7 Hz/Detune 0/Feedback 0%/Mix 30%、Delay 1/8 D/Feedback 30%/Mix 20%/Cross、Reverb 2.5 s/5000 Hz/Size 100%/Mix 20%。EQは250/1000/4000 Hz、Gain −3/+2/−2 dB、Mid Q=1。
- `chain-lead`：Soft/Gain +9 dB/Mix 40%、Phaser 1000 Hz/1200 cent/0.5 Hz/Feedback 50%/Mix 30%、Compressor −12 dB/Ratio 3/A=10 ms/R=100 ms/Knee 6 dB/Makeup +2 dB/Mix 100%。EQは250/1000/4000 Hz、Gain −3/0/+2 dB、Mid Q=1。Limiter On。

120 BPM相当で、メロディーは500 msごとに400 ms発音、低音リフは250 msごとに190 ms発音、アルペジオは250 msごとに160 ms発音します。
コードはC系 `[48,55,60,64]` とF系 `[53,60,65,69]` を2000 msごとに切り替え、1750 msでノートオフします。
Monoは1枠、プラック・ベルは4枠、コード・パッドは8枠の循環割り当てです。同時刻のノートはNote Off→Note Onとし、パラメーターを先に適用します。

### オートメーションケース

各ケースは1パラメーターだけを4秒周期の三角波で往復させ、何の更新が負荷を生むか比較できます。

| ケース | 対象・範囲 | 基本音色 |
| --- | --- | --- |
| `auto-cutoff` | Cutoff +1200〜+4800 cent | コード |
| `auto-resonance` | Q=0.7〜2.5 | コード |
| `auto-cutoff-amount` | Cutoff Amount 0〜2400 cent | コード |
| `auto-drive` | Gain +3〜+15 dB | コード＋Soft、ADAA2、Mix 40% |
| `auto-mix` | Mix 10〜50% | コード＋Soft、ADAA2、Gain +9 dB |
| `auto-pan` | Pan −40〜+40% | コード |
| `auto-volume` | Master −12〜0 dB | コード |
| `auto-width` | Width 20〜80% | コード |
| `auto-phase-mod` | Osc2の位相変調量10〜40% | ベル |
| `auto-pitch-bend` | ベンド入力−1〜+1（適用幅200 cent） | リード、50 ms重なるノート切替 |
| `auto-eq-frequency` | Mid Frequency 500〜2500 Hz | `eq-keys` |
| `auto-delay-feedback` | Feedback 20〜55% | `delay-lead` |
| `auto-reverb-mix` | Mix 10〜40% | `reverb-pad` |
| `auto-chorus-depth` | Depth 1〜4 ms | `chorus-pad` |
| `auto-phaser-center` | Center 500〜2000 Hz | `phaser-keys` |
| `auto-compressor-threshold` | Threshold −30〜−12 dB | `compressor-chords` |

ブロック末尾に1変更点が届く条件をモデル化します。連続パラメーターは前ブロック末尾の値から正規化値を線形補間し、
Cutoffは本体の`holds_between_points`に従い変更点まで保持します。Pitch Bendは音色設定の配列と分け、専用の入力へ連続値を渡します。
曲線とノート列は計測前に準備し、計測中は設定更新・ノート処理・DSP・ステレオ出力バッファ書き込みを行います。
測定の入口はSynthesizerの生成・設定メソッドです。

毎回の出力について有限性・発音／無音・反復のビット一致を検査し、ハッシュ・ピーク・RMSも保存します。
`audio_same=false`は波形差の確認を促す表示で、時間差の検定とは分けます。数値計算を変更した場合は音声側の回帰テストも実行します。

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

一度限りのテーブル比較実験の条件・結果は[Wiki](https://github.com/predicatejp/integral-vsti/wiki)に記録しています。
実験用プログラムはこのディレクトリには含めません。

Width/Panの計測は`synth/p32/u7/width`（Width=100%固定）、`width-changing`（1000サンプルで0〜100%）、`pan-changing`（Width=100%、1000サンプルでPanを−100〜＋100%）です。1/8/32ポリフォニー・1/7ユニゾンの各組み合わせで同じケースを実行できます。

Noiseは`oscillator/Noise/`と`synth/p32/u7/noise`などで測定します。後者は32音・7ユニゾン・Detune=30 cent・LP2です。乱数の初期化は測定区間の前に行います。

ディストーション単体は`distortion/`で計測します。ステレオ入力、Gain=12 dB、None/Soft/Hard、固定ADAA2のWet処理を比較します。

SoftのADAA原始関数は`src/distortion/table.rs`（1025点×3列のf64）を元に、
コンパイル時に区間ごとの積分多項式係数（1024区間×5列、40,960 bytes）へ変換し、全Voiceで共有します。
実行時はこの係数を直接参照し、区間ごとの係数計算を省きます。
再生成は次のコマンドで行います。

```sh
rustc --edition=2024 scripts/generate-distortion-table.rs -o target/generate-distortion-table
target/generate-distortion-table > src/distortion/table.rs
```

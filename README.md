# sample-vsti

RustでVST3のAPI・ABIを直接実装する、簡易ソフトウェア音源です。
最大32個の独立オシレータで、ノートオン・オフに応じて発音・消音します。
VST3パラメーターからサイン波・三角波・矩形波・ノコギリ波・ホワイトノイズを選択できます。
VST3用のバインディングやC/C++ラッパーは使用しません。GUIにはeguiを使用します。

## 現在の対応範囲

| 項目 | 内容 |
| --- | --- |
| OS・アーキテクチャ | Linux x86_64（GNU）、Windows x86_64（MSVC） |
| ホスト上の表示名 | Sample VSTi |
| 分類 | Instrument / Synth |
| 音声入力・出力 | 入力なし、ステレオ出力1バス |
| イベント | 入力1バス・16チャンネル。Note On / Note Offを処理（全チャンネル合計で最大32音） |
| サンプル形式 | float32 / float64 |
| サンプルレート | 44.1 kHz以上の有限値。範囲外はVST3の`setupProcessing`で拒否 |
| ホスト公開パラメーター・独自GUI | Osc1/2/3 Waveform（5択）・Volume、Osc2/3 Tune・FM、Attack / Hold / Decay / Sustain / Release、Unison / Detune / Width / Polyphony、Filter / Cutoff / Resonance、Phase（3択）、Volume / Pan、Cutoff AHDSR / Amount、Clipper type / Gain / Mixを公開。eguiでも操作可能 |
| 状態保存 | 波形・エンベロープ・ユニゾン・Polyphony・フィルタ・Phase・Volume・カットオフエンベロープ・Width・Pan・3発振器の設定をVST3のIBStreamで保存・復元 |
| レイテンシー・テール | 全方式で69サンプル。IIRフィルタの残響を打ち切らないようテールは保守的にkInfiniteTailを返す |

ProcessorとControllerは、同一インスタンスから取得できます。
CC、ピッチベンド、ノートエクスプレッションは未実装です。
macOS・Windows ARM64・32ビット環境は未対応です。

## 発音と位相

- 有効なNote Onを次のオシレータへ循環順に割り当て、周波数と目標振幅を更新します。枠の再使用時には古いノートを置き換え、古いノートには戻りません。
- 音程は12平均律（ノート番号69 = 440 Hz）とイベントのtuning（セント）で決めます。
  基準実効値を `BASE_RMS_AMPLITUDE = 0.5` と定義し、全波形で目標実効値を
  `BASE_RMS_AMPLITUDE × velocity` とします。velocityが0のNote Onもノートを更新し、attack時間をかけて目標振幅0へ移行します。
- Note Offは、両イベントにnoteIdがあればIDで照合します。どちらかが `-1` の場合はチャンネルと音高で照合します。
  同じID、またはIDがない同チャンネル・同音高が複数枠にあれば、全一致をreleaseします。上書き済みの旧IDへのNote Offは新しいノートを消しません。
- `OscillatorInput` は発振器回路の定常入力で、波形・位相モード・ユニゾン数・Detune・目標周波数・目標実効値・発音期間・ノート世代番号を持ちます。
  `OscillatorState` は位相と乱数の状態を保持し、Voiceが3発振器と共通の音量用`Envelope`を所有します。現在値・attack開始値・経過サンプル数・ノートオフ位置は`EnvelopeState`が保持し、イベント処理は入力だけを書き換えます。
- 各サンプルで「現在の入力と状態からサンプル生成 → 入力を用いて状態更新」を繰り返します。
  エンベロープの計算はサンプル数をミリ秒へ変換して行い、位相と時間の更新はサンプル生成後です。
  初期位相は0。Note On時の位相はPhaseの選択に従います。音程変更・ブロック境界では位相を保持します。
- Note Offではreleaseへ移行し、終了後に位相を止めます。release=0msなら即時に消音します。
  再Note Onではreleaseを挟まず、その時点の実効振幅から新しい目標へattack時間全体を使って移行します。
  位相はPhaseの選択に従います。周波数・波形の即時変更や0ms設定ではクリックが発生し得ます。
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

PhaseはVoice内の3発振器に共通するNote On時の位相を選ぶ3択パラメーターです（ParamID `0x50484153`、初期値Keep）。

| 選択 | Note On時の動作 |
| --- | --- |
| Keep（維持） | 割り当て先オシレータの直前の位相を引き継ぐ |
| Zero（ゼロ） | 割り当て先オシレータの全ユニゾン位相を0にする |
| Random（ランダム） | 全ユニゾン位相をそれぞれ独立に0以上1周期未満へ設定する |

選択の変更は次のNote Onから適用し、同じサンプル位置ではパラメーターを先に反映します。
位相とNoiseの疑似乱数には共通のxoshiro256+コンポーネントを使用し、用途ごとに独立した状態を持ちます。
シードはオシレータ作成時に標準ライブラリのRandomStateから取得します。
同じ乱数状態とノート入力からは同じ位相列を生成し、新しく作成したインスタンスでは異なる位相列になります。

内部では `OscillatorInput.waveform` の `Waveform` で波形を指定します。既定は `Sine` です。
波形を変更しても位相を引き継ぎ、次のNote Onでも選択した波形を維持します。

| 波形 | 最大ベロシティ・エンベロープ最大時のピーク振幅 | 位相0からの形状 |
| --- | --- | --- |
| サイン波（Sine） | `0.5 × √2` ≈ 0.707107 | 0から上昇 |
| 三角波（Triangle） | `0.5 × √3` ≈ 0.866025 | 0から上昇、1/4周期で正のピーク |
| 矩形波（Square） | `0.5` | 前半は正、後半は負（デューティ比50%） |
| ノコギリ波（Sawtooth） | `0.5 × √3` ≈ 0.866025 | 負のピークから直線的に上昇 |
| ホワイトノイズ（Noise） | `0.5 × √3` ≈ 0.866025 | 毎サンプル別の一様乱数 |

表はVol=0 dB・無変調時の出力です。波形の倍率は発振器の外で適用し、理想波形の1周期の実効値を0.5に揃えます。有限個のサンプルで測定した実効値は、
周波数・サンプルレート・測定区間によって誤差が生じます。帯域制限・アンチエイリアシングは未実装で、
特に高音域では折り返し成分が発生します。

## ホワイトノイズ

Noiseはxoshiro256+ 1.0の上位53bitから一様乱数`u ∈ [0,1)`を生成し、
`2u−1`の生波形を出力し、独立した音量処理で`√3 × amplitude`を掛けます。ベロシティ・エンベロープ最大時の理論実効値は0.5です。
乱数コンポーネントはOscillatorStateが所有し、乱数の状態更新は固定長の整数演算で行います。
位相用とは別の状態を進めるため、PhaseのKeep・Zero・Randomでノイズ列はリセットされません。
同じ初期乱数状態・入力・サンプル数なら同じ列を生成し、初期シードはインスタンスごとに取得します。

Detune=0ではノイズ源1個にまとめ、Widthは無効です。Detune>0ではUnison数の別々の乱数値を生成し、
既存のequal-power定位と`1/√N`の倍率で合成します。Detuneの正の値やノート音程はノイズ列の生成速度に影響しません。
ノート音程はフィルタの基準周波数に使います。Pan、音量・カットオフ用AHDSR、フィルタは他の波形と共通です。
ノートオンでは乱数列を継続し、リリース完了・処理停止後は更新を止めます。

## Voiceごとのフィルタ

音源は `Synthesizer → Voice → Oscillator` の階層です。各コンポーネントは専用のStateを所有します。各VoiceはOsc1・Osc2・Osc3とそれぞれの定常入力を保持します。Osc1はユニゾンを生成し、Osc2/3は各1発振器です。各発振器のVolumeを適用して加算し、共通の音量AHDSR、フィルタ、ディストーションの順に通した後、全Voiceを左右別に加算します。フィルタの係数は左右で共用し、積分器の履歴は独立させます。
Stateはデータ保持専用とし、所有コンポーネントのメソッドが処理とStateの更新を担当します。
音声関数は1サンプルを`*_sample`、サンプル系列を`*_samples`で区別します。

| パラメーター | 範囲 | 初期値 | ParamID |
| --- | --- | --- | --- |
| Filter | None / LowPass1 / LowPass2 / HighPass1 / HighPass2 / BandPass1 / BandPass2 | LowPass1 | `0x46545950` |
| Cutoff | ノート周波数から−2400〜9600 cent（−2〜＋8 oct） | ＋4800 cent（＋4 oct） | `0x46435554` |
| Resonance | Q=0.5〜10 | 1 | `0x46524553` |

GUIではFilterを横並びのドロップダウン、CutoffとResonanceを整数ノブで操作します。
Cutoffは0〜1200 tickを10 cent刻みへ換算し、Resonanceは0〜950 tickをQ=0.5〜10（0.01刻み）へ換算します。
Filterは`stepCount=6`のリスト、Cutoffは`stepCount=0`の連続値として公開し、どちらも変更点で切り替えます。
Resonanceは連続値で、ホストの変更点間を正規化値で線形補間します。

Voiceはノートの音程とtuningを含む周波数に `2^(cent/1200)` を乗じ、具体的なHzをフィルタへ渡します。
カットオフはユニゾンのデチューン前のノート周波数に追従し、Note Off後も最後のノート周波数を使用します。

- `FilterParameters`はそのサンプルのHzとQを持ち、既定値はUI／VST3設定側で定義します。
- `SvfState`はTPT SVF1段の状態を保持し、`Svf::process_sample`が`low / band / high`を同時に返します。外側の`Filter::process_sample`が`FilterState`を更新しながら出力選択・直列接続を行います。
- LPF・HPFは1段で12 dB/oct、2段で24 dB/oct。BPFの両側はそれぞれ6／12 dB/oct、中心利得はQ／Q²です。
- 周波数は有限値を受け取り、内部の係数計算時だけ20〜20,000 Hzにクランプします。対応サンプルレートは44.1 kHz以上なので、この範囲は常にNyquist未満です。
- 設定変更やVoiceの再使用でも有効な段の履歴を保持します。未使用の2段目とNone時の両段は消去します。Noneでは入力値を検証せずそのまま返します。
- Note Off後のフィルタ残響も処理します。処理停止・Voice数の減少による停止ではフィルタの履歴も消去します。音量補正・クリッピングは追加しません。

Voiceはカットオフ用エンベロープを毎サンプル計算し、変調後のHzをフィルタへ渡します。

台形積分したSVFの連立方程式をRustで実装しています。数式の参考は
[Andrew SimperによるSVFの解説](https://www.cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf)です。

## カットオフエンベロープ

各Voiceは、音量用と独立したカットオフ用AHDSRを持ちます。
両者は同じ`Envelope`コンポーネントを使い、`EnvelopeState`で時間と現在値を管理します。
エンベロープが扱う値は単位のない0〜1の正規化量で、適用先が実単位へ換算します。

| パラメーター | 範囲 | 初期値 | ParamID |
| --- | --- | --- | --- |
| Cutoff Attack | 0〜5000 ms | 10 ms | `0x43415454` |
| Cutoff Hold | 0〜5000 ms | 0 ms | `0x43484c44` |
| Cutoff Decay | 0〜5000 ms | 100 ms | `0x43444543` |
| Cutoff Sustain | 0〜1 | 0.7 | `0x43535553` |
| Cutoff Release | 0〜5000 ms | 100 ms | `0x4352454c` |
| Cutoff Amount | −4800〜＋4800 cent | 0 cent | `0x43414d54` |

`変調後のcent = Cutoff + Cutoff Amount × エンベロープ値` とし、
`カットオフHz = ノート周波数 × 2^(変調後のcent / 1200)` をフィルタへ渡します。
Amountはattackの目標値1に対応する変化量で、負なら下方向へ変化します。
sustainではAmount×Sustainの変化量を保ち、releaseで基本のCutoffへ戻ります。
再ノートオンでは現在のエンベロープ値を開始点として、attack時間全体を使って1へ移行します。
音量のreleaseが先に終わっても、カットオフ用のreleaseはフィルタの残響に適用されます。

時間ノブは音量用と同じ対数軸、Sustainは線形です。
Amountは10 cent刻みの整数ノブで操作します。
6パラメーターは連続値として公開し、VST3の変更点間を正規化値で線形補間します。

## マスターボリューム

Volumeは全Voiceのディストーション出力を加算した後の音量です（ParamID `0x4d564f4c`）。
全Volumeの範囲は−∞（完全消音）と−48〜＋12 dBです。MasterとOsc1の初期値は0 dB、Osc2/3は−∞です。
整数ノブのtick=0は−∞、1〜601は`dB = (tick − 1) / 10 − 48`で換算し、0.1 dB刻みで設定します。
振幅倍率は`10^(dB / 20)`（−∞では0）で、値の変更時だけ計算して保持します。
VST3では連続値として公開し、正規化値0は−∞、正の値は`max(−48, (normalized × 601 − 1) / 10 − 48)` dBです。
変更点間は正規化空間で補間し、−∞自体の加減算は行いません。

## 3つのオシレータ

| 設定 | 範囲・初期値 | ParamID |
| --- | --- | --- |
| Osc1 Volume | −∞、−48〜＋12 dB・初期値0 dB | `0x4f31564c` |
| Osc2 Waveform | 5波形・Sine | `0x4f325746` |
| Osc2 Tune | −2400〜＋2400 cent・0 cent | `0x4f32544e` |
| Osc2 Volume | −∞、−48〜＋12 dB・初期値−∞ | `0x4f32564c` |
| Osc3 Waveform | 5波形・Sine | `0x4f335746` |
| Osc3 Tune | −2400〜＋2400 cent・0 cent | `0x4f33544e` |
| Osc3 Volume | −∞、−48〜＋12 dB・初期値−∞ | `0x4f33564c` |

TuneはGUIで10 cent刻み、VST3では連続値です。ノート周波数へ`2^(Tune / 1200)`を乗じます。
Osc1のUnison・Detune・WidthはOsc1へ適用し、Osc2/3は共通Panのみで定位させます。
Phaseは3発振器共通で、Randomの乱数と位相はそれぞれ独立しています。
各発振器のVolumeが−∞の間も発音期間中は位相・ノイズ列を進め、音量を戻しても位相を再初期化しません。
3発振器の加算には音数による音量補正を行いません。カットオフはTune適用前のノート周波数を基準とします。

## 位相変調

Osc2/3の**FM**ノブは位相変調の深さを指定します。0〜200%（初期値0%、GUIは1%刻み）で、
100%は1回転、200%は2回転（4π rad）です。
Osc2 FM（ParamID `0x4f32464d`）はOsc1、Osc3 FM（`0x4f33464d`）はOsc2を変調します。
各サンプル時刻でOsc3→Osc2→Osc1の順に生成し、受け側の波形を読む位相を次の式で決めます。

`読出し位相 = (保持位相 + FM / 100 × 生波形) mod 1`（周期単位）

保持位相はTune・Detune適用後の周波数で進め、変調量は累積しません。
FM=200%でモジュレータが±1なら、±2回転なので周期波形は無変調時と同じ位相になります。
途中の値では位相がずれるため、変調音が生じます。
Osc1のユニゾン各音には同じ位相変調量を適用します。FMの変更点間は正規化空間で線形補間します。

発振器は各ユニゾンについてピーク振幅1の生波形を出力します。
実効値補正・ユニゾン合成・Vol・Panは独立した`volume`モジュールで適用し、3発振器の加算後にAHDSRを掛けます。
生波形を変調に使うため、Volが−∞でも変調は継続します。
Noiseは音声と変調で同じ乱数サンプルを共用し、Noise自体への位相変調では乱数列を変更しません。

## ポリフォニー

**Polyphony** は1〜32（初期値1）、ParamIDは `0x504F4C59`、`stepCount=31` / `kCanAutomate` です。
GUIでは1刻みの整数ノブで設定します。変更点では補間せず、ノートより先に適用します。

`SynthesizerState.next_oscillator_id` は初期値0です。有効なNote OnをそのIDへ割り当てた後、
`(ID + 1) % Polyphony` へ進めます。空き枠の探索はせず、発音中・release中の枠もこの順に再使用します。
Note Offと不正なNote OnではIDを進めません。処理停止でも次のIDは保持します。
再使用時は現在振幅を引き継ぎ、新しいノートのattackを開始します。位相はPhaseの選択に従います。

各枠に独立した入力・位相・エンベロープ状態・ノートの対応を持ち、各枠で既存のユニゾンを生成します。
波形、AHDSR、Unison、Detune、フィルタ設定は全枠共通です。フィルタ状態と実際のカットオフ周波数はVoiceごとに独立しています。
出力は使用する全枠の単純加算で、音数による音量補正やクリッピングは行いません。
最大ベロシティ・エンベロープ最大時の各枠の基準実効値は0.5のままで、合成ピークは1を超える場合があります。

Polyphonyを減らすと、IDが新しい枠数以上のノートを即時停止し、次のIDを新しい枠数の剰余へ変更します。
増やしても停止したノートは復活しません。未使用枠の位相は停止して保持します。
最大32枠の領域は初期化時に確保し、音声処理中の確保・解放は行いません。

## ユニゾン

各オシレータ内で、同じノート・波形・音量エンベロープを1〜7ボイスで生成します。

| パラメーター | 範囲 | 初期値 | ParamID |
| --- | --- | --- | --- |
| Unison | 1〜7ボイス | 1 | `0x554E4953` |
| Detune | 0〜100 cent | 0 cent | `0x4454554E` |

Nが2以上のとき、ボイスk（0〜N−1）の音程差は `detune × (2k/(N−1) − 1)` cent、
周波数は `基準周波数 × 2^(音程差/1200)` です。1ボイスではDetuneを適用しません。
Detuneが正なら合成音を `1/√N` 倍にし、最大ベロシティ・エンベロープ最大時の
長時間平均の理想実効値を0.5に合わせます。うなりにより短時間の実効値は変動します。
Detune=0では保持した先頭ボイスの位相だけで1ボイス相当の音を生成し、実効値0.5を保ちます。

各ボイスの初期位相は0で、Note On時にはPhaseの選択を全7位相へ適用します。
音程・設定変更では位相を保持します。未使用ボイスの位相は停止し、ユニゾン数を増やすと保持した位置から再開します。
設定の即時切り替えではクリックが発生する場合があります。ピーク制限は行いません。

Unisonは `stepCount=6`、Detuneは `stepCount=0`、ともに `kCanAutomate` で公開します。
Unisonは変更点で切り替え、DetuneはAHDSRと同様に正規化空間で線形補間します。
GUIではUnisonを1ボイス刻み、Detuneを1 cent刻みの整数ノブで操作します。
ホストからはDetuneの端数も受け付けます。

## WidthとPan

| パラメーター | 範囲 | 初期値 | ParamID |
| --- | --- | --- | --- |
| Width | 0〜100% | 0% | `0x57494454` |
| Pan | −100〜＋100%（左〜右） | 0%（中央） | `0x4D50414E` |

Widthはユニゾンの左右への広がり、Panは全オシレータ共通の定位です。
`W = Width / 100`、`P = Pan / 100`、`s = 2k/(N−1) − 1`として、
各ユニゾン音の定位を `p = P + (1−|P|) × W × s` とします。
1ボイスでは`s=0`です。Width=0では全音が同じ位置になり、Panを端へ近づけると広がりが縮まり、端では全音が片側へ移ります。
Detune=0では先頭位相による1ボイス相当の生成を維持するため、Widthは音に影響しません。

最終的な左右の倍率はequal-power panningを使い、`θ = (p+1) × π/4`、
`L = √2 cos(θ)`、`R = √2 sin(θ)`とします。中央の倍率を左右とも1に保つため√2を掛けています。
理想式での左右の二乗和は2です。実装では既存の四半周期正弦波表（1024区間）を線形補間し、左右倍率の最大絶対誤差を4.17×10⁻⁷、二乗和の最大絶対偏差を1.18×10⁻⁶以内とします。中央の左右倍率1と、端点の無音側0は厳密に維持します。
Width=0・Pan=0では従来と同じ左右出力になります。

GUIでは整数tickで1%刻みに操作し、VST3では連続値として公開します。
オートメーションはAHDSRと同様に正規化空間で線形補間します。
定位の倍率はSynthesizerStateがOsc1用`PanningGains`とOsc2/3用の共通Pan倍率を保持し、各Voice・Oscillatorへ定常入力として参照で渡します。実発音数・有効なWidth・Panが変わった場合だけ再計算し、1ボイス時のWidth変更や正のDetune間の変更では再計算しません。波形生成・位相更新は各ユニゾン音につき1回です。

## 音量エンベロープ

`src/envelope.rs` の `value` は、`EnvelopeParameters`、ノートオンからの経過ミリ秒、
ノートオフ時刻（同じ時刻基準、未到達は `None`）、開始値、目標ピーク値を受け取り、0〜1の正規化値を返します。
音量用`Envelope`は`EnvelopeState`に現在値・開始値・経過サンプル数を保持し、カットオフ用と同じAHDSRを使います。

| パラメーター | 範囲 | 初期値 | ParamID |
| --- | --- | --- | --- |
| Attack | 0〜5000 ms | 10 ms | `0x4154544B` |
| Hold | 0〜5000 ms | 0 ms | `0x484F4C44` |
| Decay | 0〜5000 ms | 100 ms | `0x44454359` |
| Sustain | 0〜1（ピークに対する倍率） | 0.7 | `0x53555354` |
| Release | 0〜5000 ms | 100 ms | `0x52454C53` |

すべて線形に変化します。attack終了後にholdの間ピークを維持し、decayでsustainまで下がります。
ノートオフでは、その時点の振幅からrelease時間で0へ下がります。各時間が0msならその段階は即時に完了します。
再ノートオンでは現在の実効振幅を開始値として保存し、経過時間を0に戻します。
開始振幅や新しいベロシティにかかわらず、attackの終了は再ノートオンからattack時間後です。
初期設定のsustain時、最大ベロシティでの理想実効値は `0.5 × 0.7 = 0.35` です。

時間ノブの設定値は整数tick（0〜371）です。tick=0が0ms、tick=1が1msで、
tick=1〜370は `ms = 10^((tick−1)/100)`、最大端点のtick=371は5000msに換算します。
正の対数領域では100 tick進むと10倍になります。
VST3の正規化値は実単位から連続値へ換算し、0〜1msの区間は線形に対応させます。
Sustainも整数tick（0〜100）を保持し、`音量 = tick / 100` に換算します。
ノブ操作中は中央直上に現在値（時間はms）を表示します。

5パラメーターは `stepCount=0` / `kCanAutomate` で公開します。
変更キューの連続値は、前ブロック末尾（offset=-1）から各点まで正規化空間で線形補間し、
最後の点以降は保持します。同位置のノートより先に設定を適用します。
計算は毎サンプルの設定を用いるため、発音中の時間・sustain変更は進行中の形にも反映します。
開始振幅は次のノートオンまで固定します。不正・非有限の入力は音声へ伝播させません。

## パラメーターと状態保存

ホストのVST3パラメーター表示で **Osc1 Waveform** を操作します。画面上の表示形式はホストに依存します。
`IEditController` で列挙する離散パラメーターで、固定ParamIDは `0x57415645`、
`stepCount = 4`、フラグは `kCanAutomate | kIsList`、既定値は0です。

| 表示名 | 実単位の番号 | 正規化値 |
| --- | --- | --- |
| Sine | 0 | 0 |
| Triangle | 1 | 1/4 |
| Square | 2 | 1/2 |
| Sawtooth | 3 | 3/4 |
| Noise | 4 | 1 |

正規化値は5つの等幅区間へ量子化し、上端の1はNoiseに含めます。

全パラメーターの変更は `ProcessData.inputParameterChanges` から受け取り、`sampleOffset` に従って反映します。

| パラメーター | 変更点の間の処理 |
| --- | --- |
| Osc1/2/3 Waveform・Unison・Polyphony・Filter・Cutoff・Phase・Clipper type | 直前の値を保持し、変更点で切り替える |
| 音量／カットオフ用AHDSR・Amount・Detune・Resonance・全Volume・Osc2/3 Tune・FM・Width・Pan・Gain・Mix | 前ブロック末尾（offset=-1）から各点まで正規化空間で線形補間する |

最後の点以降は値を保持します。同位置ではパラメーターをキュー順に適用した後、ノートをイベントリスト順に処理します。

音声バッファがない場合や処理停止中の0サンプルフラッシュでも、offset 0のパラメーター変更を受け付けます。
未知のParamID、不正な値、範囲外・後戻りする位置、取得に失敗した変更点は無視します。
Controllerの `setParamNormalized` は表示値を更新し、DSPの更新はホストが渡す変更キューを通じて行います。

保存・復元には標準の `IComponent::getState/setState` と `IBStream` を使用します。
保存形式v16は292バイト（`SVST`、バージョンu32、波形番号u32、実単位のAttack/Hold/Decay/Sustain/Release/Unison/Detune/Polyphony/Filter/Cutoff/Resonance/Phase/Volume/Cutoff Attack/Cutoff Hold/Cutoff Decay/Cutoff Sustain/Cutoff Release/Cutoff Amount/Width/Pan/Osc1 Volume/Osc2 Waveform/Osc2 Tune/Osc2 Volume/Osc3 Waveform/Osc3 Tune/Osc3 Volume/Osc2 FM/Osc3 FM/Clipper type/Gain/Mix/Rate/Smoothを各f64。little endian）です。
読み込みは現行形式v16のみです。異なるバージョン・空データ・不正値・途中切れは拒否し、現在の設定を保持します。
`setState` はProcessorと同一インスタンスのControllerの値を復元し、DSPには次の音声処理ブロックから適用します。
標準の `setComponentState` によるControllerの同期にも対応します。
位相・乱数状態・発音中のノートは保存・復元の対象にせず、Controller固有の保存状態は空です。
状態の受け渡しはatomicで行い、音声スレッドをロックしません。

## 内蔵プリセット

DAWのVST3プログラム一覧で「Factory → Sine」を選択できます。`IUnitInfo`でルートユニット0、
プログラム一覧0、音色番号0の「Sine」を公開します。音色設定31個に加え、
`Program`（ParamID `0x50524f47`、`kIsList | kIsProgramChange`）をホストへ公開します。
一覧は1音色なので選択番号・正規化値は0です。表示方法はDAWのプログラム選択画面に従います。

| Sineの設定 | 値 |
| --- | --- |
| Waveform / Phase | Sine / Keep |
| Unison / Detune / Polyphony | 1 / 0 cent / 1 |
| 音量AHDSR | A/H/D/R=0 ms、S=1 |
| Filter / Cutoff / Resonance | None / +4800 cent / Q=1 |
| カットオフAHDSR / Amount | A/H/D/R=0 ms、S=1 / 0 cent |
| Width / Pan / Master Volume / Osc1 Volume | 0% / 中央 / 0 dB / 0 dB |
| Osc2/3 Waveform / Tune / Volume / FM | Sine / 0 cent / −∞ dB / 0% |

ノートオン・オフで即時に発音・消音し、最大ベロシティ時の実効値は片チャンネル当たり0.5です。
選択すると全設定を読み直し、同じ「Sine」の再選択でも編集した値を元に戻します。
ControllerはGUI表示を更新し、`kParamValuesChanged`でホストへ値の再取得を通知します。
音声側はProgramの変更キューの時刻で切り替えます。同位置ではProgram、個別パラメーター、ノートの順に適用します。
連続パラメーターに後続の変更点がある場合、切替時のプリセット値からその点まで補間します。
編集内容は実際の35設定として保存し、復元時にも編集後の値を保持します。

仕様: [VST3 Presets & Program Lists](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Presets%2BProgram%2BLists/Index.html)

## ディストーション

各Voiceのフィルタ直後に適用し、処理後に全Voiceを加算します。

| パラメーター | 範囲・初期値 | ParamID |
| --- | --- | --- |
| Clipper type | None（初期値）、Soft、Hard | `0x43545950` |
| Gain | 0〜+48 dB、0.1 dB刻み、初期値0 dB | `0x43474149` |
| Mix | 0〜100%、1%刻み、初期値100% | `0x434d4958` |
| Rate | x1（初期値）、x2、x4 | `0x4f534d50` |
| Smooth | None（初期値）、1st、2nd | `0x41444141` |

NoneはGainとMixを含めてバイパスします。Softは`tanh(x × gain)`、Hardは`clamp(x × gain, −1, 1)`で処理します。
Gainの線形倍率は`10^(dB/20)`です。Mixは0%でフィルタ出力の原音、100%で処理音になり、その間を線形混合します。Gain・Mixのオートメーションは正規化値を線形補間し、Clipper typeは変更点で切り替えます。

Distortion内のRate（x1／x2／x4）とSmooth（ADAAのNone／1st／2nd）を独立に選択でき、9通りを組み合わせられます。
各Voiceのフィルタ出力を補間し、内部サンプルごとにGain・Clipper・ADAAを適用します。
全Voiceを加算してMaster Volumeを掛け、共有の遅延整合とLPFを通してホストレートへ戻します。
発振器・AHDSR・SVFはホストレートで動作します。
Dryはフィルタ出力を加算してMaster Volumeと69サンプルの純遅延だけを適用し、出力時点のMixでWetと混合します。
Clipper=NoneではDryだけを出力し、補間・間引き・ADAAを省きます。Rate／Smoothの選択値は保持します。

x2は129タップの対称FIR、x4はさらに17タップのハーフバンドFIRを直列に使用します。
係数は固定で、44.1 kHzで20 kHzまでを通過域とする設計です。帯域の境界はホストのサンプルレートに比例します。
1次ADAAのWetには2点平均を加え、ADAA自身の半サンプル遅延と合わせて1内部サンプルの遅延にします。
2次ADAAは1内部サンプルの遅延を持ちます。さらに整数遅延を加え、全方式を69ホストサンプル（48 kHzで約1.44 ms）に揃えます。
ADAAによる平均化はWetの高域特性に影響します。Dryは平均化・補間・間引きを通しません。
Softの原始関数は1025点の共有テーブルから整合した多項式で求め、近接入力では差分の桁落ちを避ける式を使います。
Hardは区分的な関数の積分を直接計算します。履歴はVoiceと左右チャンネルごとに保持します。

処理中のRate／Smooth／Clipper typeの変更は、Wetの32サンプルのフェードアウト、138サンプルの履歴充填、32サンプルのフェードインで切り替えます。
切替中もDryとノート・位相・AHDSRの時間は継続します。停止中の設定は次の処理開始時に即時適用します。
Gainはホストから届いた変更点間を内部サンプル時刻でも補間し、最後の変更点以降は値を保持します。Mixはホストレートで補間します。

## GUI

ホストのプラグイン画面に、ライトテーマの操作盤を表示します。上部のタイトルバーにSample VSTiと、発音数を示す32枠の点灯インジケーターを表示します。
左にOsc1/2/3を縦に並べ、中央上段にAmp、中央にカットオフAHDSRを統合したFilter、その下にDistortion、右にMasterを配置します。AmpのAHDSRは横一列です。WaveformとFilterのドロップダウンはタイトルの右に置きます。Rate／Smoothの名称はPhaseと同じく選択欄の上に左揃えで表示します。Masterは144px幅とし、Phaseラベルと選択欄を左揃えで上下にまとめ、その下にPoly・Vol・Panを縦に並べます。淡いグリーンと白を基調とし、文字・指針・目盛りを濃い色で表示します。
発音数は音量AHDSRのリリース中を含むVoice数です。ユニゾンやOsc1〜3の個数は加算しません。音声ブロック末尾にatomic値を更新し、GUIの既存タイマーで表示へ反映します。処理停止時は0に戻ります。
Osc1にはWaveform・Unison・Detune・Width・Vol、Osc2/3にはWaveform・Tune・Vol・FM、Masterには共通Phase・Polyphony・Volume・Panを配置します。
ノブは行内を均等割りします。ノブ行は80px高、外周・グループ間・枠内の左右余白は8pxで統一します。各列の下端を揃え、見出しの下に残る高さは上余白・行間・下余白へ均等配分します。見出し行とドロップダウンは28px高です。
ノブは直径44pxとし、下に省略名A/H/D/S/R・Uni・Det・Poly・Vol・F・Res・Amtを表示します。
Waveform・Filter・Phaseはドロップダウンで選択します。リストは全選択肢が収まる高さとし、スクロールバーを表示しません。その他のパラメーターはノブです。ノブの円弧と目盛りが設定位置を示し、Tune・Pan・Cutoff Amountの円弧は中央のゼロを基準に伸びます。
ノブを左ボタンで押し、上へ4物理px動かすごとに1tick増加、下へ4pxで1tick減少します。
操作中はカーソルを隠して移動のたびに基準位置へ戻し、ノブの中央直上に「正式名称」「現在値」を中央揃えの2行で表示します。オーバーレイは全パラメーター共通の224×60 pxとし、枠全体が画面内に収まるよう位置を補正します。
Unison・Polyphonyは整数、連続パラメーターは小数点以下最大3桁と単位を表示し、末尾の不要なゼロは省略します。
ボタン解放・Esc・フォーカス喪失・画面終了で調整を終了します。Escでも調整済みの値は保持します。
Windowsではノブ操作の開始・終了でキーボードフォーカスを変更せず、ホストの入力先を維持します。既存の16msタイマーでEsc・フォーカス変更・マウスキャプチャの解除を検出して操作を終了します。Linuxでは終了時にプラグインが保持しているフォーカスをホストの親へ戻します。
時間・音量などへの換算は整数tickから行い、各パラメーターの刻み幅は上記の仕様に従います。
選択肢・単位・整数tickの換算は`src/parameters.rs`で定義し、GUIとVST3 Controllerが共用します。
VST3の `IPlugView` を直接実装し、WindowsではHWND、LinuxではX11EmbedWindowIDへ埋め込みます。
Linuxではホストが `IPlugFrame` 経由で `Linux::IRunLoop` を提供する必要があります。Waylandへの直接埋め込みは未対応です。
サイズは784×560物理ピクセルで固定です。標準フォントは18px、見出しは24pxです。
`python3 scripts/check-abi.py --gui`では、実ウィンドウを撮影した通常画面・選択状態・操作中・発音中の画像を`target/gui-panel.png`、`target/gui-choices.png`、`target/gui-overlay.png`、`target/gui-voices.png`へ保存します。
Windowsでは、描画中に再描画と編集配送タイマーを意図的に再入させ、描画の延期と、描画終了後の編集通知を検証します。
画面作成・描画・破棄と作成失敗時に、ホストのOpenGLコンテキストと描画先が復元されることも検証します。

描画にはegui 0.37.0、egui_glow 0.37.0、baseview 0.4.2のOpenGLバックエンドを使用します。
Windowsでは同じUIスレッドで使われるホストのOpenGLコンテキストと描画先を保存し、GUI処理後に復元します。
対応ドライバーがあれば内蔵GPU・外付けGPUを使います。音声処理はCPUで行います。
ソフトウェアOpenGLでも動作検証していますが、ソフトウェアドライバーはバンドルへ同梱しません。
描画環境を初期化できない場合は画面の取り付けに失敗し、ホストのパラメーター操作と音声処理は引き続き使用できます。

GUIの操作はUI専用の32件キューからホストのUIスレッドへ配送し、
`IComponentHandler::beginEdit / performEdit / endEdit` を順に通知します。
ノブはドラッグ全体で一つの編集とし、32件のうち1件を終了通知用に予約します。
値変更のキューが満杯なら途中の更新を破棄し、終了時に最終値を通知します。
カーソルの復帰はホストへの編集終了通知より先に行います。
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

LinuxからWindows版を生成する場合:

LLVMの`clang-cl`と`lld-link`をPATHへ追加し、[cargo-xwin](https://github.com/rust-cross/cargo-xwin)を導入します。
確認した構成はcargo-xwin 0.23.1、LLVM 19.1.7、Windows SDK 10.0.26100、MSVC CRT 14.44.35220です。
cargo-xwinはMIT、LLVMはApache-2.0 WITH LLVM-exceptionで提供されています。
SDK・CRTはcargo-xwinがMicrosoftの配布元から取得・キャッシュし、[Microsoftのライセンス](https://go.microsoft.com/fwlink/?LinkId=2086102)が適用されます。

```sh
cargo install --locked cargo-xwin --version 0.23.1
rustup target add x86_64-pc-windows-msvc
python3 scripts/build-vst3.py --target x86_64-pc-windows-msvc
```

LinuxからWindowsターゲットを指定した場合、スクリプトは`cargo xwin build`でビルドし、
通常のWindowsビルドと同じ配置・バージョン情報・ライセンス通知を持つZIPを生成します。
`--target`を省略すると実行環境向けにビルドします。

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
開発用の共有ライブラリだけが必要な場合は `cargo build --locked` を使用します。

## バージョン情報

ホストへ返す `PClassInfo2.version` は次の形式です。

- 開発版: `dev-<40桁のコミットID>`。GitのHEADを使用します。
- 正式リリース版: `Cargo.toml` の `version` に指定したSemantic Versioning（例: `0.1.0`）。

`cargo build --release` の最適化指定だけでは正式リリース版になりません。
正式版は `SAMPLE_VSTI_RELEASE_VERSION=0.1.0` を明示してビルドします。
値がCargo.tomlと異なる場合、またはGitの作業ツリーに変更がある場合は失敗します。
CIでは `v0.1.0` のようなタグを指定して手動起動すると正式版を生成し、タグとCargo.tomlの一致を検証します。

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
ホストの鍵盤やピアノロールからノートを送り、複数ノートの発音と個別の消音を確認してください。
音色の操作方法は[GUI](#gui)を参照してください。
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

LinuxのGUI検証には入力注入用の`libXtst.so.6`とカーソル検証用の`libXfixes.so.3`も必要です。
GUIの表示・クリックも検証する場合は `python scripts/check-abi.py --gui` を使用します。
LinuxではABIテストのコンパイルに `libx11-dev` が必要です。画面のない環境では次のように実行できます。

```sh
sudo apt-get install libx11-dev libxtst6 libxfixes3 libxkbcommon-x11-0 libgl1-mesa-dri libegl1 xvfb xauth
LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a python3 scripts/check-abi.py --gui
```

WindowsのGUI検証にはOpenGL対応ドライバーが必要です。CIでは固定版のMesaをABIテスト実行ファイルの隣にだけ配置し、
ソフトウェア描画で検証します。このDLLは配布ZIPへ含めません。
GUIテストは表示・クリック・ノブの上下ドラッグ・カーソルの非表示と復帰・UIスレッドでの標準編集通知・保存復元・フォーカス喪失・操作中の終了と再表示を確認します。

この検証はSDKのインターフェースで共有ライブラリをロードし、バージョン・ID・戻り値、100回のインスタンス生成・解放、
複数のブロック長、32/64ビットの発音・消音、音程変更・再発音での位相継承、
イベントのサンプル位置とノート照合、バッファ境界、同一バッファを指す出力、停止・再開とフラッシュを確認します。
パラメーターの列挙・表示・値変換、変更時刻と位相を含む実際の出力サンプル、
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
（インターフェース定義も3.8.1）。Rustテスト、ABI検証、Validatorが成功しています。
参照した公式SDKヘッダのライセンスはMITです。出典とライセンス全文は [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) を参照してください。
FL Studioで発音とGUI操作の動作報告があります。
GUIの自動検証にはLinuxのXvfbとMesaを使用します。
DSPの速度測定手順は[benchmarks/README.md](benchmarks/README.md)、比較実験の結果は[Wiki](https://github.com/predicatejp/sample-vsti/wiki)にまとめています。

## PRのテスト用ZIP

PRの先頭ブランチを指定してRust CIを手動起動すると、Linux版とWindows版をビルドし、ABI検証と公式Validatorに成功した
バンドルをGitHub ActionsのArtifactsへ保存します。PRのChecksから実行結果を開き、
サマリーのダウンロードリンクまたはArtifactsからZIPを取得してください。保持期間は14日です。

例: `SampleVsti-windows-x86_64-pr2-<40桁のコミットID>.zip`

PRでは先頭コミットを明示的にチェックアウトします。ZIP名・プラグインの開発版バージョン・
`build-info.json` は同じコミットを指します。ZIPを展開すると `SampleVsti.vst3` が得られます。
ダウンロードにはGitHubへのログインとリポジトリの読み取り権限が必要です。

## 開発フロー

trunk-based developmentを採用し、`main`を唯一のトランク・統合先とします。
最新の`origin/main`から目的ごとに短命な作業ブランチを作り、単独で検証・マージできる小さなPRを
必ず`main`へ送ります。作業中は`main`の変更をこまめに取り込みます。
依存する後続作業は先行PRのマージ後に最新の`origin/main`から開始し、マージ済みブランチは再利用しません。

```sh
git fetch origin
git switch -c feat/change-name origin/main
# 変更・検証・コミット
git push -u origin HEAD
gh pr create --base main
```

PRのマージ先は`Pull request policy`を手動起動して`PR target`で検査します。宛先を変更した場合も手動で再検査します。
`main`へ統合する前に、`PR target`と`CI result`の成功、および最新の`main`を取り込んでいることを確認します。
マージ方法はsquashとし、マージ後は作業ブランチを削除します。

## Rust CI

GitHub ActionsのCIは、`workflow_dispatch`による明示的な手動起動だけで実行します。
Actions画面の「Run workflow」で対象ブランチを選択するか、次のコマンドを実行してください。
PRでは先頭ブランチとPR番号を指定します。番号はバンドルの成果物名にも使われます。

```sh
gh workflow run rust.yml --ref feat/change-name -f pr_number=123
gh workflow run pull-request-policy.yml --ref feat/change-name -f pr_number=123
# 正式版はタグを指定して起動する。
gh workflow run rust.yml --ref v0.1.0
```

`.github/workflows/rust.yml`では以下を検証します。
Linux x86_64とWindows x86_64のランナーを使用します。

| ジョブ | 内容 |
| --- | --- |
| Format, Clippy, docs | rustfmt、警告をエラーにするClippyとドキュメント生成 |
| Build and test | Linux/WindowsそれぞれでMSRV（Rust 1.95.0）と最新stableのdebug/releaseテスト・releaseビルド（全target・全feature） |
| Miri | 固定したnightlyでRustテストの未定義動作を検査 |
| Coverage | RustテストのLCOVレポートを生成・保存 |
| Dependency audit | cargo-denyで脆弱性・ライセンス・依存元・依存バージョンを監査 |
| VST3 ABI and Validator | Linux/Windowsそれぞれで公式SDKの固定版を使ったABI・GUI検証・Validatorを実行し、バンドルZIPとログを保存 |
| CI result | 上記すべてのジョブの成功を確認 |

CIは読み取り専用のリポジトリ権限で動作します。成果物は14日間保持します。
Miriとカバレッジの対象はRustテストです。外部ホストとのABI互換性は別のVST3ジョブで検証します。
現時点ではカバレッジ率の最低値は設けていません。

Build and testとVST3検証では、Cargoの取得済み依存とビルド結果を
GitHub公式の`actions/cache` v4.3.0（MIT、コミット固定）で再利用します。
キャッシュはOS・ジョブ・Rustコンパイラー・Cargo.lock・ビルド設定ごとに分けます。
依存変更時も同じコンパイラーのキャッシュから再利用できる分を復元し、Cargoが変更分を再ビルドします。
初回やコンパイラー更新時にはキャッシュ生成が必要です。CIではincrementalを無効にしてキャッシュ容量を抑え、
対象コミットを`SAMPLE_VSTI_COMMIT`でも明示して古いバージョン情報の再利用を防ぎます。
独立した`cargo check`はテスト・ビルドと重複するため実行しません。

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

## 構成

- `build.rs`: Gitコミットと正式版指定からバージョン・ビルド情報を生成。
- `src/abi.rs`: 必要なABI型、OS別のID表現・戻り値、vtable定義。
- `src/lib.rs`: モジュールの公開関数とファクトリー。
- `src/plugin/programs.rs`: VST3のルートユニットと内蔵プログラム一覧。
- `src/plugin.rs`: インスタンス、バス、イベント・パラメーター処理、音声出力、Controller。
- `src/gui.rs`: VST3画面の直接実装とホストへの編集通知。
- `src/editor.rs`: カテゴリ別ノブ・ドロップダウンの入力とegui/OpenGL描画。`src/editor/edits.rs`は編集通知キュー、`src/editor/pointer.rs`はOS別のカーソル操作。
- `src/host.rs`: UIスレッドに限定したホスト参照管理。
- `src/parameters.rs`: 全パラメーターの選択肢・単位・値変換、VST3変更キューとIBStreamの読み書き。
- `src/volume.rs`: 生波形の実効値補正・ユニゾン合成・Vol・定位。
- `src/oscillator.rs`: 定常入力 `OscillatorInput` と内部状態 `OscillatorState` を分離した5波形オシレータ。
- `src/envelope.rs`: 音量とカットオフで共用する正規化AHDSR計算と経過時間・再トリガー管理。
- `src/filter.rs`: TPT SVFの3出力と、フィルタタイプによる出力選択・多段化。
- `src/distortion.rs`: フィルタ後のプリゲイン・クリップ・1次／2次ADAA。`src/distortion/`はSoft用の原始関数と共有テーブル。
- `src/synth.rs`: Voiceの循環割り当て・加算出力と、共通の定位係数・マスター音量の管理。
- `src/oversampling.rs`: Voiceごとの補間、加算後の共有FIR間引きと固定遅延。
- `src/voice.rs`: Voiceごとのノート照合と3発振器加算 → 共通音量AHDSR → ステレオFilter → Distortionの処理パイプライン。
- `src/float_mode.rs`: 音声処理中の非正規化数処理モードの保存・切り替え・復元。
- `src/random.rs`: 位相とNoiseで共用するxoshiro256+乱数生成器。
- `src/oscillator_tables.rs`: Detune倍率と四半周期正弦波の固定テーブル。
- `src/tests.rs`: Rust側のABIレイアウトと参照管理のテスト。各コンポーネントの回帰テストは各モジュール内に配置。
- `benchmarks/`: 継続利用するDSPの速度測定。
- `tests/gui-smoke.h`: 埋め込み画面とマウス操作の検証専用コード。
- `tests/gui-reentry.h`: Windowsの描画・タイマー再入を再現する回帰試験。
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

![Integralの操作画面](artwork/example.png)

# Integral

Integralは、3基のオシレータと最大32音の同時発音に対応したVST3シンセサイザーです。ユニゾンや位相変調による音作り、フィルタ、エフェクトをひとつの操作画面にまとめています。

## 主な機能

- **3基のオシレータ**：サイン波・三角波・矩形波・ノコギリ波・ホワイトノイズを選択。Osc1は最大7声のユニゾンに対応し、デチューンと左右の広がりを調整できます。
- **位相変調**：Osc3→Osc2→Osc1の変調で音色を変化させます。
- **演奏表現**：最大32音のポリフォニー、モノフォニック時のポルタメント、ピッチベンドに対応。
- **音量とフィルタ**：独立したAHDSRエンベロープとベロシティ感度。フィルタはローパス・ハイパス・バンドパスを選択できます。
- **3つのエフェクトスロット**：Chorus、Compressor、テンポ同期Delay、Distortion、Phaser、Reverbを自由な順序で組み合わせ、3帯域EQとリミッタで仕上げられます。
- **DAWとの連携**：パラメーターのオートメーション、プロジェクトへの音色保存、内蔵プリセット「Sine」に対応。

## 動作環境

| 項目 | 対応 |
| --- | --- |
| OS | Windows / Linux（GNU）、いずれもx86_64版 |
| CPU | AVX2・FMA3対応。OSでもAVXを利用できること |
| ホスト | VST3音源に対応したDAW |
| 音声出力 | ステレオ |
| サンプルレート | 44.1 kHz以上 |

LinuxのGUIにはX11、libxkbcommon-x11、OpenGL/EGLが必要です。

## インストール

1. 使用するOS向けのZIPを展開します。
2. 以下のパスになるよう、`Integral.vst3`フォルダー全体を配置します。
3. DAWでプラグインを再スキャンし、音源トラックに **Integral** を読み込みます。

| OS | 配置後のパス |
| --- | --- |
| Windows | `C:\Program Files\Common Files\VST3\Integral.vst3` |
| Linux | `~/.vst3/Integral.vst3` |

更新時はDAWを終了し、同じ場所の`Integral.vst3`フォルダーを新しいものに置き換えてください。

## 使い始める

DAWのプリセット選択から内蔵音色 **Sine** を読み込み、MIDI鍵盤やピアノロールで発音を試せます。

- **ノブ**：上下にドラッグして調整します。操作中はパラメーター名と現在値が表示されます。音程や音量などの基準値には吸着し、合わせやすくなっています。
- **選択リスト**：クリックして波形やエフェクトの種類を選びます。エフェクト・フィルタ・EQは **Off** で無効にできます。
- **エフェクトの順序**：各スロット右上の **↑ / ↓** で入れ替えます。
- **Reset**：音色の設定を保ったまま、発音とエフェクトの残響をリセットします。

作成した音色はDAWのプロジェクトと一緒に保存できます。各コントロールの詳細は[音源仕様・操作ガイド](https://github.com/predicatejp/integral-vsti/wiki/Instrument-Reference)を参照してください。

## ソースからビルドする

[環境構築の手順](https://github.com/predicatejp/integral-vsti/wiki/Development)に沿って準備し、リポジトリのルートで実行します。

- Linux：`sh scripts/build-vst3.sh`
- Windows（PowerShell）：`python scripts/build-vst3.py`

`target/vst3/Integral.vst3`にプラグイン、`target/dist/`に配布用ZIPを生成します。詳しいビルド手順と技術資料は[Wiki](https://github.com/predicatejp/integral-vsti/wiki)にまとめています。

## ライセンス

Copyright (C) 2026 Yusuke Oda

[GNU AGPL v3.0（AGPL-3.0-only）](LICENSE)で提供しています。[著作権表示](COPYRIGHT)・[第三者のライセンス通知](THIRD_PARTY_NOTICES.txt)も参照してください。

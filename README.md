# Integral

Integralは、3基のオシレータと最大32音のポリフォニーを備えたVST3シンセサイザーです。Rustで開発しています。

## 主な機能

- サイン波・三角波・矩形波・ノコギリ波・ホワイトノイズ。Osc1は最大7声のユニゾンに対応し、Osc3→Osc2→Osc1の位相変調も可能です。
- 音量とカットオフの独立したAHDSRエンベロープ、ベロシティ感度、ローパス・ハイパス・バンドパスフィルタ。
- 順序を自由に設定できる3つのエフェクトスロット。Chorus、Compressor、Delay、Distortion、Phaser、Reverbを選択できます。
- 3帯域EQ、マスターの音量・定位・リミッタ、エフェクト系列のx1／x2／x4オーバーサンプリング。
- ノブ中心のGUI、VST3オートメーション、DAWプロジェクトへの設定保存、内蔵プリセット「Sine」。

## 動作環境

| 項目 | 対応 |
| --- | --- |
| OS | Windows x86_64、Linux x86_64（GNU） |
| プラグイン形式 | VST3 Instrument / Synth |
| 音声出力 | ステレオ、float32 / float64 |
| サンプルレート | 44.1 kHz以上 |
| ノート入力 | Note On / Note Off、16チャンネル合計で最大32音 |

LinuxのGUIにはX11、libxkbcommon-x11、OpenGL/EGLが必要です。

## インストール

使用するOS向けのZIPを展開し、`Integral.vst3`フォルダー全体をVST3の探索先へ配置します。

| OS | 配置先の例 |
| --- | --- |
| Windows | `C:\Program Files\Common Files\VST3\Integral.vst3` |
| Linux | `~/.vst3/Integral.vst3` |

DAWでプラグインを再スキャンし、音源一覧から **Integral** を読み込みます。
内蔵プリセット「Sine」で基本の発音を試せます。更新時はDAWを終了し、旧版のバンドルを置き換えてください。

## ビルド

Rust 1.95以上、Python 3、Gitが必要です。LinuxではCリンカー、WindowsではVisual Studio Build Toolsの「C++によるデスクトップ開発」とWindows SDKを用意します。

Linux:

```sh
sh scripts/build-vst3.sh
```

Windows（PowerShell）:

```powershell
rustup target add x86_64-pc-windows-msvc
python scripts/build-vst3.py
```

`target/vst3/Integral.vst3`にバンドル、`target/dist/`にZIPを生成します。
クロスコンパイルや正式リリースの手順は[開発資料](https://github.com/predicatejp/sample-vsti/wiki/Development)を参照してください。

## ドキュメント

- [音源仕様・操作](https://github.com/predicatejp/sample-vsti/wiki/Instrument-Reference)：パラメーター、発音、信号処理、GUI。
- [ビルド・検証・開発](https://github.com/predicatejp/sample-vsti/wiki/Development)：環境構築、テスト、CI、ソース構成。
- [Wiki](https://github.com/predicatejp/sample-vsti/wiki)：技術資料と性能・音響特性の調査。

## ライセンス

Copyright (C) 2026 Yusuke Oda

**AGPL-3.0-only**。全文は[LICENSE](LICENSE)、著作権表示は[COPYRIGHT](COPYRIGHT)を参照してください。
VST3のABI定義に関する出典とMITライセンス通知は[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt)に記載しています。
配布バンドルには本体・依存ライブラリ・付属フォントのライセンス通知を同梱します。

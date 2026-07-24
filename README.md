# tc-npc

AI NPC(マスコット)を1プロセスで動かすRustのCLI/サーバーです。会話・短期/長期記憶・音声(STT/TTS)・画面認識・VRChat互換のOSCアバター制御・定時アナウンスといったエージェント群を1バイナリに統合し、起動時にローカルWeb UI(既定 `http://127.0.0.1:47950`)を配信します。もとは複数のGoマイクロサービス(Redis pub/subで連携)だった構成を、プロセス内バス(`tokio::sync::broadcast`)に置き換えて1バイナリへ統合したものです。

## 特徴

- **会話 (talk)**: OpenAI互換LLMとのチャットエージェント。プロンプトテンプレートに短期/長期記憶・ペルソナを差し込み
- **記憶 (memory)**: 直近会話の短期要約と、チャンク分割+埋め込みによる長期ベクトルストア(RAG検索)
- **音声 (speech / tts・stt)**: マイク入力のVAD区切り+OpenAI互換STT、OpenAI互換TTSでの発話再生。既定では無効(config有効化制)
- **視覚 (vision)**: 画面/ウィンドウの定期キャプチャとVLMによる説明。既定では無効
- **行動 (action)**: VRChat互換OSCでのアバター制御、位置管理、自然言語からのコマンド列生成。既定では無効
- **スケジューラ (scheduler)**: 時刻指定の定時アナウンス(TTS発話+チャイム)。既定では無効
- **Web UI**: Preact製のローカルUI(チャット/キャラ/音声/視覚/行動/設定タブ)をバイナリに同梱して配信
- **キャラクターインポート**: tc-town のキャラクターエクスポートJSONを取り込み、ペルソナシートとして利用
- **mist (オプション機能)**: `mist` cargo featureで mistlib (P2P) を組み込み、tc-townのキャラクターカタログルームに接続する機能。既定では無効

## クイックスタート

### 前提

- Rust (stable)
- Node.js (Web UIのビルド用)

### ビルド・実行

```bash
# Web UIビルド + Rustビルドを一括で
just all

# または手動で
cd web && npm install && npm run build
cd ..
cargo build --release
```

初回実行:

```bash
tc-npc run
```

起動すると `config.server.addr`(既定 `http://127.0.0.1:47950`)でWeb UIが開きます。

## 設定

設定ファイルは既定で `./config.json`、または `--config <path>` で指定したパスから読み込まれます。まずは例をコピーしてください。

```bash
cp config.example.json config.json
```

APIキーなどの秘匿情報は `.env` に記載します(`.env.example` を参照)。`.env` は先に読み込まれ、その後一部の環境変数が対応するJSONフィールドを上書きします。

LLM/TTS/STTのエンドポイントはすべてOpenAI互換で、既定値はlocalhost(Ollama等のローカル推論サーバー)を想定しています。

**APIキーは絶対にコミットしないでください。** `config.json` と `.env` は `.gitignore`済みです。

主な設定セクション:

| セクション | 概要 |
|---|---|
| `api` | チャット/埋め込み用のOpenAI互換LLMエンドポイント |
| `tts` | 音声合成(OpenAI互換TTS)。既定で無効 |
| `stt` | 音声認識(OpenAI互換STT)。既定で無効 |
| `talk` | 会話エージェントの履歴サイズ・プロンプトテンプレート |
| `memory` | 短期/長期記憶の閾値・チャンクサイズ・要約プロンプト |
| `vision` | 画面キャプチャ間隔・対象ウィンドウ・VLMプロンプト。既定で無効 |
| `action` | VRChat OSCアドレス・移動先(locations)・巡回ルート(routes)。既定で無効 |
| `vrc` | VRChat連携(チャットボックス送信、OSCアドレス) |
| `scheduler` | 定時アナウンスのリスト(時刻・本文・チャイム音声・音量)。既定で無効 |
| `server` | Web UI/APIのバインドアドレスと自動オープン |
| `character` | 現在アクティブなキャラクターID |
| `mist` | mistlib接続設定(シグナリングURL・ルームID)。`mist` featureを有効にした場合のみ意味を持つ |

## キャラクター

キャラクターは [tc-town](https://github.com/tik-choco) のキャラクターエクスポートJSON(`{"app":"tc-town","version":1,"kind":"character","characters":[...]}`)から取り込みます。

```bash
tc-npc import <export.json>
tc-npc characters
```

インポートされたキャラクターは `character.active_id` に設定したものがアクティブになり、そのパーソナリティシートがNPCのペルソナ(システムプロンプト)として使われます。

## CLI コマンド

| コマンド | 説明 |
|---|---|
| `tc-npc run` | エージェント群とWebサーバーを起動(サブコマンド省略時の既定動作) |
| `tc-npc import <path>` | tc-townエクスポートJSONからキャラクターを取り込み |
| `tc-npc characters` | 保存済みキャラクターの一覧表示 |
| `tc-npc config-path` | 解決される設定ファイルパスとデータディレクトリを表示 |

すべてのサブコマンドで `--config <path>` を指定可能です。

## アーキテクチャ

各エージェントは `Module` トレイトを実装するクレート(`npc-talk` / `npc-memory` / `npc-speech` / `npc-vision` / `npc-action` / `npc-scheduler`)として分離されており、プロセス内バス(`npc-core::bus`)経由でメッセージをやり取りします。バスのトピック/メッセージ種別は、統合前のGoエージェント群(Redis pub/sub)の配線契約をそのまま踏襲しています。詳細は [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) を参照してください。

## VRChat連携

`action` モジュールはVRChat互換のOSCプロトコルでアバターを制御します。既定のOSC送信先は `127.0.0.1:9000` です。

- 位置・移動: `config.action.locations` / `routes` で定義した地点・巡回ルートへの移動
- チャットボックス: `config.vrc.chatbox` を有効にすると発話内容をVRChatのチャットボックスに送信
- 自然言語コマンド: チャット入力から行動コマンド列をLLMで生成し、OSC経由でアバターへ送信

## mist機能(オプション)

`mist` cargo featureを有効にすると、mistlib(MPL-2.0ライセンスのP2Pライブラリ)を組み込み、tc-townのキャラクターカタログルームに接続できます。

```bash
cargo build --features mist
```

既定(feature無効)のビルドではmistlibへの依存は発生しません。`config.mist` セクションで接続先(シグナリングURL・ルームID)を設定します。

## ライセンス

このリポジトリ自体は [MIT License](LICENSE) です。mist機能が依存する mistlib は MPL-2.0 ライセンスで、外部依存として取得されます(このリポジトリのコードには含まれません)。

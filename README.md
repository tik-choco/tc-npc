# tc-npc

AI NPC(マスコット)を1プロセスで動かすRustのCLI/サーバーです。会話・短期/長期記憶・音声(STT/TTS)・画面認識・VRChat互換のOSCアバター制御・定時アナウンスといったエージェント群を1バイナリに統合し、起動時にローカルWeb UI(既定 `http://127.0.0.1:47950`)を配信します。もとは複数のGoマイクロサービス(Redis pub/subで連携)だった構成を、プロセス内バス(`tokio::sync::broadcast`)に置き換えて1バイナリへ統合したものです。

## 特徴

- **会話 (talk)**: OpenAI互換LLMとのチャットエージェント。プロンプトテンプレートに短期/長期記憶・ペルソナを差し込み
- **記憶 (memory)**: 直近会話の短期要約と、チャンク分割+埋め込みによる長期ベクトルストア(RAG検索)
- **音声 (speech / tts・stt)**: マイク入力のVAD区切り+OpenAI互換STT、OpenAI互換TTSでの発話再生。既定では無効(config有効化制)
- **視覚 (vision)**: 画面/ウィンドウの定期キャプチャとVLMによる説明。既定では無効
- **行動 (action)**: VRChat互換OSCでのアバター制御、位置管理、自然言語からのコマンド列生成。既定では無効
- **スケジューラ (scheduler)**: 時刻指定の定時アナウンス(TTS発話+チャイム)に加え、同じ時刻に実行するアクション(自然言語での行動指示・コマンド・会話への投入・suspend/resume・任意のトピックへのraw送信)も指定可能。既定では無効。Web UIの「予定」タブでは次の実行までの残り時間が表示され、時刻を待たずにその場で実行する「テスト実行」も可能
- **同時通訳 (translation)**: 聞き取った発話をLLMで翻訳。`interpret`(通訳のみ・NPCは応答しない) / `assist`(会話しつつ発話と応答に訳文を付ける)の2モードがあり、翻訳先は最大2言語、逆方向の発話は自動で話者の言語へ訳し戻します(auto_reverse)。訳文はWeb UIの「通訳」タブとVRChatチャットボックスに出力。既定では無効
- **Web UI**: Preact製のローカルUI(チャット/キャラ/音声/視覚/行動/予定/通訳/設定タブ)をバイナリに同梱して配信
- **多言語対応**: UIの表示言語(日本語/English/中文)は「設定 > 一般」タブで切り替え。ブラウザの言語設定を初期値とし、選択はブラウザに保存されます。NPC自身の応答言語は別設定(`config.language`)
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
| `language` | NPCが応答する言語。`auto`(既定・プロンプト任せ) / `ja` / `en` / `zh`。talk・vision・memory・actionの各システムプロンプトに「この言語で答えて」という指示を追加します |
| `api` | チャット/埋め込み用のOpenAI互換LLMエンドポイント |
| `tts` | 音声合成(OpenAI互換TTS)。既定で無効 |
| `stt` | 音声認識(OpenAI互換STT)。既定で無効 |
| `talk` | 会話エージェントの履歴サイズ・プロンプトテンプレート |
| `memory` | 短期/長期記憶の閾値・チャンクサイズ・要約プロンプト |
| `vision` | 画面キャプチャ間隔・対象ウィンドウ・VLMプロンプト。既定で無効 |
| `action` | VRChat OSCアドレス・移動先(locations)・巡回ルート(routes)。既定で無効 |
| `vrc` | VRChat連携(チャットボックス送信、OSCアドレス) |
| `scheduler` | 定時アナウンスのリスト(時刻・本文・チャイム音声・音量・アクション)。既定で無効 |
| `translation` | 同時通訳(モード・話者の言語・翻訳先言語・文脈数・auto_reverse・チャットボックス出力)。既定で `mode: "off"` |
| `server` | Web UI/APIのバインドアドレスと自動オープン |
| `character` | 現在アクティブなキャラクターID |
| `mist` | mistlib接続設定(シグナリングURL・ルームID)。`mist` featureを有効にした場合のみ意味を持つ |

### プライベートCA配下のエンドポイント

学内・社内で自前運用しているOpenAI互換サーバーは、公開CAではなくプライベートCAが発行した証明書を使っていることがあります。HTTPSクライアントはOSのトラストストア(Windowsの証明書ストア、Linuxの `/etc/ssl/certs` 等)も参照するため、そのルートCAをOSにインストール済みであればそのまま接続できます。

OSにインストールできない場合は、ルートCAを含むPEMバンドルのパスを環境変数で渡してください。

| 環境変数 | 説明 |
|---|---|
| `TC_NPC_CA_BUNDLE` | 追加で信頼するCA証明書のPEMバンドルへのパス |
| `TC_NPC_INSECURE_TLS` | `1` / `true` / `yes` でTLS証明書の検証を完全に無効化(最終手段。通信内容が保護されなくなります) |

接続テスト(モデル一覧の取得)が `502 Bad Gateway: http request failed: ... invalid peer certificate: UnknownIssuer` で失敗する場合が、まさにこのケースです。

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

各エージェントは `Module` トレイトを実装するクレート(`npc-talk` / `npc-memory` / `npc-speech` / `npc-vision` / `npc-action` / `npc-scheduler` / `npc-translate`)として分離されており、プロセス内バス(`npc-core::bus`)経由でメッセージをやり取りします。バスのトピック/メッセージ種別は、統合前のGoエージェント群(Redis pub/sub)の配線契約をそのまま踏襲しています。詳細は [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) を参照してください。

## 予定とアクション

`scheduler.announcements[]` は毎日同じ時刻に繰り返す予定です。`text` を読み上げるだけでなく、`actions` に指定したものを同じ時刻に実行できます(Go版 `agent-scheduler` の `redis_actions` 相当)。`text` が空でアクションだけの予定も有効です。

| `kind` | 動作 |
|---|---|
| `speak` | `{content, chime_file}` を読み上げ |
| `action` | 自然言語の指示を `npc-action` に渡す(例: 「原点に移動して」) |
| `command` | CLIコマンドを実行(例: `route patrol`) |
| `chat` | 発話として投入し、NPCに応答させる |
| `suspend` / `resume` | 会話・発話の一時停止/再開 |
| `raw` | `{topic, type, payload}` をそのままバスへ送信 |

```json
{
  "time": "17:00",
  "text": "",
  "actions": [
    { "kind": "action", "content": "原点に移動して" },
    { "kind": "command", "text": "route patrol" }
  ]
}
```

Web UIの「予定」タブから編集でき、保存すると再起動なしで反映されます。「テスト実行」は本文とアクションの両方をその場で実行します。

## 同時通訳

`translation.mode` を切り替えて使います(Go版 `agent-speech` の翻訳機能の移植)。

- `off`: 翻訳しない(既定)
- `interpret`: 聞き取った発話を翻訳する。NPCは応答せず通訳に徹します(`npc-talk` が発話入力を受け取らなくなります)
- `assist`: いつもどおり会話しつつ、発話とNPCの応答の両方に訳文を付けます

`target_language` / `target_language_2` で最大2言語に翻訳し、`auto_reverse` を有効にすると翻訳先の言語で話しかけられたときに `source_language` へ訳し戻します。`context_size` の分だけ直近の発話を文脈としてLLMに渡します。訳文はWeb UIの「通訳」タブに表示され、`translation.chatbox` を有効にすると `vrc.osc_address` 宛にVRChatチャットボックスとしても送信されます。設定はWeb UIから変更でき、再起動なしで次の発話から反映されます。

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

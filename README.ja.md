# tc-npc

AI NPC(マスコット)を1プロセスで動かすRustのCLI/サーバーです。会話・記憶・音声・
画面認識・VRChatアバター制御・定時アナウンスといったモジュール群が、プロセス内バス
(`tokio::sync::broadcast`)経由でやり取りします。もとは複数のGoマイクロサービス
(Redis pub/sub連携)だった構成を1バイナリへ統合したものです。起動時にローカル
Web UI(既定 `http://127.0.0.1:47950`)を配信します。

English README: [README.md](README.md)

## 特徴

- **会話 (talk)**: OpenAI互換LLMとのチャットエージェント。プロンプトテンプレートに
  短期/長期記憶・ペルソナを差し込み。感情ドライブモデルと終話の自動制御にも対応
  (`talk.affect`)
- **記憶 (memory)**: 直近会話の短期要約と、チャンク分割+埋め込みによる長期ベクトル
  ストア(RAG検索)。話者名や視覚から人物ごとの記録も蓄積(`memory.people`)
- **音声 (speech)**: マイク入力のVAD区切り+OpenAI互換STT、OpenAI互換TTSでの発話
  再生。既定では無効
- **視覚 (vision)**: 画面/ウィンドウの定期キャプチャとVLMによる説明。既定では無効
- **行動 (action)**: VRChat互換OSCでのアバター制御、位置管理、自然言語からのコマンド
  列生成。既定では無効
- **スケジューラ (scheduler)**: 時刻指定の定時アナウンス(TTS+チャイム)に加え、
  同じ時刻にアクション(コマンド・会話投入・suspend/resume・raw送信)も実行可能。
  既定では無効
- **同時通訳 (translation)**: `interpret`(通訳のみ)/ `assist`(会話+訳文付き)の
  2モード、翻訳先は最大2言語。既定では無効
- **Web UI**: Preact製のローカルUI(チャット/キャラ/人物/音声/視覚/行動/予定/通訳/
  設定タブ)をバイナリに同梱して配信
- **キャラクターインポート**: tc-town のキャラクターエクスポートJSONをペルソナ
  シートとして取り込み
- **VRMアバター**: `~/.tc-npc/vrm/` に `.vrm` を置く(またはキャラタブから追加)
  だけでモデル一覧に載り、チャット画面をアバター主体のレイアウトに切り替えられる。
  待機モーション・まばたき・感情モデル連動の表情に加え、口はサーバー側で実際に
  鳴っているTTSの音量に追従する。キャラクターは必須ではなく、未登録ならモデル
  単体で使え、キャラごとの割り当ても可能。`#/avatar` は同じアバターだけを表示
  する画面で、別ウィンドウとして開ける(画角・背景・カメラ位置はリロードしても
  保持される)
- **mist (オプション機能)**: `mist` cargo featureで mistlib (P2P) を組み込み、
  tc-townのキャラクターカタログルームに接続。既定では無効

モジュール・バス・プロトコルの詳細(人物メモリの仕組みを含む)は
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)(英語)を参照してください。

## クイックスタート

前提: Rust (stable)、Node.js(Web UIビルド用)

```bash
just release          # Web UIビルド + 単一バイナリ(デスクトップUI付き)
tc-npc                # 初回実行 — サーバー、Web UI ウィンドウ、マスコット
```

または手動で:

```bash
cd web && npm install && npm run build
cd ..
cargo build --release --features desktop
```

ウィンドウシステムに依存しないビルドが欲しい場合は `--features desktop` を
外してください(`just release-headless`)。詳細は[モード](#モード)を参照。

起動すると `config.server.addr`(既定 `http://127.0.0.1:47950`)でWeb UIが開きます。
ブラウザを自動で開かせたくない場合は `--no-open`(または
`config.server.auto_open: false`)を指定してください。`just watch` はRust
ソースの変更ごとに再起動しますが、この場合毎回新しいタブが開くことはありません。

## テスト

```bash
cargo test --workspace     # Rust
cd web && npm test         # Web UI (vitest)
```

## モード

実行ファイルは1つ、顔が4つあります。サブコマンドで切り替えます。

| コマンド | 内容 |
|---|---|
| `tc-npc` / `tc-npc app` | **既定**。サーバー + デスクトップウィンドウ2枚(Web UI と、デスクトップに立つキャラクター) |
| `tc-npc mascot` | キャラクターのウィンドウだけ。起動中の tc-npc に接続 |
| `tc-npc serve` | GUI なし。サーバーのみで、Web UI はブラウザで開く(`run` はエイリアス) |
| `tc-npc tui` | 端末UI。起動中の tc-npc に接続 |

`app` と `mascot` は `--features desktop` ビルド(`just release`)が必要です。
これが Tauri を引き込む唯一のスイッチです。付けないビルド
(`just release-headless`)はウィンドウシステムへの依存が一切なく、サーバーや
ディスプレイの無いマシン向けに小さくなります。そこでも `serve` と `tui` は
動き、`app` は誤動作せず「desktop 付きで再ビルドしてください」と言って終了
します。

デスクトップのキャラクターは `#/avatar` を透過・枠なし・最前面で表示した
ウィンドウです。**移動はウィンドウ上端の帯をドラッグ**してください。ウィンドウに
カーソルを乗せるとグリップが浮かび上がります。キャラクターの体をドラッグすると
カメラが回転しますが、これは意図した挙動です。閉じる・隠す・表示するはトレイ
アイコンから行います(タイトルバーが無いため)。

`tui` はサーバーではなくクライアントです。Web UI と同じ HTTP/WebSocket API を
話すので、SSH ポートフォワード越しの tc-npc にもそのまま繋がります
(`tc-npc tui --addr 127.0.0.1:47950`)。

## 設定

```bash
cp config.example.json config.json
```

設定ファイルは既定で `config.json`(または `--config <path>`)から読み込まれます。
APIキーなどの秘匿情報は `.env` に記載します(`.env.example` を参照)。`.env` は
先に読み込まれ、一部の環境変数が対応するJSONフィールドを上書きします。

**APIキーは絶対にコミットしないでください。** `config.json` と `.env` は
`.gitignore` 済みです。

LLM/TTS/STTのエンドポイントはすべてOpenAI互換で、既定値はlocalhost(Ollama等)を
想定しています。設定項目の全リストは `config.example.json` 内のコメントを参照
してください。

学内・社内のOpenAI互換サーバーがプライベートCA証明書を使っていてOSのトラスト
ストアにルートCAをインストールできない場合は、環境変数 `TC_NPC_CA_BUNDLE` に
PEMバンドルのパスを指定するか、最終手段として `TC_NPC_INSECURE_TLS=1` を設定して
ください。接続テストが `invalid peer certificate: UnknownIssuer` で失敗する場合は
これが原因です。

## CLI

| コマンド | 説明 |
|---|---|
| `tc-npc run` | エージェント群とWebサーバーを起動(サブコマンド省略時の既定動作) |
| `tc-npc import <path>` | tc-townエクスポートJSONからキャラクターを取り込み |
| `tc-npc characters` | 保存済みキャラクターの一覧表示 |
| `tc-npc config-path` | 解決される設定ファイルパスとデータディレクトリを表示 |

すべてのサブコマンドで `--config <path>` を指定可能です。

## ライセンス

このリポジトリ自体は [MPL-2.0 License](LICENSE) です。mist機能が依存する外部
ライブラリ mistlib も同じく MPL-2.0 ライセンスです。

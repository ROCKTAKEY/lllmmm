# lllmmm

llama.cppとStrataのローカルLLMを、1つの接続先から使うためのLinux向けサーバーです。要求したモデルを自動でロードし、モデルの切り替えや、アイドル時のアンロードを行います。CodexなどのResponses API対応クライアントから利用できます。

## 準備

LinuxとGuix、および使用するモデルファイルを用意してください。起動スクリプトがビルド環境と、`runtime-manifest.scm` に指定した llama.cpp・Strata（`strata-hip`）をGuixで用意します。`strata-hip` を提供する [roquix](https://github.com/ROCKTAKEY/roquix) チャンネルが必要です。Strataを使う場合は、モデルのpack・tokenizerなども準備してください。

バイナリを直接起動すると、設定ファイルは `$XDG_CONFIG_HOME/lllmmm/lllmmm.toml` から読み込みます。`XDG_CONFIG_HOME` が未設定・空・相対パスの場合は `~/.config/lllmmm/lllmmm.toml` を使います。

リポジトリのディレクトリで、初回に設定例を配置します。既に設定がある場合は、そのファイルを編集してください。

```sh
case "${XDG_CONFIG_HOME:-}" in
    /*) config_dir="$XDG_CONFIG_HOME/lllmmm" ;;
    *) config_dir="$HOME/.config/lllmmm" ;;
esac
mkdir -p "$config_dir"
cp -i examples/lllmmm.toml "$config_dir/lllmmm.toml"
# Strataを使う場合
cp -ri examples/strata "$config_dir/"
```

`lllmmm.toml` で、使用するモデルのファイルパスや実行ファイルを指定します。不要な `[[models]]` ブロックは削除してください。Strataを使う場合は `strata/*.json` のパスも編集します。

このREADMEとCodex設定例では、接続先のポートを `19100` とします。コピーした設定の `listen` を次の値にしてください。

```toml
listen = "127.0.0.1:19100"
```

## 起動・終了

```sh
# 設定を確認
./scripts/run.sh "$config_dir/lllmmm.toml" check

# ビルドして起動
./scripts/run.sh "$config_dir/lllmmm.toml"
```

接続先は `http://127.0.0.1:19100/v1` です。最初の推論要求ではモデルのロード時間がかかります。終了は **Ctrl-C**、設定変更の反映は再起動で行います。

ビルド済みのバイナリは `lllmmm` で起動し、`lllmmm check` で設定を確認できます。設定ファイルを明示する場合は `--config /path/to/lllmmm.toml` を指定します。

`run.sh` は、引数なしではリポジトリ内の `config/lllmmm.toml` を使います。XDG配下など別の設定を使う場合は、上の例のようにパスを指定してください。

## 管理画面

起動後、ブラウザで `http://127.0.0.1:19100/` を開きます。ポートを変更した場合は、そのポートでアクセスしてください。

モデルの状態・使用中のモデル・待機要求数を2秒ごとに確認できます。登録モデルの「ロード」で切り替え、「アンロード」で終了します。使用中の要求がある場合は、応答の終了を待って操作します。画面には操作結果や接続エラーも表示されます。

「ハードウェア」でサーバー全体のDRAM使用量とGPUごとの使用率・VRAMを確認できます。GPUの計測値はLinuxのドライバーが公開する情報を利用し、取得できない項目は「取得不可」と表示します。AMD GPUの使用率・VRAMに対応しています。配色は「OS設定」「ライト」「ダーク」から選択でき、選択はブラウザに保存されます。

## モデルの設定

llama.cppの最小設定例です。`model_file` を実際のモデルファイルに置き換えてください。

```toml
listen = "127.0.0.1:19100"
idle_timeout_secs = 900

[[models]]
id = "my-model"
backend = "llama_cpp"
executable = "llama-server"
port = 18081
model_file = "/srv/models/model.gguf"
context_size = 32768
```

- `id`：クライアントの `model` に指定する名前。
- `port`：そのモデルの推論サーバーに使う空きポート。`listen` とは別の値を指定します。
- `context_size`：モデルに渡せる文脈の長さ（トークン数）。
- `idle_timeout_secs`：応答後、自動アンロードまで待つ秒数。`0` で無効にすると、モデルをメモリに保持して次の要求で再利用できます。手動アンロード、モデル切り替え、終了時には解放します。

GPUなどの追加設定は `extra_args`、環境変数は `models.env` に記述できます。[モデル設定例](examples/lllmmm.toml)を参照してください。パスは `~` や環境変数を展開済みの文字列で記述します。

Strataでは `backend = "strata"` とし、`executable` に `strata-server`、`config_file` に推論設定のJSONを指定します。推論設定の `exe` は `"strata"` とします。コマンド名は起動環境の `PATH` から解決するため、`run.sh` ではサーバーとエンジンの両方に manifest のパッケージを使用します。明示的なパスを指定する場合は絶対パス、またはJSONファイルからの相対パス（`./engine` など）を使います。[Strata設定例](examples/strata/iq3_s.json)を利用環境に合わせて編集してください。

`guix pull` でチャンネルを更新した後に `run.sh` を実行すると、更新されたパッケージ定義が使われます。

## Codexから使う

Codex CLI 0.134.0以降向けの設定例を用意しています。利用する推論サーバーとモデルには、Responses APIとツール呼び出しへの対応が必要です。

1. [プロバイダー設定](examples/codex.toml)を `~/.codex/config.toml` に追加します。
2. 使用するモデルの `examples/local-*.config.toml` を `~/.codex/` にコピーします。
3. プロファイルを指定して起動します。

```sh
codex --profile local-strata-iq3
# または
codex --profile local-strata-iq2
codex --profile local-ornith
```

`CODEX_HOME` を設定している場合は、`~/.codex/` の代わりにそのディレクトリを使います。プロファイルの `model` はlllmmmのモデルIDに、`model_context_window` はモデルのコンテキスト設定に合わせてください。ポートを変更した場合は、プロバイダー設定の `base_url` と `model_catalog_url` も合わせます。

`model_catalog_url` に `http://127.0.0.1:19100/codex/models` を指定すると、登録モデルをCodexのモデル選択画面から選べます。このURLは、Codexが選択候補と能力情報を読み込むための専用JSONを返します。設定項目の詳細は[Codexの設定リファレンス](https://developers.openai.com/codex/config-reference/#model_providersidmodel_catalog_url)を参照してください。

初期選択の候補は、lllmmmの設定ファイルのトップレベル（最初の `[[models]]` より前）で指定できます。

```toml
default_model = "qwen3.8-flash-next-iq3_s"
```

指定した登録モデルをカタログの先頭に置き、最も高い優先順位で返します。省略時は全モデルを同じ優先順位で返し、未登録・不正なモデルIDは起動時に拒否します。Codexのモデル一覧では優先候補が `isDefault: true` になります。T3 Codeなどのクライアントが保存済みの選択や明示的なモデル設定を使っている場合は、そちらの選択が優先されることがあります。設定変更後はlllmmmを再起動し、クライアントのモデル一覧を再取得してください。この設定はカタログの選択候補にのみ適用され、起動時の自動ロードは行いません。推論APIでは引き続き `model` の指定が必要です。

カタログはモデルをロードせずに生成します。llama.cppのコンテキスト長には `context_size` を使用します。Strataでは推論設定JSONの `args` にある `"--max-context", "262144"` のような指定からコンテキスト長を取得し、省略時はStrataの既定値である4,096トークンを使用します。`--kv-resident` はKVキャッシュの配置に関する値なので、コンテキスト長には使用しません。推論エンジンとCodexで上限を一致させるため、`--max-context` は値を別の要素として指定し、重複・値の欠落・正の整数でない値は設定読み込み時に拒否します。Codexプロファイルで `model_context_window` を明示している場合は、その値も推論設定と揃えてください。カタログではテキスト入力と通常のツール呼び出しを前提とし、推論レベルの選択肢・推論要約・verbosity・並列ツール呼び出しは宣言しません。

詳細は[Codexのプロファイル設定](https://developers.openai.com/codex/config-advanced#profiles)を参照してください。

## 状態確認・手動アンロード

```sh
# 登録モデルの一覧
curl http://127.0.0.1:19100/v1/models

# Codex用のモデルカタログ
curl http://127.0.0.1:19100/codex/models

# 現在の状態
curl http://127.0.0.1:19100/admin/status

# 使用中のモデルをアンロード
curl -X POST http://127.0.0.1:19100/admin/unload
```

起動やモデルのロードに失敗した場合は、ターミナルのエラーと、設定ファイルのディレクトリにある `.local/<モデルID>.log` を確認してください。ログの場所は `state_dir` で変更できます。

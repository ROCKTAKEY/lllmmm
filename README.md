# lllmmm

llama.cppとStrataのローカルLLMを、1つの接続先から使うためのLinux向けサーバーです。要求したモデルを自動でロードし、モデルの切り替えや、アイドル時のアンロードを行います。CodexなどのResponses API対応クライアントから利用できます。

## 準備

LinuxとGuix、および使用するモデルファイルを用意してください。起動スクリプトがビルド環境とllama.cppをGuixで用意します。Strataを使う場合は、Strata 0.1.39とモデルのpack・tokenizerなども準備してください。

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
- `idle_timeout_secs`：応答後、自動アンロードまで待つ秒数。

GPUなどの追加設定は `extra_args`、環境変数は `models.env` に記述できます。[モデル設定例](examples/lllmmm.toml)を参照してください。パスは `~` や環境変数を展開済みの文字列で記述します。

Strataでは `backend = "strata"` とし、`executable` に `strata-server`、`config_file` に推論設定のJSONを指定します。[Strata設定例](examples/strata/iq3_s.json)を利用環境に合わせて編集してください。

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

`CODEX_HOME` を設定している場合は、`~/.codex/` の代わりにそのディレクトリを使います。プロファイルの `model` はlllmmmのモデルIDに、`model_context_window` はモデルのコンテキスト設定に合わせてください。ポートを変更した場合は、プロバイダー設定の `base_url` も合わせます。

詳細は[Codexのプロファイル設定](https://developers.openai.com/codex/config-advanced#profiles)を参照してください。

## 状態確認・手動アンロード

```sh
# 登録モデルの一覧
curl http://127.0.0.1:19100/v1/models

# 現在の状態
curl http://127.0.0.1:19100/admin/status

# 使用中のモデルをアンロード
curl -X POST http://127.0.0.1:19100/admin/unload
```

起動やモデルのロードに失敗した場合は、ターミナルのエラーと、設定ファイルのディレクトリにある `.local/<モデルID>.log` を確認してください。ログの場所は `state_dir` で変更できます。

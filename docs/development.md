# 開発

管理画面は、モデルのライフサイクルとホストの計測値を表示するサーバー中心のUIです。表示に使う状態はRustのenumから組み立て、[Askama](https://askama.rs/en/stable/)で型検査されたHTMLを生成します。[htmx](https://htmx.org/docs/)は定期取得と操作結果の部分更新を担当します。ブラウザ固有の配色設定、通信状態、画面を開いてからの履歴はJavaScriptで扱います。

画面は `src/management/templates`、表示用の型は `src/management/view.rs` にあります。JSON APIと画面の操作は同じロード／アンロード処理を使うため、モデルの切替条件と要求の待機動作を共有します。

`cargo build --locked --release` でテンプレートと静的資産を含むバイナリが生成されます。Guix環境では次のコマンドで検証できます。

```sh
guix shell -m manifest.scm -CWNF -- cargo test --locked
guix shell -m manifest.scm -CWNF -- cargo clippy --all-targets --locked -- -D warnings
guix shell -m manifest.scm -CWNF -- cargo fmt --all -- --check
python3 tests/launcher.py
```

ブラウザで確認するときは、ロード完了までに2秒の定期更新が挟まる条件でも操作が完了し、ボタンが再び使えることを確認します。通信切断時の「未確認」表示と復帰、自動更新の停止・手動更新、配色の保存、狭い画面での操作も確認します。

## htmxの更新

ローカルで管理画面を利用できるよう、htmxは `src/management/vendor` に固定版を同梱しています。バージョンは2.0.11、ライセンスは同ディレクトリの `htmx-LICENSE` です。

配布元: <https://cdn.jsdelivr.net/npm/htmx.org@2.0.11/dist/htmx.min.js>

公式ドキュメント掲載のSHA-384（Base64）:

```text
2OatzQy1H+Zd/IIrjr1TcuDGqLXeHhbooAyJY1KdQMKnr4LZ22k31GBLdYKHmVjg
```

更新時は公式配布物とライセンスを取得し、公開されたハッシュとの一致を確認します。配布ファイル名、Rust側の参照、この資料のバージョンとハッシュを揃え、上記の検証を実行します。画面はCSPに合わせてhtmxのコード評価とインラインスクリプトを無効にしているため、属性追加時もこの設定で動作を確認します。

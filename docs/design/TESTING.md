# TESTING — テストの層とカバレッジ計測

<a id="267326f7-e919-48f2-9737-fe0c0daec9d5"></a>

uniqnode のテストは 12 層ある。すべて `cargo test` で走り、決定論的である(乱数は種付き自前
生成器のみ、時刻への依存なし、待ちは条件ポーリング)。

| 層 | 場所 | 検証するもの |
|---|---|---|
| ユニット | node/src/*.rs の `#[cfg(test)]` | 純関数層: 暗号(RFC 8032・FIPS ベクタ、openssl 相互検証)、c1 正規形、CRC、ストアの回復・取り込みの成功系と拒否系 |
| 統合(API・MCP) | node/tests/api.rs, ingest.rs, search.rs, rendition.rs, viewer.rs, query.rs, groups.rs, health.rs, sync.rs, distributed_search.rs, mcp.rs | 実プロセスの serve に対し、curl が組み立てる形の生 HTTP/1.1 で往復する(should/0138)。レプリケーションの受け入れ基準(複製が読める・停止に耐える・復旧で収束)を含む。MCP は実プロセスの `uniqnode mcp` に Claude Code が実際に送る形の JSON-RPC を標準入力から流し、応答の出典と、標準出力に JSON-RPC 以外の行が 1 行も混ざらないことを検査する([docs/design/MCP.md](#dacd474d-424a-45d5-a278-766fc2465dd9)) |
| 運用ログ | node/tests/logging.rs | 実プロセスの serve に本番の呼び手と同じ POST /v1/search を投げ、データディレクトリに残った記録を読む: 既定でファイルが作られること、内容が標準エラーと一致すること、行頭に UTC の時刻が付くこと、小さな上限を注入して実際に回転し古い世代が消えること、既定の道に書けない(0500 の)ストアを指した mcp が XDG_STATE_HOME の下へ倒して最初の行に両方の道を残すこと、`--log` で明示した開けない道を指したとき倒さずに理由を言って提供は続けること([docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004)) |
| バックアップ | node/tests/backup.rs | 本番の入口 `uniqnode backup` を子プロセスとして走らせ、写し先をストアとして開いて観測する: ロックを持ったまま開いてあるストアの隣で取った写しが全オブジェクトを持ち fsck が緑であること、2 回目が封印済みセグメントを写さず未封印だけ写し直すこと、写し先の封印済み pack を壊すと赤になり消せば写し直されること、derived・logs・tmp を写さず報告に名を出すこと、未封印の尻尾の切り詰めを `cut` で言うこと、逆向きの backup と fsck で復元できること([docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2)) |
| GC | node/tests/gc.rs | 本番の入口 `uniqnode gc` と serve の `POST /v1/admin/gc` を実プロセスで走らせ、出力と終了コードと指したストアに残ったものを観測する: 小さい封印閾値で複数の pack に分けて上書きで孤児を作り、dry-run が pack ごとの行と集計を出して 0 で終わり packs/・reflog/・MANIFEST の中身と mtime が 1 つも動かず tmp/ が空のままであること、`--threshold` 0 と 1 で対象が変わり範囲外は usage で落ちること、回収が孤児を含む pack を書き直して生きているものが全件読め fsck が緑で used_bytes と packs/ のディスク上の合計が減り旧 pack が消えて新 pack が MANIFEST に載ること、2 回目が参照表(derived/refs/)を流用して mtime を動かさないこと、admin の dry_run と不正なボディの 400 と走っている間の 2 つ目の 409、B の後で止めている間に GET が答え孤児を指す新しい ref と put し直しが生き返って再起動後も読めること、serve がロックを持つストアには 1 で終わり serve が生きていること、ストアでない場所は初期化せず 1 で断ること([docs/design/GC.md](#9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)) |
| 検査は状態を作らない | node/tests/fsck.rs | 本番の入口 `uniqnode fsck` を子プロセスとして走らせ、終了コードと指した場所に残ったものを観測する: 空のディレクトリと存在しない道には理由を言って 1 で終わり node_key も MANIFEST も作らないこと、封印前で MANIFEST の無い在るストアには緑であること、復元先を先に fsck しても続く逆向きの backup が断られず同じ node id で戻ること、status も初期化せず init だけが作ること([docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2) の「復元」) |
| 据え付け | node/tests/install.rs | 本番の入口 `uniqnode install` を `--unit-dir`(作業ディレクトリ)と `--no-start` で子プロセスとして走らせ、置かれたものを観測する: バイナリが走らせた実行ファイルと同じ中身で実行でき、unit 4 本が docs/mop/systemd/user/ の現物と一致し、drop-in 3 本に指定した値と置いたバイナリを指す ExecStart= が載り、再実行しても同じ結果になること。/tmp の下のストアは何も置かずに断ること、知らない引数は usage で落ちること、ロックの探りが実プロセスの serve が開いているストアを見分けること。daemon-reload まで行う設計なので systemctl が PATH に無い機械ではその 1 本だけが前提を出力して戻る(should/0128 の規約からの逸脱で、理由はテストの冒頭に記す)([docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2)) |
| クラッシュ | node/tests/crash.rs, gc_crash.rs | 書き込み中の実プロセスを位置を変えて5回 SIGKILL し、毎回の回復と fsck 全件パス。回収(gc)は debug ビルドだけが読むテスト用の口 `UNIQNODE_GC_CRASH_AFTER` で各相(S・B の途中・C-1・C-2・C-3)の直後に abort させ、開き直しの fsck が緑で生きているものが全件読め、孤児は消えているか次回の対象として残っているかのどちらかで、tmp/ の書きかけと MANIFEST に無い pack を回復が片づけること、S と C の間に追記があるときの C-2 の後(C-1 の順序)、MANIFEST を失ったストアでは残骸の規則を使わず fsck が事実として報告すること([docs/design/GC.md](#9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)) |
| モデル(実装) | node/tests/sync_model.rs | 実装そのもの(Store + sync の核)を種付き乱数で駆動: 書き込み・tombstone・意図的 dangling・同期・再起動を無作為に交錯させ、全対同期後の収束・閉包完全性・fsck 健全を検証する |
| モデル(仕様) | sim/ | 仕様 §8(レプリカ・健全性)の離散イベントシミュレーション。実装に先行して規則の安全性を検証した([docs/analysis/20260815-replica-model-simulation.md](#31e38823-b783-4dfe-bc7c-3cd268f5e7b4)) |
| 評価(順位の質) | node/tests/eval.rs | 固定の小コーパスと「クエリ → 正解チャンク」対で検索方式の Recall@k と MRR を測り、現在の実測値を基準線として固定する。方式を差し替えて数値を比べる差し替え点も同じ層にある([docs/design/EVAL.md](#1109a04b-923e-4493-8f00-d704047d6a2a)) |
| 文書の衛生 | node/tests/repo_hygiene.rs | 追跡している散文そのものを機械で検める: policy 集の形と retired stub、SPEC・docs・ソースからのポリシー参照が実在の番号を指すこと、docs の UUID アンカーが定義され一意で解決し、文書名を名乗るリンクの字面が行き先と一致すること、強調の太字を使っていないこと、lock を「ロック」と書き U+9320 の字を使っていないこと(CLAUDE.md の用語の裁定。ソースと unit も対象) |

敵対系も欠かさない: 改竄レコード(署名不正)、seq の飛び、要求 ID と異なるバイト列を返す
不正ピア、封印済みセグメントの破損、二重オープン。これらは「拒否されること」をテストする。

外部コマンドを前提とするテストは、そのコマンドが無い環境で黙って飛ばさず、必要なバイナリ名と
導入手順を示して失敗する。飛ばして緑にすると、検証したのか検証を諦めたのかが結果から区別
できなくなる(should/0128)。

外部サービスを前提とするテストも同じ規約に従う。埋め込みサーバを要するテストは 4 つ(順位の
基準線と、語彙の隔たる対が意味検索で届くことの確認と、API 越しに言い換えのクエリを引くものと、
hybrid で BM25 が一件も一致しなかったときに片肺の融合を応答が言うことの確認)で、
環境変数や feature で切り替えるのではなく、実際に 1 本埋め込んでみて届かなければ、待ち受ける
べきアドレスと llama-server の起動コマンドを示して落ちる。生存の判定を実際の埋め込みで行うのは、
llama-server の GET /v1/models の capabilities が埋め込み対応の判定に使えないため
でもある([docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580))。

埋め込み層の残りは、サーバを要求しない純粋なテストの側にある。RRF の融合(固定した順位の列に
対する手計算)、コサイン、ベクトルのキャッシュの往復(追記の途中で止まった末尾を捨てて何バイト
捨てたかを数える経路を含む)、応答 JSON の読み取りの拒否系、chunked の HTTP 応答の組み立て直しは
どれも単体テストである。劣化の経路(埋め込みサーバに届かないときに BM25 で答え、method と
degraded がそれを言う)も、届かないアドレスを指せば足りるのでサーバを要求しない。要求するのは
「実際の模型が付ける順位」を見るテストだけである。

## 方針(lamalium の COVERAGE.md から輸入)

- カバレッジは計測のみで、ゲート条件(閾値)は設けない。行が通ったことと挙動が検証された
  ことは別物であり、数値目標化は薄いテストを誘発する。低下時に差分を見る運用にとどめる。
- 新しいテストは挙動を検査し、失敗時に期待と実際を語る(should/0125)。バグ修正には欠陥を
  復元して赤を確認したテストを添える(should/0137)。
- 検索の順位の質も同じ扱いである。評価ハーネスの基準線は現在の実測値であって目標値ではなく、
  テストの仕事は順位が黙って悪化したことの検出にとどまる。閾値を先に書いて実装をそれに合わせに
  いかず、意図して数値を動かしたときは実測し直してリテラルを置き換える
  ([docs/design/EVAL.md](#1109a04b-923e-4493-8f00-d704047d6a2a))。

## 計測手順

```
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
cargo llvm-cov --summary-only     # テキストサマリ
cargo llvm-cov --html             # target/llvm-cov/html/index.html
```

計測値は節目ごとに docs/analysis/ に記録する(このファイルには写さない — 古びるため)。

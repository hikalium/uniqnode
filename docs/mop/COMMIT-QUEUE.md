# COMMIT-QUEUE — uniqnode の変更を Commit Queue に main へ入れてもらう

<a id="1df91ea3-07a5-422f-9c9d-83c9f9a7b8ae"></a>

読み手は、uniqnode の変更を main へ入れたいセッション(送り手)と、それを main へ入れる
Commit Queue(CQ)である。lamalium と sumi も同じ CQ が入れる。lamalium の側の手順は lamalium の
docs/mop/WORKTREE.md の Integrate and deploy 節にある(別のリポジトリなので、#uuid の参照では
指せない)。

## 誰が main へ入れるか

利用者の指示(2026-10-01 13:16Z、lamalium のプロジェクトのスレッドで)は次のとおりである:
「各マシンセッションはmainへの統合作業を実施せず、このセッションの1エージェントのみがCommit Queue
としての役割を果たす」「Commit Queue役が各セッションから来たコミットをmainに統合するという作業を
無事に完了したらその旨送信元セッションに通知し、チェックの失敗などで当該パッチが取り込めないと
判断された場合もその旨および失敗したチェック等の情報を添えて当該コミットを差し戻す」。ここで
「このセッション」は、そのスレッド「Commit Queue(mainへの統合)」のセッションを指す。

指示に由来する決まりは次の 4 つである。

- main へ統合するのは CQ の 1 エージェントだけで、ほかのセッションは main へ push しない。
- 送り手は commit id と統合の意思を CQ へ送る。
- 統合できたら、CQ は送り手へ知らせる。
- 取り込めないときは、CQ は失敗した検査などの情報を添えて送り手へ差し戻す。

次の 3 つは、指示には無く、運用として足した決まりである。

- 対象を uniqnode・lamalium・sumi の 3 つにする。プロジェクトの調整役が CQ を立てたときの指示に
  よる。
- 送る項目を、下の「送り手の手順」の形に揃える。CQ が受け付けを機械的に確かめられるようにする
  ためである。
- CQ は変更の中身を直さずに差し戻す。直した変更を送り手が確かめないまま main へ入るのを避ける
  ためである。

## 宛先と経路

CQ は、claude.ai の lamalium のプロジェクトのスレッド「Commit Queue(mainへの統合)」の
セッションである(2026-10-01 時点)。ホストの Claude のセッションからは、SendMessage で
`bridge:session_0121YxhvvFnBHVUwzPYgESRo` へ送るか、claude-code-remote の send_message で
`session_0121YxhvvFnBHVUwzPYgESRo` へ送る。CQ はその送り手のセッションへ send_message で返す。
CQ のセッションが替わったら、プロジェクトの調整役が新しい宛先を送り手へ知らせる。

Codex には SendMessage が無い。crystal の Codex は、依頼の全文を
`/home/lamalium/lamalium-install/coordination.md` に書き、crystal の Claude のセッション
(`bridge:session_01MaBCbXUfCELqmukmhAB7wT`)へ turn/steer で知らせる。その Claude のセッションが
全文を CQ へ送り、CQ の答えを同じファイルへ書き戻す(lamalium の CLAUDE.md の Codex の節にある
分担の形である)。

vega の検査(下の「送り手の手順」の 4)は、vega の Claude のセッション
(`bridge:session_01EVEBXiTc3Wrjfoetpbkij2`)に頼む。頼む側は、push 済みのブランチの
リポジトリ、ブランチ、先頭の SHA を送る。vega のセッションは、その SHA を fetch し、
`git worktree add --detach <新しい場所> <SHA>` で専用の作業場所を作り、`git rev-parse HEAD` が
その SHA と一致し `git status --porcelain` が空であることを確かめてから、そこで
`cargo test --no-fail-fast` を回す。返すのは、実際の `git rev-parse HEAD` と `HEAD^` の SHA と、
落ちたテストの名前(無ければ無いこと)である。vega のセッション自身が送り手なら、同じ形で自分で
回す。

## 合格の条件

検査は `cargo test --no-fail-fast` である。次の 3 つが全て揃えば合格とする。

- ビルドが通る。
- 全てのテストのバイナリが最後まで走り終える(異常終了や時間切れが無い)。
- 落ちたテストが、下の「クラウドの CQ の手元で落ちるテスト」の表に名前のあるものだけである。
  vega で回したときは、落ちたテストが 1 つも無い。

表に無いテストが落ちたら不合格である。回し直して通っても合格にしない。例外は、説明の文書だけの
変更(下の定義)のときに、下の「不安定なテスト」の節に名前と原因を挙げたテストが、そこに書いた
原因の形で落ちたときだけである。そのテストが別の形で落ちたら、例外にしない。repo_hygiene のように
文書の中身を検査するテストは、不安定なテストの節に挙げない。送り手も CQ も、同じ例外を使う。

説明の文書だけの変更とは、取り込む全ての commit で、変えたファイルが全て次のどれかに当たる
変更である。拡張子は `.md` だけを許す。

- docs/ の下の Markdown のファイル。ただし docs/mop/systemd/ の下は除く(unit はバイナリに埋め
  込まれる。node/src/install.rs)。
- リポジトリの根の Markdown のファイル(README.md、CLAUDE.md、SPEC.md など)と policy/ の下の
  Markdown のファイル。

node/ の下のファイルは、Markdown でも説明の文書に当たらない。node/tests/assets/ の Markdown は
評価のコーパスや試験の資材として読み込まれる(node/tests/eval.rs、node/tests/agent_door.rs)。
判定は commit ごとに `git diff-tree --no-commit-id -r --no-renames --name-status <commit>` で行い、
消した側と足した側の両方のパスを見る。名前の変更を 1 行にまとめると移動元が見えないからである。

## 送り手の手順

1. 最新の `origin/main` からブランチを切って変更を作り、コミットする。複数の commit でもよいが、
   merge commit は作らない。
2. 合格の条件を満たすまで、手元で `cargo test --no-fail-fast` を回す。
3. 設計の変更・修正・実装は、README の「開発の作法」のとおり、異なる種類のモデルのレビューを
   通す。
4. ブランチを origin へ push する。main へは push しない。説明の文書だけの変更でないときは、push
   した先頭の SHA を vega で検査してもらい(「宛先と経路」の形)、落ちたテストが 1 つも無いことを
   確かめる。このとき、ブランチは送る時点の `origin/main` を祖先に持たせる
   (`git merge-base --is-ancestor origin/main <先頭>` が 0 で終わる)。
5. CQ へ 1 通で送る。中身は次のとおりである。
   - リポジトリ、ブランチ、先頭の commit id(40 桁)、基点(`origin/main` のどの commit から
     切ったか)、目的、依存する変更、統合の意思。
   - 検査の結果: どの機械で、どの SHA で回し、どのテストが落ちたか。手順 4 の vega の結果なら、
     vega が返した HEAD と HEAD^ の SHA も写す。
   - レビュー: どのモデルで、どの SHA に通したか。軽微で明確な変更として通していないなら、その旨。

差し戻されたら、push 済みのブランチを書き換えない(must/0011)。最新の `origin/main` から新しい
ブランチを切り、自分の変更を cherry-pick して直し、手順 2 からやり直す。CQ が統合する前に main が
進んだときも、同じやり方で新しいブランチを作る。進んだ main を自分のブランチへ merge しない。

## CQ の手順

1. 送られたブランチを fetch し、先頭が送られた commit id と一致することを確かめる。一致しなければ
   積まずに送り手へ問い合わせる。レビューの欄が空のときも、統合せずに問い合わせる。
2. 取り込む範囲(`origin/main..<先頭>`)に merge commit が無いことを
   `git log --merges origin/main..<先頭>` が空であることで確かめる。あれば差し戻す。
3. 範囲の各 commit を「合格の条件」の節の方法で調べ、説明の文書だけの変更かどうかを決める。
4. 説明の文書だけの変更でないとき:
   - `origin/main` が先頭の祖先でなければ、統合せずに差し戻し、新しい `origin/main` から切った
     ブランチを vega で検査し直してもらう。CQ の手元では表のテストが落ちるので、vega の結果の
     代わりにならない。
   - vega の結果の HEAD の SHA が、送られた先頭と一致することを確かめる。
   - 統合する木は、送られた先頭そのものである。CQ は cherry-pick も rebase もせず、送られた先頭へ
     main を fast-forward する。vega で検査した木と main に入る木が同じになる。
5. 説明の文書だけの変更のときは、`origin/main` が先頭の祖先ならそのまま進める。祖先でなければ、
   CQ の手元の写しで、`origin/main` の上へ送り手の commit を古い順に cherry-pick する。送り手の
   ブランチも main も書き換えないので、must/0011 の禁じる、push 済みの commit の書き換えには
   当たらない。衝突したら、直さずに差し戻す。
6. 統合する木で `cargo test --no-fail-fast` を回し、合格の条件で判定する。
7. 合格なら main を fast-forward で push する。push までに main が動いたら、手順 2 から
   やり直す。統合の知らせには、リポジトリ、送られた先頭の SHA、main へ入った各 commit の SHA と
   送られた commit との対応(cherry-pick したときは、元の SHA と新しい SHA の組を全て)を書く。
8. 不合格なら push せず、送り手へ差し戻す。差し戻しには、リポジトリ、送られた先頭の SHA、赤の
   理由(merge commit、衝突したファイル、ビルドの失敗、走り終えなかったテストのバイナリ、落ちた
   テストの名前)、出力の要点の行を本文に写して添える。送り手は CQ の手元のログを読めないから
   である。

## クラウドの CQ の手元で落ちるテスト

CQ はクラウドで動き、vega に届かない。次のテストは、vega の上でしか通らないものを使うので、
CQ の手元では `origin/main` でも落ちる(2026-10-01 に a8e287d と b9b3bb0 で確かめた)。

| テスト | ファイル | CQ の手元で落ちる理由 |
|---|---|---|
| `rerank::tests::the_running_reranker_reorders_the_candidates` | node/src/rerank.rs | 127.0.0.1:8084 のリランカーが無い |
| `rendition::tests::renditions_are_generated_once_and_then_reused` | node/src/rendition.rs | pdftoppm が 22.02 でなく 24.02 である |
| `embedding_and_fusion_hold_the_recorded_baseline_on_the_fixed_corpus` | node/tests/eval.rs | 127.0.0.1:8083 の埋め込みサーバが無い |
| `the_vocabulary_gap_pairs_that_semantics_reaches` | node/tests/eval.rs | 127.0.0.1:8083 の埋め込みサーバが無い |
| `a_paraphrased_query_reaches_its_section_through_the_search_api` | node/tests/search.rs | 127.0.0.1:8083 の埋め込みサーバが無い |
| `hybrid_says_when_bm25_matched_nothing_and_the_fusion_was_one_sided` | node/tests/search.rs | 127.0.0.1:8083 の埋め込みサーバが無い |
| `the_server_fills_missing_vectors_after_a_write_without_a_request` | node/tests/search.rs | 127.0.0.1:8083 の埋め込みサーバが無い |
| `install_without_start_places_the_binary_the_units_and_the_drop_ins` | node/tests/install.rs | install が 0 で終わらない(systemd と nft が無い) |
| `a_second_instance_gets_its_own_units_drop_ins_and_firewall_table` | node/tests/install.rs | install が 0 で終わらない(systemd と nft が無い) |
| `an_unwritable_default_log_falls_back_to_the_users_state_directory` | node/tests/logging.rs | root で走るので、書けないはずのログに書けてしまう |
| `an_ipv4_mapped_connection_from_an_ipv6_socket_gets_through` | node/tests/main_door.rs | IPv6 のソケットが作れない(Address family not supported) |
| `loopback_literals_bind_and_the_owner_gets_through_on_ipv4_and_ipv6` | node/tests/main_door.rs | serve が 30 秒の内に待ち受けを始めない |
| `a_connection_that_sends_and_closes_at_once_is_not_executed` | node/tests/main_door.rs | serve が 30 秒の内に待ち受けを始めない |

表は、変更と同じ commit で直す。vega の上でしか通らないテストを足す変更は、この表に行を足す。
表のテストをクラウドでも通るように直した変更は、その行を消す。表のテストを消すか `#[ignore]` に
する変更は、表の行も消し、理由を目的に書く。表に無いテストが表の理由と同じ形で落ちたら、送り手か
CQ が表を直す変更を送る。

## 不安定なテスト

次のテストは、変更の前から CQ の手元でときどき落ちる。説明の文書だけの変更では、ここに書いた
原因の形で落ちたときに限り、合格の条件の例外として扱う。

| テスト | ファイル | 落ちる形 |
|---|---|---|
| `a_node_that_is_not_registered_gets_no_answer` | node/tests/distributed_search.rs | ピアの応答の読み取りが `Resource temporarily unavailable (os error 11)` で時間切れになり、`"outcome":"timed_out"` が返る |

2026-10-01 の観測: CQ の手元では a8e287d でも落ち、単独で 5 回回すと 1 回落ちた。vega では同じ日に
全テストを 3 回、このテストだけを 10 回回し、1 度も落ちていない。1 回の問いは budget_ms 2500 の
時間切れを待つので、遅い環境では待ち時間の境目にかかりうる(確かめてはいない)。直す仕事は
計画へ積む。直したら、この表から行を消す。

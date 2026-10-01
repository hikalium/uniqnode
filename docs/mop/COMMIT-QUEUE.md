# COMMIT-QUEUE — uniqnode の変更を Commit Queue に main へ入れてもらう

<a id="1df91ea3-07a5-422f-9c9d-83c9f9a7b8ae"></a>

読み手は、uniqnode の変更を main へ入れたいセッション(送り手)と、それを main へ入れる
Commit Queue(CQ)である。lamalium と sumi も同じ CQ が入れる。lamalium の側の手順は lamalium の
docs/mop/WORKTREE.md の Integrate and deploy 節にある。

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

## 宛先

CQ は、claude.ai の lamalium のプロジェクトのスレッド「Commit Queue(mainへの統合)」の
セッションである(2026-10-01 時点)。ホストの Claude のセッションからは、SendMessage で
`bridge:session_0121YxhvvFnBHVUwzPYgESRo` へ送るか、claude-code-remote の send_message で
`session_0121YxhvvFnBHVUwzPYgESRo` へ送る。CQ はその送り手のセッションへ send_message で返す。
Codex には SendMessage が無いので、同じホストの Claude のセッションに中継を頼む(頼み方は
lamalium の docs/mop/WORKTREE.md の Integrate and deploy 節にある)。CQ のセッションが替わったら、プロジェクトの調整役が
新しい宛先を送り手へ知らせる。

## 合格の条件

検査は `cargo test --no-fail-fast` である。次の 3 つが全て揃えば合格とする。

- ビルドが通る。
- 全てのテストのバイナリが最後まで走り終える(異常終了や時間切れが無い)。
- 落ちたテストが、下の「クラウドの CQ の手元で落ちるテスト」の表に名前のあるものだけである。
  vega で回したときは、落ちたテストが 1 つも無い。

## 送り手の手順

1. 最新の `origin/main` からブランチを切って変更を作り、コミットする。
2. 合格の条件を満たすまで `cargo test --no-fail-fast` を回す。
3. 変更が docs/ と Markdown 以外のファイル(コード、テスト、Cargo.toml、Cargo.lock)に触れる
   ときは、vega で `cargo test --no-fail-fast` を回し、落ちたテストが 1 つも無いことを確かめる。
   回すのは、送る時点の `origin/main` を親に持つ commit である。
4. ブランチを origin へ push する。main へは push しない。
5. CQ へ 1 通で送る。中身は、リポジトリ、ブランチ、commit id(40 桁)、目的、依存する変更、
   検査の結果(どの機械で、どの commit で回し、どのテストが落ちたか)、統合の意思である。

差し戻されたら、push 済みのブランチを書き換えない(must/0011)。最新の `origin/main` から新しい
ブランチを切り、自分の変更を cherry-pick して直し、手順 2 からやり直す。

## CQ の手順

1. 送られたブランチを fetch し、先頭が送られた commit id と一致することを確かめる。一致しなければ
   積まずに送り手へ問い合わせる。
2. CQ の手元の写しで、最新の `origin/main` の上へ送り手の commit を古い順に cherry-pick する。
   送り手のブランチも main も書き換えないので、must/0011 の禁じる、push 済みの commit の書き換えには
   当たらない。送り手の commit が既に `origin/main` の真上にあれば、cherry-pick せずにその commit
   のまま進める。衝突したら、直さずに差し戻す。
3. 変更が手順 3 の対象(コードなどに触れる)で、送り手が vega で回した commit の親が今の
   `origin/main` でないときは、統合せずに差し戻し、新しい `origin/main` の上で vega で回し直して
   もらう。CQ の手元では表のテストが落ちるので、vega での結果の代わりにならないからである。
4. その木で `cargo test --no-fail-fast` を回す。表に無いテストが落ちたら、そのテストだけを 1 回
   回し直す。2 回とも落ちたら赤である。2 回目に通ったら、同じ入力で結果が変わるテストがあると
   いうことなので、テストの名前と 2 つの出力を送り手に知らせたうえで統合する。そのテストを直す
   仕事は計画へ積む。
5. 合格なら main を fast-forward で push し、main へ入った commit id を送り手へ知らせる。push までに
   main が動いたら、手順 2 からやり直す。
6. 赤なら push せず、送り手へ差し戻す。差し戻しには、赤の理由(衝突したファイル、ビルドの失敗、
   走り終えなかったテストのバイナリ、落ちたテストの名前)と、出力の要点の行を本文に写して添える。
   送り手は CQ の手元のログを読めないからである。

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

`a_node_that_is_not_registered_gets_no_answer` は、2026-10-01 に a8e287d で 1 回だけ落ち、次の回では
通った。表には入れず、手順 4 の回し直しで扱う。

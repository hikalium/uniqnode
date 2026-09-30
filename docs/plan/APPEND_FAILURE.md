# APPEND_FAILURE: ストアの書き込みに失敗した後、応答済みの書き込みを失わない(設計草案)

<a id="d973833f-4e2b-4fc8-8a49-42f6821b6a7a"></a>

版: 第 19 版(2026-10-01)。第 18 版への Codex と Claude のレビュー(Codex 高 1・Claude 中 3: 書き込みの途中の旗を入れ子と早い戻りとパニックに耐える数え値と RAII の番へ、Codex 高 2・高 3・中 5・Claude 中 2・低 11: backup を、ロックの持ち主が答える永続化済みの境界までの写しと、写し先の tmp/ での段取り・検め・公開の後の削除へ組み直し、ロックの socket に accept の輪と照会の期限、印に開くたびの nonce、Codex 中 4: 2 度目のシグナルを受ける輪を終わり方から分ける、低 9: GC の競合の試験の 2 つの停止点と期待値、Claude 高 1: 開くことの失敗を手綱に記録する、中 5: install の断りの完了条件、低 10: 終わり方の 2 度呼び)を取り込んだ。第 18 版(2026-10-01)。第 17 版への Codex と Claude のレビュー(Codex 高 1: GC の D の packs/ の sync をストアのロックの中で行い、終わり方と排他にする、Codex 中 2〜4・低 6: 旧い名の unit を外す命令の backup の待ち・退避の対象・退避先の一意・パイプラインの失敗、Claude 中 1: 読むだけの走査の Drop は印を書かない、中 2: CLI のシグナルは共有の小さな手綱で閉じる、中 3: serve のログがデータのディレクトリの祖先を作る、中 4: 保留の写し元を backup が写さない、中 5: vega で共有するバイナリと旧い名の unit、低 5: シグナルで閉じたときの終了コード、低 6: 開く途中のシグナル、低 8: 外す命令の字句の試験)を取り込んだ。第 17 版(2026-10-01)。第 14 版(9d04834)への Claude のレビュー(高 1: 保留で開いた serve が sync していない
レコードを複製の口でピアへ渡す、中 1: 開いた後の抜け道とシグナル、中 2: 既存の試験との食い違いと Drop、中 3: 命令ごとの
開き方、中 4: GC の D のディレクトリの sync、中 5: 旧い名の unit からの移行を本番への反映の前提に、低 3〜8)を、
第 16 版に照らし直して取り込んだ。第 16 版(2026-10-01)。第 15 版(3162e7f)への vega の Codex の再レビュー(高なし、中: ストアの障害でない
CLI の誤りでの終わりも無事な終わり方を通す)を取り込んだ。第 15 版(2026-10-01)。第 14 版(9d04834)への vega の Codex の再レビュー(高なし、中: CLI の sync の
差分なしの抜け道)を取り込んだ。第 14 版(2026-10-01)。第 13 版(2e312d3)への vega の Codex の再レビュー(高なし、中 1: 鍵が未確定の
初期化の失敗は保留で開けない、中 2: 明示的な sync の入口で 503、低: 検めを通らない印の完了条件)を
取り込んだ。第 13 版(2026-10-01)。第 12 版(3641dc8)への vega の Codex の再レビュー(高: `Running` を
recover・開くときの sync より前に書く、中: GC の A も書けない状態を見る、低: NoSpace の試験の期待値を
読み直した印で分ける)を取り込んだ。第 12 版(2026-10-01)。第 11 版(48099d2)への vega の Codex の再レビュー(高なし、中 1: 検めを通らない印は
再起動でも戻らない、中 2: MCP の Local の exec による差し替え、低: NoSpace の印の更新の失敗の 3 通り)を
取り込んだ。第 11 版(2026-10-01)。第 10 版(b4f026c)への vega の Codex の再レビュー(高なし、中 2: 印の完全性、
書けない状態で開く道の読み出し)を取り込んだ。第 9 版(98e4b0c)への vega の Codex の再レビュー(高 1: shutdown の順、
高 2: ログの確認は再開の根拠にならない、中: NoSpace の戻し方の矛盾)を取り込んだ。第 8 版(601ae81)への vega の Codex の再レビュー(高 1・2: 印の置き方を
「先に置き、無事な終わり方でだけ外す」へ、中: query の開始後の失敗と非同期)を取り込んだ。第 7 版(b398d42)への vega の Codex の再レビュー(高 1: install の再実行、
高 2: 印を残せない経路、中: query の保存、低: 完了条件の食い違い)を取り込んだ。第 6 版(e34d81d)への vega の Codex の再レビュー(高: install が作る親)と、
crystal の Claude の第 3 版へのレビュー(中 1: 再起動で Io の状態が消える、中 2: query のキャッシュ、低)を
取り込んだ。第 5 版(3152f68)への vega の Codex の再レビュー(高 2: 祖先の sync の例外、
再開時に採用するレコードの中身の sync。中: 鍵の tmp の権限)と Claude のレビュー(中 1・低 4〜7)を
取り込んだ。第 4 版(8d55129)への vega の Codex の再レビュー(高 1: データのディレクトリの
親と祖先の sync、高 2: node_key の中身の sync)を取り込んだ。第 1 版(a9eba65)への Claude のレビュー(高 1・中 4・低 4)、FEED 第 3 版の
再レビューの Claude B・C、FEED 第 4 版の再確認の Claude N-3・N-4、第 2 版(e7c6b61)への Codex の
レビュー(高 1・2、中 3〜5、低 6・7)、第 3 版(c1067dc)への vega の Codex の再レビュー(高 2 の残り、中 5)と
crystal の Codex の低 3 つを取り込んだ。次は Codex の再レビュー。
出所は lamalium の健全性点検(2026-09-30)の項目 10。FEED 第 2 版の再レビューで、Codex(H1a・H1b)と
Claude(N1)が独立に見つけた。[docs/plan/FEED.md](#fa8de6f9-59f8-4512-a815-9f41d305db15) の
「失敗の境界」は、ストア全体の規則としてこの文書を前提にする。FEED より先に、単独で入れる。

## 欠陥(今のコードの事実)

`append_record`(node/src/store.rs の 208 行付近)は、ファイルを append で開いて
`[len][crc][payload]` を `write_all` し、`sync_data` する。どこかで誤りが出ても、何も戻さずに
`Err` を返す。呼び手は 3 つ: `put_object`(pack、917 行付近)、`append_own_record`(自分の ref・
pin・保持表明、1198 行付近)、`ingest_ref_record`(複製の受け側、879 行付近)。GC は
`append_record` を呼ばず、gc.rs の PackWriter がロックの外で tmp/ に新 pack を書く。

1. reflog の尻切れ: `write_all` が途中で失敗すると(ENOSPC が典型)、reflog の末尾に尻切れの
   レコードが残る。呼び手は 500 を返すが、ストアは次の要求で追記を続け、次のレコードは尻切れの
   後ろに書かれて 200 が返る。再起動すると、再生(store.rs の 723 行付近)は最初の壊れた
   レコードで切り、`set_len` で切り詰める。尻切れの後ろの応答済みの書き込みが全部消える。
2. reflog の sync だけの失敗: 完全なレコードがディスクに残ったまま、メモリの seq は進まない。
   次の自分のレコードは同じ seq を使う。再生では先の(応答していない)方が適用され、後の
   応答済みの方は `AlreadyKnown` として黙って捨てられる(776 行付近)。尻切れが無くても失われる。
   `export_ref_records` はディスクから読むので、両方がピアに流れ、ピアでも同じく捨てられる。
3. pack: 1 と同じく尻切れが残る。加えて `active_pack_length` は追記の成功の後にしか進まない
   (918 行付近)ので、次の `put_object` は実際には尻切れの後ろ(O_APPEND なのでファイルの本当の
   末尾)に書かれるのに、索引には古い長さからの offset を記録する。sync だけが失敗した場合も
   同じずれが起きる。`get_object` は ID を検めないので、同じプロセスの中でそのオブジェクトを
   読むと、黙って別のバイトが返る(Corruption で止まるのは gc の compact だけで、fsck は不一致を
   報告の errors に積む)。
4. MANIFEST: `seal_active_pack`(932 行付近)は `sealed_packs` に番号を足してから
   `write_manifest()?` を呼び、失敗しても取り消さない。
5. GC の確定(`gc_commit`、1043 行付近): C-2 の rename そのものが失敗すれば C-3 には進まないが、
   C-2 の rename の後のディレクトリの sync か、C-3 の `write_manifest` が失敗すると、メモリだけが
   途中まで変わる(C-3 では `sealed_packs` が対象を外して新 pack を足した姿、`active_pack_number` は rename 済みの新 pack の番号、
   `active_pack_length` は 0 のまま)。次の `put_object` は中身の入った新 pack の後ろに追記しつつ、
   offset 0 からの位置を索引に記録する(3 と同じ)。後で封印の MANIFEST が成功すると、対象の pack を
   外した MANIFEST が永続し、再起動で対象の pack が残骸として消され、その間に「既に在る」として
   ref を張られたオブジェクトが失われる。ENOSPC の最中に gc を回すのは典型的な運用なので、現実に
   踏む道である。
6. `atomic_write`(398 行付近)の tmp 名は `tmp/write-<pid>` の 1 つだけで、GC の参照表の書き込み
   (gc.rs の 279・712 行付近。ロックの外)と、ロックの中の MANIFEST の書き込みが同時に走ると、
   同じ tmp に 2 つの fd から書きうる。MANIFEST が参照表と混ざれば、次の起動が Corruption で開けない。
7. 新しいセグメントの名前: `append_record` は `create(true)` で新しい pack・reflog のファイルを作り
   うるが、ファイルの `sync_data` だけで親ディレクトリを sync しない。ファイルの sync だけでは
   ディレクトリの項目(名前)の永続は保証されず、ホストが落ちると、応答済みのレコードを持つ
   新しいセグメントが丸ごと見えなくなりうる(Codex 高 2。SPEC §5 の永続の保証に関わる)。
8. `atomic_write` の rename の後のディレクトリの sync が失敗した場合、MANIFEST は既に新しい中身に
   なっているかもしれない(rename はファイルシステムに届いていて、永続だけが不明)。したがって
   MANIFEST の失敗は「前の MANIFEST のまま」とは限らない(Codex 中 3)。
9. fsync の失敗の意味: Linux は fsync に失敗した dirty page を clean と印して捨てることがあり、次の
   fsync は成功を返しうる(fsyncgate)。後の sync の成功は、前の書き込みの永続を保証しない。
   page cache はプロセスの再起動では消えないので、EIO のときは serve の再起動でも末尾の健全さは
   言えない。ENOSPC・EDQUOT は write でも sync でも返りうる(fsync(2))。

## 方針

ストアの持続的な書き込み(reflog と pack の追記、MANIFEST、GC の確定)の I/O の誤りが 1 度でも
出たら、そのプロセスではもう書かない。応答済みの書き込みを守る確実な方法は、「壊れているかも
しれない末尾の後ろに書かない」ことである。

1. `append_record` は、書く前のファイル長を開いたハンドルの fstat(`file.metadata().len()`)で読む
   (メモリの数え値は使わない。reflog には数え値が無い。Claude C)。pack では、読んだ長さが
   `active_pack_length` と違えば、書かずに書けない状態に入る(3 のずれの検算)。`write_all` か
   `sync_data` が誤りを返したら、`set_len(前の長さ)` と `sync_all` を 1 度だけ試み、結果に関わらず
   元の誤りを原因として 2 の状態に記録する(要求へ返すのは 2 の `WritesDisabled`)。切り詰めは再起動の再生を
   軽くするための best-effort であり、正しさは 2 が持つ。
1a. `append_record` が新しいファイルを作ったとき(開く前に在るかを見るか、開いた後の長さが 0 の
   とき)は、最初の追記の sync の後に親ディレクトリも sync し、その成功までを追記の成功とする
   (Codex 高 2)。加えて、ストアを開くときは、書き込みを受け付ける前に packs/・reflog/ とデータの
   ディレクトリ自身を sync する(recover が採用したセグメントの名前を、最初の応答より前に永続させる)。
   ファイルの sync の後・親の sync の前にプロセスが落ち、同じホストで起こし直して追記を続ける形でも、
   名前が永続しないまま応答することが無くなる(第 3 版への Codex の再レビューの高)。開くときの
   sync の失敗は、開くことの失敗として扱う(serve は起動に失敗して理由を言う)。
   開くときに sync するものの全体は次のとおりで、「既に在る」ことを理由に省かない(第 4 版への
   Codex の再レビューの高 1・2):
   - `node_key` の中身(`sync_all`)。新しく作るときは、今の `std::fs::write` をやめ、tmp/ に書いて
     `sync_all` してから rename し、データのディレクトリを sync する。tmp は `create_new(true)` と
     `mode(0o600)` で作り、中身を書くのはその後にする(umask に依らず、秘密鍵が一瞬も他の利用者に
     読めない。第 5 版への Codex の再レビューの中)。途中で落ちても、短い鍵が
     `node_key` の名前で残ることは無い(残るのは tmp/ の残骸で、次に開くとき消す)。鍵が失われると
     同じノードとして署名を続けられないので、pack と同じ重さで守る。鍵が確定していない(`node_key`
     がまだ無い)ストアは、ノード ID を決められないので書けない道(保留)でも開けない。`Running` を
     書いた後に鍵の作成が失敗したら、開くことは失敗し、同じブートの再試行も開くことを断る(保留で
     起きる保証の外)。案内は、ストアを置いたファイルシステムを点検した後に release-hold で印を
     `Clean` にして開き直す(鍵が無いので新しい鍵を作る。まだ何も応答していないので失うものは
     無い)とする(第 13 版への Codex の再レビューの中 1)。
   - recover が採用した pack と reflog のファイルの中身(`sync_data`)。対象は、封印済みとして
     MANIFEST に載っている pack を除く全部(アクティブの pack と reflog の全セグメント)。write_all の
     後・sync の前にプロセスが落ちると、完全なレコードが page cache にだけ在る形で再開し、recover が
     それを採用する。そのまま同じオブジェクトの再送を「既に在る」として 200 にしたり、それを指す ref を
     書いたりすると、後の電源断で応答済みのものが消える(第 5 版への Codex の再レビューの高 2)。
   - packs/・reflog/・データのディレクトリ自身・データのディレクトリの親。祖先を作る道は無くす:
     `Store::open` の `create_dir_all` をやめ、データのディレクトリだけを `mkdir` で作る(親が無ければ
     理由を言って断る)。データのディレクトリの親自身の名前と、根からそこまでの経路は、既に在って
     永続しているものとし、この前提を文書と誤りの文に書く。ログを開く道も同じ規則に揃える(第 17 版への
     Claude のレビューの中 3): 今の serve は `Store::open` より前に `start_logging` を呼び(main.rs の 1897 行
     付近)、log.rs の `Destination::open`(180 行付近)が `create_dir_all` で `<dir>/logs` とその祖先を sync
     せずに作るので、親の無い道を渡すとデータのディレクトリが先にでき、上の「親が無ければ断る」が効かない。
     そこで、ストアを開く命令(serve・書く CLI・MCP の Local)は、データのディレクトリの検め(親が在るか)と
     `mkdir` を `start_logging` より前に行い、断るときは標準エラー(system の unit なら journal)にだけ
     理由を言う。log.rs は `create_dir_all` をやめ、既定の `<dir>/logs` は `mkdir` だけで作る(`--log` で
     渡した道は、親が在ることを求め、無ければログをファイルに残せない旨を言って標準エラーだけで続ける。
     今の失敗の扱いと同じ)。試験は、親の無い道を渡した serve が理由を言って終わり、データのディレクトリも
     `logs/` も作られないことを見る。install も同じ規則に揃える: 今の
     install.rs(2171 行付近)の `create_dir_all` をやめ、足りない経路の要素を上から 1 つずつ `mkdir`
     する。その後、作ったかどうかに依らず、データのディレクトリから根までの全ての要素について、
     その名前を持つディレクトリを下から順に sync する(system の install は root で走るので全て開ける。
     user 単位の install は利用者の権限で走るので、読めない祖先があると開けない。そのときは飛ばさず、
     その祖先の道と理由を言って install の失敗にする。serve の開く道の「親を開けなければ開くことの
     失敗」と同じ規則で、据え付けた後に serve が同じ理由で起きない形を先に言う。第 14 版への Claude の
     レビューの低 8)。前の install が `mkdir` の後・親の sync の前で中断していても、再実行で
     「既に在る」要素の名前まで永続させる。install は serve を起こす前にこれを済ませるので、serve が
     開く時点で親の名前まで永続している(第 6 版・第 7 版への Codex の再レビューの高)。親を開けない(読めない)なら sync できないので、開くことの失敗にする(第 5 版への
     Codex の再レビューの高 1。「読めない祖先は飛ばす」は撤回した: 読めなくても書けて辿れる
     ディレクトリの下には名前を作れるので、飛ばす根拠にならない)。
   順は、recover の削除・切り詰め → 上の中身と名前の sync → 書き込みの受け付け(FEED の起動時の
   前進も含む)とする(Claude 低 7)。方針 5 の印を含めた書ける道の全体の順は、データのディレクトリの
   `mkdir`(無ければ。印の置き場で、ロックの名も正規化した道から作るので先に要る)→ ストアのロック →
   印を読む → `Running` を書く → packs/・reflog/・tmp/ の `mkdir` → tmp/ の片付け → `node_key` の
   読み込みか作成 → recover → 上の sync → 書き込みの受け付け、とする。今の `Store::open`(store.rs の
   474〜488 行付近)は packs/・reflog/・tmp/ の `create_dir_all` をロックより前に、tmp/ の片付けを
   ロックの直後に行うが、どちらもストアを変える操作なので `Running` の後へ移す(第 14 版への Claude の
   レビューの低 6)。
   試験は、初めて作るストアで、ディレクトリと鍵を作った後・sync の前に落として起こし直す形、
   pack の write_all の後・sync の前に落として起こし直し、同じオブジェクトを再送する形(再送の 200 より
   前に sync が済んでいることを数える口で見る)、親が読めない(0333)ときに開くことが理由を言って
   失敗する形、開くたびに上の全部が sync されることを数える形を置く。
2. ストアに `write_failure: Option<WriteFailure>` を持たせる。`WriteFailure` は `{reason: String,
   op: Write | Sync | DirSync | Manifest | GcCommit | LengthMismatch, errno, cleanup: Ok | Failed | NotTried,
   kind: NoSpace | Io, since: unix 秒}`。kind が NoSpace になるのは、op が Write で errno が ENOSPC か
   EDQUOT、かつ切り詰めとその sync が成功した場合だけである。sync の段の誤り(errno が ENOSPC でも)、
   切り詰めの失敗、ディレクトリの sync・MANIFEST・GC の確定の誤りは、すべて Io とする。sync の段の
   ENOSPC は、書いたページが捨てられた可能性があり、単なる空き不足とは扱えない(Codex 高 1)。
   次のどれかが誤りを返したら、そこに入れる:
   - `append_record` の、ファイルを開いた後の誤り(0 バイトで返った write の誤りも含める。
     ENOSPC は続くことが多く、区別しても得が無い)と、1a のディレクトリの sync。開くこと自体の誤りは、1 バイトも書いて
     いないので入れない(EMFILE のような一時的な誤りで書けなくならないように。Claude C)。
   - GC の D の最後の packs/ のディレクトリの sync(下の 3 の D の項)。
   - `write_manifest`。包みは `atomic_write` ではなく `write_manifest` に掛ける(`atomic_write` は gc の
     参照表(gc.rs の 298 行付近)と backup の MANIFEST(backup.rs の 280 行付近)も書き、それらは
     ストアの持続的な書き込みではない。crystal の Claude 低)。
   - `gc_commit` の C-1 より後の全ての誤り(rename・ディレクトリの sync・MANIFEST)。
   書けない状態に入れたその要求自身にも、元の誤りではなく `WritesDisabled`(元の原因を reason と op と
   errno に持つ)を返す。最初の失敗の応答から 503 と案内が届き、送り手と FEED の「書けない 503 なら
   操作者に上げる」がその 1 回目から働く(第 3 版への Codex の再レビューの中 5)。
   以後、ストアの全ての書き込みの入口(`put_object`・`append_own_record`・`ingest_ref_record`・
   pack の封印・`write_manifest`・`gc_try_begin`・`gc_commit`)は、先頭で `write_failure` を見て、
   `StoreError::WritesDisabled` を返す。GC の A と C は、それぞれロックを取り直した直後(`references_since` より前)にも
   見る(A も gc.rs でロックを取り直して `references_since` を呼ぶ。第 12 版への Codex の再レビューの
   中。切り詰めに失敗した尻切れが残っていると、先に Corruption で落ちて理由を取り違えるため。
   Claude 低 4)。3 の fstat の検算の食い違いは、op が LengthMismatch、errno が無し、kind が Io とする
   (Claude 低 6)。
   rendition の GET(`GET /v1/objects/{id}/rendition/{別名}`。写しが無ければ作って `put_object` と
   `set_ref` をする。rendition.rs の 617 行付近)も書き込みの入口に含める。その誤りは
   `RenditionError::Store` を 500 にせず `store_error_response` に回す。書けない間は、作った写しを
   保存せずにそのまま返す(読み出しを続けるという約束を、未生成の写しの閲覧でも守る。Claude 中 1)。
   `POST /v1/query` が取ってきたオブジェクトをストアに置く道(query.rs の 472 行付近)は、置けなければ
   答えの本文を渡せない(`QueryAnswer::Object` は取得元だけを持ち、応答は常に `stored: true` を言う)。
   そこで書けない間は、手元に無いオブジェクトを取りに行く query を `WritesDisabled` の 503 で断る。
   手元に在るものを答える query と検索は続ける(crystal の Claude 中 2、第 7 版への Codex の再レビューの
   中。保存したと偽らない)。query の途中で書けない状態になった形(取得中に別の要求が立てた、
   query 自身の保存が最初の失敗だった)も扱う: 今の peer worker(query.rs の 472 行付近)は保存の誤りを
   `Err(())` に潰してピアの沈黙として扱い、HTTP は 200 を返す。これを改め、query の終わりの状態に
   `WriteFailure` を持たせ、保存に失敗したら再試行をやめて待つ側へ知らせる。`wait:true` は 503 と
   案内を返し、`wait:false` は後の状態の取得(api.rs の 2127 行付近)が失敗の理由と案内を返す(第 8 版
   への Codex の再レビューの中)。`POST /v1/admin/gc` は 503、`uniqnode gc` は終了コード 1 で
   断る。読み出しは続ける。
3. 順を「ディスクが先、メモリが後」にそろえる:
   - `seal_active_pack` は、番号を足した `sealed_packs` の写しで MANIFEST を書き、成功してから
     メモリの `sealed_packs` と `active_pack_number` を進める。
   - `gc_commit` の C-3 も写しで MANIFEST を書き、成功してからメモリの `sealed_packs` を書き換える。
     C-2 の後に C-3 が失敗したら、2 により書けない状態に入る。MANIFEST が新 pack を含むかは結果不明
     である(rename の前の失敗なら含まず、rename の後のディレクトリの sync の失敗なら含みうる。欠陥 8 と
     4 と同じ扱い)。どちらでも次の起動の recover が、その時の MANIFEST を正として決める。書けない
     状態の間も、起動し直した後も、MANIFEST に載っている古い pack は消さない(D は C-3 の成功の後に
     しか走らない)。
     `active_pack_number` を新 pack の次へ進めるのも C-3 の成功の後にする。
   - GC の D(確定の後、ロックの外で古い pack と参照表を消す。gc.rs の 693〜701 行付近)の削除
     (unlink)は、この判定の外に置く。消すのに失敗しても、その GC の実行が誤りで終わるだけでよい(FEED 第 4 版の
     再確認の Claude N-3)。ただし D の最後の packs/ のディレクトリの sync(gc.rs の 701 行付近)の
     失敗は、書けない状態(kind は Io、op は DirSync)に入れる。D がその誤りを受け取ると、ディレクトリの
     errseq に「報告済み」の印が付き、後から開いた fd の sync にはもう返らない。すると後の 1a の
     packs/ の sync(新しいセグメントの名前の永続)が成功を返し、書き戻せなかったディレクトリの更新を
     帳消しにしうる(欠陥 9 と同じ形。第 14 版への Claude のレビューの中 4)。第 16 版までの「D は全部この
     判定の外」をこの項で改めた。この最後の sync は、ストアのロックを取り直してその中で行い、方針 5 の
     「閉じる手綱」から書き込みの番を取って(閉じている状態なら sync をせずに GC を終える)、
     失敗したら同じロックの中で `write_failure` を立て、印に `Io` を書いてから番を返してロックを放す
     (第 17 版への Codex のレビューの高 1)。第 17 版の「ロックの外で sync し、失敗してからロックを取り
     直す」形では、失敗からロックを取り直すまでの間に shutdown がロックを取り、まだ `write_failure` が
     無いので `Clean` を書いて終われた。同じブートの次の起動が書ける道で開き、保留を迂回する。sync を
     ロックの中へ入れると、失敗の記録と無事な終わり方の「`write_failure` を確かめて `Clean` を書く」が
     排他になる(終わり方が先なら D は sync をせずに終わり、D が先なら終わり方は `write_failure` を見て印を
     書き換えない)。閉じている状態で sync を飛ばした形は、D の unlink の名が永続していないかもしれない
     だけで、次の起動の recover が残骸として消す(下の「消し損ね」と同じ扱い)。unlink はロックの外の
     ままでよい。MANIFEST に無い古い pack は、ふつう次の起動の recover が残骸として消す。
     ただし消し損ねたのが封印済みの最大番号の pack で、新 pack も作らなかった形では、次の起動で
     アクティブとして生き返る。失われるものは無く、ゴミが戻るだけで、次の GC が改めて扱う(Claude 低 5)。
   - GC の B(PackWriter の tmp/ への書き込み)は、この判定の外に置く。tmp の書き込みの失敗は
     どこからも参照されず、GC が誤りで終わって tmp を消すだけで済む(C の中で追加の根を新 pack へ
     写す救済のコピー(gc.rs の 660 行付近)も tmp の PackWriter への書き込みなので、同じくこの側に
     入る。crystal の Claude 低)(今の gc.rs の 572 行付近の
     扱い)。ロックも取らない。ただし B の後の `gc_commit` の先頭で `write_failure` を見る(B の間に
     別の要求が書けない状態を立てたかもしれない)。
4. メモリの表は、今と同じく sync まで成功した追記の分しか進めない(`apply_verified` と索引への
   挿入は `append_record` の成功の後)。書けない状態のメモリの表は「最後に成功した追記まで」で、
   ディスクの再生の結果とは違いうる。追記の失敗なら差は最大 1 レコードである(sync が失敗し、
   切り詰めも失敗したが、バイトはディスクに届いていた場合)。その 1 本は 5xx を返した要求の分で、
   応答済みのものではない。書けない状態なので後続の自分のレコードは書かれず、欠陥 2 の seq の
   重なりは起きない。MANIFEST と GC の確定の失敗では、差は 1 レコードに限らない(欠陥 8。MANIFEST が
   新しい中身で永続すれば、再起動で古い pack が残骸として消え、メモリに在った索引の項目が孤児を
   含めて変わる)。それでも応答済みの ref とそこから辿れるオブジェクトは失われない: MANIFEST は
   `gc_commit` の C-3 で、新 pack に生きたオブジェクトを写し終えてから書くからである(Codex 中 3)。
   送り手から見れば「結果不明」で、再送で決着する。重複を作らないと言えるのは、同じ内容の文書の
   再取り込み(ingest の「同じ内容なら ref を書かない」)とオブジェクトの投入(内容アドレス)だけで
   ある。汎用の `PUT /v1/refs`・pin・保持表明の再送は、同じ target でも新しい seq のレコードを足す
   (store.rs の `set_ref` は同じ target を比べない)。害は reflog が 1 本伸びることだけである(Codex 中 5)。
5. 戻るのは、ストアを開き直したときだけである。戻し方は kind で案内を分ける(欠陥 9):
   - NoSpace: 空きを作ってから serve を再起動する。
   - Io: ホストを再起動する(再起動できないときは、umount・fsck・mount し直しの後に release-hold)。serve の再起動だけでは
     page cache に残った「正常に見える」末尾の後ろに書くことになりうる。
   Io の状態は serve の再起動をまたいで持ち越す(crystal の Claude 中 1)。配備のし直し・systemd の
   再起動・OOM のような、ホストを再起動しない serve の起こし直しで書き込みが戻ると、書けなかった
   レコードが page cache に残ったまま後ろに追記を重ね、次のホストの再起動で再生がそこで止まって、
   後ろの応答済みのレコードが切り捨てられうる。
   第 8 版の「誤りを見た後に tmpfs へ印を書く」形は捨てた: 誤りを見てから印を書くまでに殺される
   形、印を書けない形、user の `$XDG_RUNTIME_DIR` がログアウトで消える形のどれでも印が残らない
   (第 8 版への Codex の再レビューの高 1・2)。代わりに、危険な書き込みより前に「動いている」印を
   置き、無事な終わり方でだけ「無事に閉じた」へ書き換える(第 9 版で、印を消す形から、状態を
   書き換える形へ改めた。第 9 版への Codex の再レビューの高 1・中):
   - 印はデータのディレクトリの `open-marker` で、大きさを固定(4 KiB)して作るときに領域を確保
     (`fallocate`)し、以後は同じ場所への上書き(`pwrite` と `fdatasync`)だけで状態を変える。確保
     済みの領域への上書きは新しい領域を要らないので、空きが無いときにも書ける見込みが高い。書けな
     ければ、下の規則でより厳しい側(書けない側)に倒れる。中身は状態・boot_id(`/proc/sys/kernel/
     random/boot_id`)・pid・開くたびの nonce(書ける道で開くたびに `/dev/urandom` から読む 16 バイト。pid の
     使い回しで別のプロセスを取り違えないため。第 18 版への Claude のレビューの低 11)・時刻。状態は `Running`・`NoSpace`・`Io`・`Clean` の 4 つ。形は、先頭の
     魔法の語・版・状態・boot_id(36 文字)・pid・nonce・時刻・それら全体の CRC32 を固定長で並べたもので、
     書くたびに全長を 1 回の `pwrite` で書けたこと(返った長さ)と `fdatasync` の成功を確かめる。
     部分書き込み・sync の失敗・更新の途中の停止で中身が不確かになりうるので、読むときは CRC と
     形を検め、読めない・短い・CRC が合わない・状態が不明な印は `Io` と同じに扱う。「boot_id が
     違う」と言えるのは、検めを通った印の boot_id が今と違うときだけである(第 10 版への Codex の
     再レビューの中 1)。
   - 書く入口の開く道(serve・MCP の Local・書く CLI。どの命令がこの道かは下の「命令ごとの開き方」)は、ストアのロックを取って印を読み、書ける道と
     決めたら、ストアを変える最初の操作(packs/・reflog/・tmp/ の `mkdir`、tmp/ の片付け、recover の
     削除・切り詰め、`node_key` の作成、1a の開くときの sync。順は 1a の末尾)より前に `Running` を書く。
     書けなければ(短い `pwrite`・`fdatasync` の失敗・印を作れない)、開くことの失敗にはせず、書けない道
     (保留、kind は Io、reason は「印を書けない」)へ切り替えて開き、読み出しは答える。第 16 版までの
     「開くことの失敗」は、方針 6 の「読み出しは答え続ける」と食い違っていた(第 14 版への Claude の
     レビューの低 4)。印は書きかけかもしれないので、次の開く道は読み直した印に従う(検めを通らなければ
     保留が続き、release-hold が要る)。鍵が確定していないストアだけは、保留でも開けないので開くことの
     失敗にする(1a の `node_key` の項)。こうすると、その後の
     recover や開くときの sync が失敗して起動が失敗しても、印は今のブートの `Running` のまま残り、
     同じブートの再試行は書けない道(保留)に入る。後の再試行の sync の成功で前の失敗を帳消しに
     しない(第 12 版への Codex の再レビューの高)。印がまだ無い(初めて開く)ときは、印を作って
     `Running` を書き、印とデータのディレクトリを sync してから次へ進む。置き場がデータのディレクトリ
     なので、system・user・手で起こした形・CLI のどれでも同じ場所で、`ReadWritePaths` も今のままで
     よく、ログアウトでも消えない。
   - 書けない状態に入るとき: kind が Io なら、切り詰めを試みるより前に `Io` を書く。kind が NoSpace
     (切り詰めとその sync が成功した場合だけ)なら、その後で `NoSpace` を書く。`NoSpace` の更新が
     失敗したとき、次の開く道が読む印は 3 通りある: 全長の `pwrite` が済んで `fdatasync` だけが失敗
     した形では、同じブートの読み直しで検めを通る `NoSpace` が見えうる。この形は切り詰めが永続して
     いるので、下の「同じ boot_id の `NoSpace`」のとおり普通に開いてよい。`Running` が残る形と、検め
     を通らない印の形は、書き込みを許さない(無事に終わらなかった扱い)。試験の期待値はこの 3 通り
     に分けて書く(第 11 版への Codex の再レビューの低)。
   - 閉じる手綱(第 17 版への Claude のレビューの中 2): 終わり方の判定は、`Store` の本体とは別の、
     `Arc` で共有する小さな構造体 `CloseHandle` で行う。中身は 1 つの `Mutex` の下の「開いている途中」の旗、
     持続的な書き込みの途中の数え値 `writers`(bool ではない)、「閉じている」の段(開いている・閉じる途中・
     閉じた)、`write_failure` の写し、開くことが失敗したか(`open_failed`)、書き込みの途中でパニックしたか
     (`poisoned`)、この開き方が `Running` を書いたか(`wrote_running`)、開くたびの nonce、永続化済みの境界
     (下の「backup の写し元の検め」)、印の fd と、それに付けた `Condvar` である。今の CLI の書き手は `Store` を
     main のスレッドで値として持つ(main.rs の 1394 行付近の put など)ので、シグナルを受けるスレッドは
     `Store` にもストアのロックにも届かない。serve でも、`Mutex<Store>` は query や ingest が長く持ちうる。
     そこで、ストアの全ての持続的な書き込みの入口(方針 2 の一覧と、方針 3 の GC の D の sync)は、先頭で
     手綱から書き込みの番(`WriteGuard`)を取る。番を取るときは手綱の `Mutex` を短く取り、「閉じている」
     (閉じる途中か閉じた)なら `WritesDisabled` と同じ扱いで断り、そうでなければ `writers` を 1 つ増やして
     放す。番の Drop は、成功・`?` の早い戻り(`Invalid` のような障害でない誤りを含む)・パニックの巻き戻しの
     どの道でも `writers` を 1 つ減らし、0 になったら `Condvar` を鳴らす。I/O の誤りのときは、番を落とす前に
     `write_failure` を手綱の写しにも記録する(包みの中で記録してから戻るので、順は必ずそうなる)。巻き戻しの
     中の Drop(`std::thread::panicking()` が真)は、減らす前に `poisoned` を立てる(第 18 版への Claude の
     レビューの中 3)。数え値なので入れ子に耐える(第 18 版への Codex のレビューの高 1): 今のコードは
     `put_object → seal_active_pack → write_manifest` と入れ子に呼ぶ(store.rs の 912・934 行付近)ので、
     入口ごとに bool を立てて降ろすと、内側の終わりで外側も「書き込みなし」に見え、その隙に終わり方が
     `Clean` を書き、外側の `append_record` がその後に走れた。数え値なら内側の終わりでは 0 にならない。
     GC の C は C-1 から C-3 の最後の MANIFEST まで 1 つの番で覆い、D の sync も 1 つの番で覆う。serve の
     `Mutex<Store>` の外で走る書き手(GC の D、周期の書き手)と重なっても、数え値がそれぞれを数える。
     閉じる途中の段は `writers` が 0 のときにしか立たないので、外側の番を持つ間の内側の番は断られない。
     手綱の `Mutex` を持つのは旗と数え値の読み書きと印の `pwrite`・`fdatasync` の間だけで、書き込みそのものの
     間は持たない。
   - 無事な終わり方は、次の順で行う(第 9 版への Codex の再レビューの高 1): 手綱の `Mutex` を取り、
     「開いている途中」が降り `writers` が 0 になるまで `Condvar` で待ち、閉じる途中の段へ移す(以後の
     全ての書き込みの入口と GC の確定は `WritesDisabled` と同じ扱いで断る)→ `wrote_running` が真で、
     `write_failure`・`open_failed`・`poisoned` のどれも無いことを確かめる → `Clean` を書く → 閉じた段へ
     移し、結果(`Clean` を書いたか、残したか)を手綱に記録する。`write_failure`・`open_failed`・`poisoned` の
     どれかがあるか、この開き方が `Running` を書いていなければ(保留で開いた、読むだけの走査で開いた)、
     印を書き換えずに閉じた段へ移す。
     開くことの失敗(第 18 版への Claude のレビューの高 1): `Running` を書いた後に recover・開くときの sync・
     `node_key` の作成が失敗したら、開く道は「開いている途中」を降ろす前に、同じ `Mutex` の中で `open_failed`
     を立てる。第 18 版では手綱に「失敗した」の状態が無く、開く途中にシグナルが待っていると、終わり方は
     `wrote_running` が真で `write_failure` が無いのを見て `Clean` を書き、その `exit(0)` が main の `exit(1)`
     より先に走りえて、保留を迂回できた。`open_failed` は `write_failure` と同じく `Clean` を書かせない。
     2 度呼び(第 18 版への Claude のレビューの低 10): ExecStop の後の SIGTERM、Drop とシグナル、shutdown の
     2 本のように、終わり方は何度も呼ばれうる。最初の呼び手だけが上の手順を踏み、後の呼び手は閉じた段に
     なるまで `Condvar` で待って、記録された結果を受け取る(印を 2 度書かない)。プロセスを終える形の
     入口 `finish(code)` は、この結果を受け取ってから終了コードを決める: 印を残したときは、呼び手の渡した
     code に依らず 1 にする(シグナルの閉じ方の 0 や 130 が、失敗の 1 を追い越さない)。`finish` は 1 つの
     `Once` で囲み、最初の呼び手だけが `process::exit` を呼び、後の呼び手はプロセスが終わるまで戻らない
     (2 つのスレッドから同時に `exit` を呼ばない)。閉じる途中か閉じた段の手綱に対して開く道(MCP の
     Local の `exec` の失敗の後の開き直しなど)が走ったら、開くことを断る。
     手綱は `Store` を持たないスレッドからも使えるので、シグナルのスレッドも shutdown の handler も
     同じ関数を呼べる。この関数はストアのロック(serve の `Mutex<Store>`)を持たずに呼ぶ(GC の D の sync は
     ストアのロックの中で書き込みの番を取るので、ストアのロックを持ったまま番の返りを待つと互いに
     待つ)。shutdown の API は今、終了の旗を返して http.rs が
     `process::exit(0)` を呼ぶだけなので、この順を踏む関数を置き、shutdown の API・CLI の終わり・
     MCP の Local の終わりから呼ぶ。CLI の終わりは正常終了に限らない: ストアを開いた後の誤りでの
     終わり(例: 今の CLI の sync は、相手に繋がらないときやハッシュが合わないときに main.rs から直接
     `process::exit(1)` する)も、終了コードを決めた後にこの関数を通してから終える。`Clean` へ書き換え
     るかは終了コードでなく `write_failure` の有無で決める。ストアを開いた後の CLI の `process::exit`
     は、この関数を通る 1 つの終わり方に寄せる(第 15 版への Codex の再レビューの中)。
     ストアを開いた後に抜ける道は CLI の sync だけではない(第 14 版への Claude のレビューの中 1)。
     serve は今、ストアを開いてから束縛し(main.rs の 1908〜1949 行付近)、束縛の失敗は
     `process::exit(1)` で抜ける。main.rs には誤りでの `process::exit` が 39 箇所あり、CLI の SIGINT
     (Ctrl-C)と MCP の Local の SIGTERM も今は手続きなしで落ちる。これらがどれも `Running` を残すと、
     ポートの塞がりや取り込みの中断のたびにホストの再起動が要る。そこで次の 3 つにする:
     (a) 起動の時点で分かる断りは `Store::open` より前に済ませる。serve は束縛と、API_AUTH の A1・A2 の
     起動時の断り(束縛先の字面、Linux 以外、許す集合に overflowuid)を先に行い、全部通ってから
     ストアを開く。「listening on」の 1 行はストアを開き終えた後に出す(今の順の理由は、待つ側が
     1 行を見た後で落ちられると騙されることなので、1 行の位置を保てば束縛を先にしてよい。束縛から
     開き終わるまでに来た接続はカーネルの backlog で待つ)。
     (b) ストアを開いた後の抜け道は、全部この関数を通る 1 つの終わり方に寄せ、`process::exit` を直接
     呼ばない(`?` で main から返る道は下の Drop が受ける)。
     (c) serve・書く CLI・MCP の Local は SIGTERM と SIGINT を受け、この関数を通して終える。依存を
     足さない制約の下で、std と `extern "C"` の `pthread_sigmask`(スレッドを作る前に 2 つを塞ぐ)と、
     専用のスレッドの `sigwait`(または signalfd)で受ける。受ける輪と閉じる仕事は分ける(第 18 版への
     Codex のレビューの中 4): 第 18 版では受けたスレッド自身が終わり方を呼んで `Condvar` で待つので、その間の
     2 度目は(全スレッドで塞いであるので)保留のまま誰にも受け取られず、「すぐ終える」へ進めなかった。
     受ける輪のスレッドは、1 度目を受けたら `CloseHandle` の複製を持つ閉じる役のスレッドを 1 本作って
     `finish(シグナルに応じた code)` を任せ、自分はすぐ `sigwait` へ戻る。閉じる役は、書き込みの番が全部
     返るのを待って閉じる途中の段へ移し、`Clean` を書いてからプロセスを終える(第 17 版への Claude の
     レビューの中 2)。その間に 2 度目の SIGINT か SIGTERM を受けたら、受ける輪が印を書き換えずにすぐ終える
     (`extern "C"` の `_exit` で、atexit の処理を走らせない。終了コードは serve と MCP の Local が 1、書く CLI が
     128 とシグナルの番号の和)。`Running` が残り、同じブートの次の起動は保留になる(止まらない取り込みを
     操作者が打ち切る道)。閉じる役が `Clean` を書いている最中に `_exit` が走っても、印は 1 回の `pwrite` の
     全長か CRC で検め、書きかけなら保留に倒れる(安全な側)。
     シグナルが開く途中(`Running` を書いた後、recover や開くときの sync の最中)に来たら、閉じる役は手綱の
     「開いている途中」が降りるまで待ち、開き終えてから同じ手順で閉じる。開くことが失敗したら、開く道が
     「開いている途中」を降ろす前に `open_failed` を立てているので、閉じる役は `Clean` を書かず、`finish` は
     1 で終える(印は `Running` のまま。第 17 版への Claude のレビューの低 6、第 18 版への Claude のレビューの
     高 1)。main の側の開くことの失敗も同じ `finish` を通るので、どちらが先でも終了コードは 1 である。シグナルを塞ぐ
     のはストアを開くより前(スレッドを作る前)なので、開く途中のシグナルは既定の動作で落とさず、保留中
     として待つスレッドへ届く。
     終了コード(第 17 版への Claude のレビューの低 5): serve と MCP の Local は、SIGTERM でも SIGINT でも、
     `Clean` を書いて閉じたら 0 で終える。systemd は SIGTERM の後の 143 を失敗と数えるので、stop のたびに
     unit が failed にならないよう 0 にする(テンプレートに `SuccessExitStatus=143` は足さない)。
     `write_failure` があって印を残したとき・`Clean` を書けなかったときは 1 で終え、systemd の記録にも
     失敗として残す。書く CLI は、閉じ方が無事でも作業を途中で打ち切られているので、128 とシグナルの
     番号の和(SIGINT なら 130)で終え、呼んだスクリプトが中断を見分けられるようにする。
     Store の Drop は、`wrote_running` が真で(書ける道で開き、この開き方自身が `Running` を書いた)、
     `write_failure`・`open_failed`・`poisoned` が無く、持続的な書き込みの途中でなく(手綱の `writers` が 0)、
     パニックの巻き戻しの中でもなければ、上の終わり方(プロセスを終えない形)を呼んで `Clean` を書く(第 14 版への Claude のレビューの中 2)。読むだけの
     走査で開いたストアと保留で開いたストアの Drop は、印を一切書かない(第 17 版への Claude のレビューの
     中 1。読むだけの走査は印を読まず `write_failure` も持たないので、`write_failure` の有無だけで決めると、
     保留のストアに `uniqnode status` を当てた終わりに Drop が `Clean` を書き、保留を解いてしまう)。CLI が `?` で main から
     返る形と、試験がプロセスの中でストアを落として開き直す形(tests/gc_crash.rs の 167 行付近など)が、
     これで無事な終わり方になる。書き込みの途中やパニックで落ちたストアは `Running` のまま残す
     (書きかけの末尾を無事と言わない)。書き込みの番を持ったスレッドがパニックで巻き戻った後は、
     `poisoned` が立つので、serve の終わり方も SIGTERM の閉じ方も `writers` の数え値を待ち続けず(番の Drop が
     減らしている)、印を残して 1 で終える(第 18 版への Claude のレビューの中 3。第 18 版では、早い戻りや
     パニックで旗が立ったままになり、終わり方が SIGKILL まで待った)。
     shutdown の API は、`Clean` を書いて `fdatasync` が成功してから応答を返す。今の http.rs(167〜172
     行付近)は応答を書いてから `process::exit(0)` するので、この関数は応答を作る前(handler の中)で
     呼ぶ。`write_failure` があって書き換えなかったとき・`Clean` を書けなかったときは、応答の本文に
     そう言う(`"marker":"clean"` か `"marker":"left"` と理由)。ExecStop はその本文を journal に残す。
     ExecStop が返った時点で、印が `Clean` で永続しているか、残した理由が journal にあるかのどちらかに
     なるようにするため(第 14 版への Claude のレビューの低 7)。MCP の Local は、実行ファイルが更新されると自分を
     `exec` で差し替え(mcp.rs の `exec_replacement`)、成功すれば終わりの処理へ戻らない。そこで
     `exec` の前にもこの関数を呼んで `Clean` を書き、ストアを閉じる。`exec` が失敗して旧イメージの
     まま続けるときは、開く道(ロックを取り直し、印を読み、書ける道なら `Running` を書く)をやり直し、
     `Running` を書けるまで書き込みを受け付けない(第 11 版への Codex の再レビューの中 2)。
     query の保存・health・他の要求のどれも、`Clean` を書いた後には持続的な書き込みをしない(閉じて
     いる状態が手綱の中で断るため)。
   - 開くとき: 印が無い、`Clean`、または boot_id が今と違う(ホストを再起動した後で page cache は
     消えている)なら、recover に任せて普通に開く。同じ boot_id の `NoSpace` なら、切り詰めは永続して
     いるので普通に開く(空きを作ってから serve を再起動する、の戻し方がそのまま効く)。同じ boot_id
     の `Running`(前のプロセスが殺された・落ちた)と `Io` と、検めを通らない印は、書けない状態
     (kind は Io、reason は「前のプロセスが無事に終わらなかった」か元の WriteFailure)で開き、読み
     出しは答える。
   - 開く道は、印を読んだ直後に 2 つに分かれる(第 10 版への Codex の再レビューの中 2)。書ける道は、
     先に `Running` を書き(上の項)、その後で今の recover(残骸の削除・尻切れの切り詰めとその sync)
     と 1a の開くときの sync を行い、全部が済んでから書き込みを受け付ける。書けない道(上の保留)は、印を書き換えず、削除・切り詰め・sync を一切しない読むだけの
     走査で、最初の尻切れのところまでを索引に載せ、読み出しの表を作る。今の `Store::open` は必ず
     recover を呼んで切り詰めと sync をするので、読むだけの走査を別の関数に分ける。こうして、書き
     込みや sync が失敗し続ける同じブートの中でも、serve は起きて status と既存のデータを答える。
     1a の「開くときの sync の失敗は、開くことの失敗」は書ける道だけに掛かる。
   - 保留で開いた serve は、ピアが引く複製の口 `GET /v1/replication/signers` と
     `GET /v1/replication/refs`(api.rs の 260・2137 行付近。中身は store.rs の `signers` と
     `export_ref_records`、1344〜1354 行付近)に 503 と理由を返す(第 14 版への Claude のレビューの高 1)。
     読むだけの走査は尻切れの手前までの完全なレコードを採用するが、中身を sync しない。Io の後に完全な
     レコード R(seq n)が page cache にだけ残っていると、同じブートで起こし直した serve は保留で開き、
     走査が R を採用して `signer_last_seq` が n になるので、export の `seq <= signer_last_seq` の絞りでは
     止まらない。ピア(例えば graph_a の毎分の pull)が R を取り込んだ後にホストを再起動すると、R は
     ディスクから消え、こちらは seq n を別のレコード R′ に使い直し、ピアは R′ を `AlreadyKnown` として
     捨てるので、同じ署名者の seq が 2 系統に割れる。503 は、ストアを書ける道で開き直す(1a の開くとき
     の sync が採用したレコードの中身を永続させる)まで続く。保留は同じプロセスの中では解けないので、
     保留で開いた serve の間はずっと 503 である。実行中に書けない状態へ入った serve は、メモリの表が
     sync まで成功した分しか進まないので、この口を閉じなくてよい(「外から見える形」の export の絞りで
     足りる)。
   - 戻し方(Io と、無事に終わらなかったもの): 既定はホストの再起動だけである。カーネルのログに
     誤りが無いことは再開の根拠にならない(書き戻しの誤りは報告された後は見えなくなり、sync の段の
     ENOSPC も Io に入る。第 9 版への Codex の再レビューの高 2)。ホストを再起動できないときの代わりは、
     ストアを置いたファイルシステムを umount して fsck し、mount し直して page cache を捨てた後に、
     `uniqnode store release-hold --data-dir <dir>` で印を `Clean` にすることである。検めを通らない
     印(読めない・短い・CRC が合わない・状態が不明)は boot_id を信用しないので、ホストを再起動
     しても同じ判定のまま保留が続く。この形は、再起動の後でも release-hold が要る(案内の文もそう
     書き分ける。第 11 版への Codex の再レビューの中 1)。release-hold は
     ストアのロックを取って行い(serve や他の CLI が開いている間は断る)、fsck と mount し直しは
     操作者の作業として案内の文に書く。
   - 代価: 同じブートの中で serve が SIGKILL・OOM・異常終了で落ちると、I/O の誤りが無くてもホストを
     再起動するまで書けない状態で起きる。systemd の stop と restart を無事な終わり方にするため、unit
     に `ExecStop=`(主の口へ `POST /v1/admin/shutdown` を送り、終わりを待つ)を足す。今の serve は
     SIGTERM を扱わないので、これが無いと stop のたびに `Running` が残る。上の (c) で serve が SIGTERM を
     受けるようになれば、ExecStop が主の口に届かないとき(API_AUTH の判定の枠が塞がっている、など)も
     systemd の SIGTERM が無事な終わり方を通す。ExecStop は結果を journal に残す第一の道として置き続ける。
     `TimeoutStopSec` を過ぎて SIGKILL になった形は、書けない状態で起きる(安全な側)。
   試験は、実際の unit での stop と start で書けるまま起きること、SIGKILL の後に同じ boot_id で書け
   ない状態で起きること、boot_id が違えば普通に開くこと(boot_id の読み口は debug ビルドで差し替え
   られるようにする)、`Running` を書けないときに書けない道で開いて読み出しが答えること(鍵が確定して
   いないストアでは開くことが失敗すること)、Io の後の shutdown で印が
   `Io` のまま残ること、NoSpace の後に serve を再起動すると書けること、検めを通らない印がホストの
   再起動(boot_id の差し替え)の後も保留のままで release-hold で戻ること、MCP の Local の `exec` に
   よる差し替えの後に書けるまま起きることと、`exec` の失敗の後に `Running` を書き直すまで書き込みが
   断られること、`NoSpace` の更新の失敗を 3 通り(`fdatasync` だけの失敗・`Running` が残る・検めを
   通らない)に注入して、それぞれ上の期待値になること、shutdown と query の保存・
   health・他の書き込みの要求を競わせて `Clean` の後に持続的な書き込みが走らないこと、release-hold が
   開いているストアでは断られることを見る。CLI の sync を相手に繋がらない形で失敗させて終了コード 1
   で終えた後、同じ boot_id で開き直すと書けるまま開くことも見る。加えて、印の部分書き込み・sync の失敗・更新の途中の
   停止(注入)の後の開く道が、読み直した印に従うこと(検めを通る `NoSpace` なら書ける、`Running`
   か検めを通らない印なら書き込みを許さない)、`Running` を書いた後の recover・
   開くときの sync のそれぞれの失敗(注入)の後に、同じ boot_id の再試行が書けない道で開くこと、
   `node_key` の作成の失敗(注入)の後は同じ boot_id の再試行が開くことを断り、release-hold の後に
   開けること、CRC の合わない印が Io に扱われること、書けない
   道で開いたとき削除・切り詰め・sync の呼び出しが 0 回であること、書き込みと sync が失敗し続ける
   注入の下でも同じブートで status と既存のデータの読み出しが答えることを見る。
   空きを足しても自動では戻らない(プロセスの中では末尾の健全さを言えないため)。
6. serve は終了しない。検索と読み出しは答え続け、書き込みだけが断られる。終了して systemd の
   Restart=on-failure に開き直させる案は採らない: 空きが無いままなら再起動の輪になり、読み出しも
   止まる(FEED の Claude N2・N3)。

## 命令ごとの開き方

「書く CLI」がどれかを、名前でなく実際にストアを変えるかで決める(第 14 版への Claude のレビューの中 3)。
今の main.rs は `open`(無ければ初期化)と `open_existing`(在るストアだけ)を分けている(667〜681
行付近)が、この分け方は書くかどうかと一致しない: gc の CLI は `open_existing` で開いて書き(1645 行
付近)、fsck・status・get・refs と backup の写し先の検め(backup.rs の 226 行付近の `verify_copy`。
install の確認も同じ関数)は、`Store::open` の recover で削除・切り詰め・sync をする。S1 の後は次の
表のとおりにする。

| 命令 | 今の開き方 | S1 の後 |
|---|---|---|
| serve、mcp の Local(`--serve-url` なし) | `Store::open` | 印の手順(書ける道か保留) |
| init・put・set-ref・pin・correct・ingest(道と URL)・ingest-annotations・fetch・sync・flood | `open` | 印の手順 |
| ingest-git | `open`(`--serve-url` なら開かない) | 印の手順(`--serve-url` なら開かない) |
| gc | `open_existing`(書く) | 印の手順(在るストアだけ) |
| status・get・refs・fsck | `open_existing`(recover が削除・切り詰め・sync をする) | 読むだけの走査 |
| embed | `open`(ストアは読むだけで、書くのは derived/ のベクトルの控え) | 読むだけの走査(在るストアだけ) |
| backup の写し先の検め(`verify_copy`) | `open_existing`(写し先の尻切れを切り詰める) | 読むだけの走査 |
| mcp の Forward、viewer、install | 開かない(install はロックを探るだけ) | 開かない |

読むだけの走査は、方針 5 の書けない道と同じ関数で、ストアのロックは取る(serve と同時に走らない今の
形を保つ)が、印を読み書きせず、`mkdir`・tmp/ の片付け・削除・切り詰め・sync を一切しない。保留の
ストアにも答える。init と status は今 1 つの腕(main.rs の 1382 行付近)なので分ける。backup の
`torn_tails_cut` は、ファイルが縮んだかではなく、走査の有効な長さとファイルの長さの差から出す(写し先の
アクティブのセグメントは次回の backup が丸ごと写し直すので、切り詰めなくてよい)。

backup の写し元の検め(第 17 版への Claude のレビューの中 4): backup は写し元をストアのロックなしに生の
ファイルとして写す(backup.rs の 233 行付近)。保留や実行中の Io の後には、sync していない完全な
レコード R が page cache にだけ在りうるので、それを写すと、写しが R を持ち、ピアは同じ seq の R′ を持つ
という割れを、写しから戻したときに持ち込む。

第 18 版の「写す前と後に印を読んで検める」形は捨てた(第 18 版への Codex と Claude のレビュー)。serve が
レコード R を書き終えて sync している最中に backup が R を写して後の検めまで済ませると、その時点の印も
持ち主も正常な `Running` なので採られ、その後に sync が失敗して R が切り詰められると、採った写しにだけ R が
残る(Codex 高 2)。また後の検めで断っても、今の backup.rs は写しを写し先の packs/ へ rename 済み(104・147 行
付近)で、旧 pack の削除も MANIFEST より前に済ませている(260 行付近)ので、「その回を採らない」つもりで前回の
正常な写しまで壊していた(Codex 高 3、Claude 中 2)。所有者の照会も、accept しないロックの socket に繋ぐだけ
だったので、未 accept の接続が backlog を埋めた(Codex 中 5)。S1 では次の 3 つに組み直す。

(1) 永続化済みの境界を持ち主から受け取る。手綱は「永続化済みの境界」を持つ: 封印済みの pack と reflog の
番号の一覧、アクティブの pack の番号と長さ、reflog の各セグメントの番号と長さ。`append_durable` は、sync
(と新しいセグメントなら親の sync)が成功した後、書き込みの番を返す前に、手綱の `Mutex` の中でそのセグメントの
長さを進める。封印と GC の C-3 は、MANIFEST の書き込み(rename の後のディレクトリの sync まで)が成功して
メモリを進めた後に一覧を進める。したがって境界の内側のバイトは、全部 sync の成功を見た追記のもので、後の
失敗の切り詰め(`set_len(書く前の長さ)`。書く前の長さは境界以上)も recover の切り詰めも届かない。
境界は、ロックの socket に accept の輪を置いて答える。ストアのロックを取ったプロセス(書ける道・保留・読むだけの
走査のどれも)は、ロックの `UnixListener` を持つ小さなスレッドを 1 本置き、繋いできた接続を 1 本ずつ accept
して、1 行の答え(形の版、pid、nonce、開き方: `write`・`hold`・`readonly`、`write_failure` の kind か none、
上の境界)を書き込みの期限(1 秒)つきで書いて閉じる。答えを作るときは手綱の `Mutex` を短く取るだけで、
ストアのロック(`Mutex<Store>`)は取らない。これで install の探りを含む全ての接続が accept されて閉じられ、
backlog が溜まらない(Codex 中 5)。ロックを放すときは、listener を `shutdown` して accept を起こし、輪の
スレッドが listener を閉じて終わるのを待つ(輪が fd の複製を持ったままロックが残る形を作らない)。
照会する側(backup)は、非ブロックの connect と全体の期限(5 秒)を持つ: `EAGAIN`(backlog が満ちている)は
100 ms おきに期限まで繋ぎ直し、`ECONNREFUSED` は持ち主が無いと判定し、繋がったら `SO_PEERCRED` の uid が
自分の euid と同じことを確かめてから(違えば名を横取りされている。API_AUTH の「既知の限界」)、残りの時間を
読みの期限にして 1 行を読む。期限を過ぎたら理由を言って 1 で終える(S1 より前のバイナリで走る serve は
accept しないので、ここで断る。serve を S1 のバイナリで起こし直せば戻る)。

(2) 何をどこまで写すかを決める。
- 持ち主が `write` で答え、`write_failure` が無いか NoSpace で、答えの nonce と pid が写し元の `open-marker`
  (検めを通る同じ boot_id の `Running`)のものと一致するとき: 答えの境界のとおり写す。封印済みの一覧の pack は
  丸ごと(不変)、アクティブの pack と reflog の各セグメントは境界の長さまで(ファイルがそれより長くても、
  その先は読まない)。写し先の MANIFEST は、写し元の MANIFEST のバイト列ではなく、答えの封印済みの一覧から
  ストアと同じ直列化の関数で作る(照会と MANIFEST の読みの間に封印が進んでも食い違わない)。写す途中で GC の D が
  答えにある封印済みの pack を消していたら、写しは誤りで終わる(今と同じく次の回に回る)。境界の内側は写す
  途中に持ち主が替わっても変わらないので、写した後の検めは要らない。
- 持ち主が `hold` で答えたとき、`write_failure` が Io のとき、nonce か pid が印と食い違うときは、写さずに
  理由を言って 1 で終える(保留の serve のメモリの表は sync していないものを含みうる)。
- 持ち主が無い(`ECONNREFUSED`)か `readonly` で答えたとき(読むだけの走査がロックを持つ間は書き手は開けない):
  印が、書ける道がそのまま開いてよい形(印が無い、`Clean`、boot_id の違う印、同じ boot_id の `NoSpace`)の
  ときだけ写し、同じ boot_id の `Running` と `Io`、検めを通らない印は断る。写す長さは、写し始めに各
  ファイルを開いて fstat した長さまでとする。写し終えた後に印のバイト列を読み直し、写し始めのものと
  1 バイトでも違えば(書き手が開いて `Running` を書いた。書き手はストアを変える前に必ず印を書き、開くたびに
  nonce が替わるので、開いて閉じただけでも違う)、その回の写しを採らない。印が無いストアは、写し終えた後も
  印が無いことを求める。

(3) 写し先の tmp/ で段取りし、検めてから公開し、古いものは公開の後に消す。backup は写し先の
`tmp/stage-<nonce>/` に、ストアの形(MANIFEST・packs/・reflog/・設定のファイル)をそろえる: 今回写す
セグメントと設定はそこへ写して `sync_all` し、写し先に同じ大きさで在る封印済みの pack はそこへ hard link で
入れる(同じファイルシステムの中なので写さない)。その stage を読むだけの走査で開いて fsck し(今の `verify_copy`
と同じ検め)、(2) の後の検めも済ませる。どれかで断ったら stage を消すだけで終え、写し先の packs/・reflog/・
MANIFEST には一度も触れていない。通ったら公開する: stage の今回写したセグメントと設定を写し先の同じ名へ
rename し、packs/・reflog/・写し先のディレクトリを sync する → stage の MANIFEST を写し先の MANIFEST へ
rename してディレクトリを sync する → その後で、写し先にあって新しい MANIFEST の封印済みにもアクティブにも
無い pack を 1 本ずつ名を言って消し、packs/ を sync する → stage を消す。公開の途中で落ちた写し先は、
「前回の MANIFEST + 前回の pack が全部 + 今回の新しいセグメントの一部」で、前回の MANIFEST が指すものは
消えていない。MANIFEST の後・古い pack を消す前に落ちた形は、今の recover(store.rs の 620〜647 行付近)が
MANIFEST に無く最後でもない pack を残骸として扱い、次の backup も消す。backup.rs の冒頭の「残骸の削除は
MANIFEST を据える前」と docs/mop/BACKUP.md の同じ規則は、この順へ改める(S1 で一緒に直す文書)。

## 外から見える形

- `StoreError::WritesDisabled(WriteFailure)` を足す。HTTP では 503、本文は
  `{"error":"writes disabled (<kind>): <理由>; <5 の案内>","writes_disabled":{"reason":…,"op":…,
  "kind":"no_space"|"io","since":…}}`。案内を `error` の文字列そのものに入れるのは、MCP の Forward が
  応答の `error` だけを写す(mcp.rs の 1268 行付近)ので、そこで落ちないようにするためである。変換は
  api.rs の `store_error_response` の 1 箇所で、主の口・読み口の書く口(`--agent-writable`、グラフの
  書く口)・MCP(Local は api の関数を直接呼び、Forward は serve の 503 を受ける)が同じ変換を通る。
  `POST /v1/sync` は今 sync.rs の 393 行付近でストアの誤りを一律 500 にしているので、
  `SyncError::Store(WritesDisabled)` を同じ変換へ寄せる(Codex 中 4)。ただし今の sync は、差分が無い
  か必要なオブジェクトが揃っていれば取り込みを呼ばずに 200 を返す(sync.rs の 104・365 行付近)ので、
  変換だけでは 503 にならない。判定は、HTTP の `POST /v1/sync` と CLI の `uniqnode sync` が共に呼ぶ
  `sync_from_peer` の先頭に置き、`writes_disabled()` なら差分を見る前に `SyncError::Store(WritesDisabled)`
  を返す(HTTP は 503、CLI は理由を言って終了コード 1。今の CLI は HTTP の道を通らず main.rs から
  `sync_from_peer` を直接呼ぶため、入口を 1 つにする。第 14 版への Codex の再レビューの中)。周期の同期は、
  呼ぶ前に同じ判定を見て黙って 1 回飛ばす(記録は遷移の 1 行だけ)として区別する(第 13 版への
  Codex の再レビューの中 2)。
- `/v1/status` に `writes_disabled`(null か上の形)を載せる。FEED の `GET /v1/feeds/{c}` も同じ形を
  写す。健全性の集計にも 1 項目足す(遷移で 1 度だけ記録。should/0129)。
- serve の記録に、入ったときに 1 行残す(`uniqnode: store: writes disabled: …`)。
- `Store::writes_disabled()` を公開し、ストアの外から周期的に書くもの(health.rs の保持表明、
  sync.rs の取り込み)は、書けない間は書かずに飛ばす。飛ばしたことは、入った遷移の 1 行と status が
  示すので、周期ごとには記録しない。
- CLI(`uniqnode ingest` などストアを直接開くもの)は、誤りを言って終了コード 1 で終える。
- `export_ref_records` は、全ての署名者について `seq <= signer_last_seq[signer]` に絞る(応答して
  いない 1 本をピアへ流さない。複製で受けた他の署名者のレコードも、追記の成功の後にメモリへ適用する
  ので同じことが起きる。Codex 低 6)。この絞りが効くのはメモリの表が sync の済んだ分だけのときで、
  保留で開いた serve は複製の口そのものを 503 にする(方針 5。第 14 版への Claude のレビューの高 1)。

## 関連して直すもの

- 書き込みの口を寄せる: 3 つの呼び手が `append_record` を直接呼ぶのをやめ、`Store` のメソッド
  `append_durable(&mut self, path, payload)` を通す。MANIFEST と `gc_commit` のファイル操作は、同じ
  `write_failure` を立てる小さな包み(`self.durable(|| …)`)を通す。
- `atomic_write` の tmp 名を呼び出しごとに一意にする(`write-<pid>-<単調増加の数>`)(欠陥 6)。
- 失敗の注入(外部の crate を足さない): gc.rs の `UNIQNODE_GC_CRASH_AFTER` と同じく、
  `cfg(debug_assertions)` のビルドだけが読む環境変数 `UNIQNODE_APPEND_FAULT=<種類>:<何回目>` を
  `append_durable` と包みが見る。release には入らない。実プロセスの serve を立てる node/tests の
  api 系のテストから使える。種類は次の 10 個:
  - `before`: 1 バイトも書かずに誤り(errno は EIO)。
  - `torn:<n>`: n バイトだけ書いて誤り(errno は ENOSPC。切り詰めが成功するので kind は NoSpace)。
  - `sync`: 全部書いてから sync の位置で誤り(切り詰めは成功させる)。
  - `sync-keep`: 全部書いてから sync の位置で誤り、切り詰めも失敗させる(完全な 1 本がディスクに
    残る。方針 4 の「最大 1 レコード違いうる」を作る)。
  - `manifest`: 次の `write_manifest` の rename の前で誤り。
  - `manifest-dirsync`: rename の後のディレクトリの sync で誤り(MANIFEST は新しい中身になる)。
  - `dirsync`: 新しいセグメントを作った追記の、親ディレクトリの sync で誤り。
  - `nospace-sync`: sync の段で ENOSPC を返す(kind が Io になることを見る)。
  - `crash-before-dirsync`: 新しいセグメントの最初の追記の、ファイルの sync の後・親の sync の前で
    プロセスを abort する(開く道のディレクトリの sync を確かめる)。
  - `gc-dirsync`: GC の D の最後の packs/ のディレクトリの sync で誤り(第 14 版への Claude の
    レビューの中 4)。
  ストアの層のテストは、同じ注入をプロセスの中の `#[doc(hidden)]` のメソッドでも掛けられるように
  する(tests/gc.rs と同じ形)。

## やらないこと

- 書き込みのやり直し(再試行)。やり直しても、前の失敗の後の末尾の健全さは言えない。
- 1 要求の中の複数の書き込みの原子化(文書 1 本の取り込みは、チャンクのオブジェクトを何本も
  置いてから ref を 1 本書く)。途中で書けなくなれば、置いたオブジェクトは孤児として残り、
  次の gc が回収する。複数の ref にまたがる原子性が要るのは FEED だけで、それは FEED.md の pending の
  規則が持つ。
- 導出データ(ベクトルの控え embed.rs)の追記は、壊れても作り直せるので対象外。運用の記録
  (log.rs)は作り直せない履歴だが、失ってもストアの整合性には響かないので対象外とする(Codex 低 7)。

## 完了条件(テストで固定する)

- `before`・`torn`・`sync` を、reflog の追記と pack の追記のそれぞれに、実プロセスの serve で掛ける。
  どれでも、失敗した要求は 5xx、その後の書き込み(admin/gc を含む)は全部 503 と理由で断られ、
  読み出し(文書・検索)は答え続け、`/v1/status` の `writes_disabled` が kind を持つ。
- 注入の前に 200 を返した書き込みは、ストアを開き直した後に 1 つも消えていない。
- `sync-keep` の後に開き直すと、残った 1 本が適用され、応答済みのものは欠けず、自分の署名の seq に
  重なりが無い。`export_ref_records` は書けない間、その 1 本を返さない。
- pack の `torn` の後、同じプロセスの中で既存のオブジェクトの読み出しが正しい中身を返す(offset の
  ずれた索引項目が生まれない)。
- `manifest` を封印に掛けると、メモリの `sealed_packs` が進まず、書けない状態に入り、開き直した
  ストアが一貫している。`gc_commit` の C-2 の後と C-3 に掛けても、開き直した後に失われる
  オブジェクトが無い(tests/gc_crash.rs と同じ検め方)。
- GC の B の途中の tmp の書き込みの失敗と、D の削除の失敗は、書けない状態を立てない。D の packs/ の
  sync の失敗(`gc-dirsync`)は、kind が io・op が DirSync の書けない状態を立て、印が `Io` になる
  (第 14 版への Claude のレビューの中 4)。
- `manifest-dirsync` を封印と `gc_commit` の C-3 に掛けて開き直しても、応答済みの ref から辿れる
  オブジェクトが全部読める。
- `torn` の ENOSPC は kind が no_space、`nospace-sync` と `sync` と `dirsync` は io になり、503 の本文と
  MCP の誤りの文に案内が載る。`POST /v1/sync` も、差分が無いときを含めて 503 を返し、CLI の
  `uniqnode sync` も差分が無いときを含めて理由を言って終了コード 1 で終える。
- 複製の受け側(`ingest_ref_record`)に `sync-keep` を掛けると、`export_ref_records` はその 1 本を返さない。
- 書けない状態に入れた最初の要求そのものが 503 と案内を返す。
- kind が Io の状態で serve だけを起こし直すと、書けない状態で開く(`open-marker` が効く)。
  boot_id が変わった後か、release-hold の後は書ける。
- install が足りない経路を作って据え付けるとき、作った各要素の親が serve の起動より前に sync される。
- 書けない状態で、手元に無いオブジェクトを取りに行く `POST /v1/query` は 503 と案内を返し、
  `stored: true` を言わない。手元に在るものの query は答える。取得の途中で書けない状態になったとき、
  `wait:true` は 503、`wait:false` は状態の取得が理由と案内を返す。
- 前の install を `mkdir` の後・親の sync の前で中断させてから install を再実行すると、データの
  ディレクトリから根までの全ての名前が serve の起動より前に sync される。
- 書けない状態で、未生成の rendition の GET が写しを返し(保存はしない)、rendition の道の書き込みの
  失敗が 503 と案内になる。
- 新しいセグメントの最初の追記の、ファイルの sync の後・親の sync の前でプロセスを止め(注入の
  `crash-before-dirsync`)、同じ boot_id で起こし直すと書けない状態で開く。boot_id を差し替えて
  (ホストの再起動に当たる)起こし直して追記を続けると、開く道が packs/・reflog/ を sync してから
  書き込みを受け付ける(sync の呼び出しを数える口で確かめる)。
- 開き直したときの `writes_disabled` は、読み直した印で分かれる: 検めを通る `NoSpace` なら null に
  戻る(印の更新の `fdatasync` だけが失敗した形も含む)。検めを通る `Io` と `Running`(前のプロセスが
  無事に終わらなかったもの)は、同じ boot_id の間は Io のまま開き、ホストの再起動(boot_id が変わる)
  か、(umount・fsck・mount し直しの後の)release-hold の後に null に戻る。検めを通らない印は、ホストを
  再起動しても Io のまま開き、release-hold の後にだけ null に戻る(第 13 版への Codex の再レビューの低)。
- 保留で開いた serve は `/v1/replication/signers` と `/v1/replication/refs` に 503 と理由を返す。
  reflog に `sync-keep` を掛けて完全な 1 本を残した後、同じ boot_id で serve を起こし直すと保留で開き、
  別のノードの `POST /v1/sync` でその serve を引いても何も取り込まれない。boot_id を差し替えて開き直すと
  200 に戻る(第 14 版への Claude のレビューの高 1)。
- 塞がったポートへの束縛と、A1・A2 の起動時の断りで serve が終わった後、同じ boot_id で開き直すと
  書けるまま開く(ストアを開く前に断っている)。取り込みの途中の CLI に SIGINT を、serve と MCP の
  Local に SIGTERM を送ると `Clean` を書いて終わり、同じ boot_id で書けるまま開く。ストアを開いた後の
  誤りでの終わり(注入で起こす)も同じ(第 14 版への Claude のレビューの中 1)。
- プロセスの中でストアを落として開き直すと書けるまま開き(Drop が `Clean` を書く)、持続的な書き込みの
  途中で(注入で)パニックさせて落としたストアは、同じ boot_id で保留に開く。子プロセスを SIGKILL・abort
  で落として開き直す既存の試験(tests/crash.rs の 19〜60 行付近の flood の 5 周、tests/gc_crash.rs の
  各相の abort の後の開き直しと、180〜190 行付近の `a_second_gc_finishes_the_job`)は、落とした後の
  開き直しの前に debug の口で boot_id を差し替える(ホストの再起動に当たる)。差し替えない形は、同じ
  boot_id で保留に開くことを見る別の試験にする。boot_id の口は、プロセスの中で開く試験にも効くように、
  debug ビルドだけが読む StoreConfig の欄か環境変数で与える(第 14 版への Claude のレビューの中 2)。
- 保留のストアに fsck・status・get・refs と backup の写し先の検めを当てると、切り詰め(`set_len`)・
  削除・sync・`mkdir` の呼び出しが 0 回で、印も変わらない。gc の CLI は印の手順を通る(第 14 版への
  Claude のレビューの中 3)。
- GC の D の最後の sync の競合を、2 つの停止点(debug の口)で試す(第 17 版への Codex のレビューの高 1、
  第 18 版への Codex のレビューの低 9)。停止点 P1 は D が書き込みの番を取る前(unlink の後)、P2 は
  `gc-dirsync` を掛けた sync が失敗した直後・記録の前である。
  - P1 で止めて shutdown(と SIGTERM)を先に通してから D を進める: D は閉じている段を見て sync をせずに
    終わり、印は `Clean`、`write_failure` は無く、同じ boot_id で書ける道に開く(sync を省いただけで失敗は
    起きていない)。
  - P1 で止めて D を先に進め、P2 で止めてから shutdown を送る: 終わり方は D の番が返るまで待ち、D が
    `write_failure` と印の `Io` を記録して番を返した後に `write_failure` を見て印を書き換えない。印は `Io`、
    shutdown の本文は `"marker":"left"`、同じ boot_id で保留に開く。
  - `gc-dirsync` を掛けずに P1 から D を先に通した形は、印が `Clean` で書ける道に開く。
- 保留のストアと書けるストアに `uniqnode status`・fsck・get・refs を当てて終えた後、印のバイト列が
  変わっていない(読むだけの走査の Drop は印を書かない。第 17 版への Claude のレビューの中 1)。
- `Store` を main のスレッドで値として持つ書く CLI(put、取り込み)に、持続的な書き込みの途中と、書き込みの
  間の両方で SIGINT を送ると、途中の書き込みが終わってから `Clean` を書いて 130 で終わる。2 度目の SIGINT
  ではすぐ終わり、印は `Running` のまま残る。serve に SIGTERM を送ると `Clean` を書いて 0 で終わる。開く
  途中(recover の最中で止める注入)に SIGTERM を送ると、開き終えてから `Clean` を書いて終わる(第 17 版への
  Claude のレビューの中 2・低 5・低 6)。閉じる役が書き込みの番の返りを待っている間(持続的な書き込みの途中で
  止める注入)に 2 度目の SIGTERM を送ると、受ける輪がすぐ終え(serve は 1)、印は `Running` のまま残る(第 18 版
  への Codex のレビューの中 4)。
- 開く途中(recover の最中で止める注入)に SIGTERM を送り、その後の recover に失敗を注入すると、プロセスは 1 で
  終わり、印は `Running` のまま残り、同じ boot_id の次の起動は保留に開く。開くときの sync の失敗の注入でも同じ
  (第 18 版への Claude のレビューの高 1)。
- 書き込みの番の数え値: `put_object` が封印と `write_manifest` を入れ子に呼ぶ間(内側の `write_manifest` の
  終わりで止める注入)に shutdown を送ると、外側の追記が終わるまで `Clean` が書かれない。書き込みの入口が
  `Invalid` で早く戻った後の shutdown は待たずに `Clean` を書く。書き込みの番を持ったまま(注入で)パニック
  させた serve に SIGTERM を送ると、待ち続けずに印を残して 1 で終わる(第 18 版への Codex のレビューの高 1、
  Claude のレビューの中 3)。
- 終わり方の 2 度呼び: shutdown の直後に SIGTERM を送る形と、シグナルの閉じ方の最中に Drop が走る形で、印の
  `pwrite` が 1 回だけで、終了コードが 1 つに決まる。`write_failure` があるとき、どちらの順でも終了コードは 1
  (第 18 版への Claude のレビューの低 10)。
- 親の無い道を渡した serve が理由を言って終わり、データのディレクトリも `logs/` も作られない(第 17 版への
  Claude のレビューの中 3)。
- backup が、写し元の印が `Io`・検めを通らない・持ち主の無い同じ boot_id の `Running` のとき、持ち主が
  `hold` で答えるとき、答えの nonce が印と食い違うときに、理由を言って終了コード 1 で終え、写し先の
  ファイル(packs/・reflog/・MANIFEST・設定)のバイト列と一覧が前と同じである。serve が動いている(その serve が
  書いた `Running`)ときと `Clean` のときは写す(第 17 版への Claude のレビューの中 4)。
- serve がレコードを書き終えて sync の手前で止まる(注入)間に backup を走らせると、写しはそのレコードを含まず
  (答えの境界の手前まで)、その後に sync の失敗を注入して切り詰めさせても、写しと写し元の再生が同じ seq で
  割れない(第 18 版への Codex のレビューの高 2)。
- 持ち主の無いストアで、backup の写しの途中(注入で止める)に書く CLI が開いて閉じると、backup は印の変化を
  見てその回を採らず、写し先は前と同じである。
- stage の検めの後・公開の前(注入で止める)に断らせると、写し先は前と同じ。公開の途中(セグメントの rename の
  後、MANIFEST の前)と、MANIFEST の後・古い pack の削除の前で abort させても、写し先は開けて fsck が通り、
  前回か今回の MANIFEST が指すオブジェクトが全部読める(第 18 版への Codex のレビューの高 3、Claude の中 2)。
- ロックの socket に 1,000 本繋いで閉じる(読まない)接続を送った後も、backup の照会と install のロックの探りが
  期限の内に答えを得る。accept しない相手(注入で輪を止める)への照会は 5 秒の期限で理由を言って終わる。
  `SO_PEERCRED` の uid が違う相手には断る(第 18 版への Codex のレビューの中 5、Claude のレビューの低 11)。
- shutdown の応答を受け取った時点で、印が `Clean` で永続している(応答を書く前に `fdatasync` が済んだ
  ことを数える口で見る)。`write_failure` があれば本文が `"marker":"left"` と理由を言う(第 14 版への
  Claude のレビューの低 7)。
- install の断り(第 18 版への Claude のレビューの中 5): 据え先の unit_dir に旧い名の unit
  (`uniqnode-serve.service` など LEGACY_UNITS のどれか)を置いた試験の据え先で `install --instance graph_a` を
  打つと、理由(旧い名の unit が残っていて、共有のバイナリを差し替えると旧い serve の次の起動が新しい
  バイナリを旧い unit で走らせる)と docs/mop/SYSTEMD.md の「旧い名の unit から移る」を言って断り、据え先の
  バイナリのバイト列と mtime が前と同じである。旧い名が無ければ据わる。この試験は、この直しを含む段
  (API_AUTH の A1 か S1 のうち先に入る方)の完了条件にする。

## 段取り

| 段 | 中身 | 大きさ |
|---|---|---|
| S1 | `append_durable` と包みへの寄せ、新しいセグメントのディレクトリの sync、`WriteFailure` と `WritesDisabled`、fstat と切り詰めの試み、封印と `gc_commit` の順の入れ替え、GC の D の packs/ の sync の失敗を書けない状態へ、`atomic_write` の tmp 名、周期的な書き手の飛ばし、export の絞り、注入、HTTP と MCP の 503、status と健全性の欄、`open-marker`(固定長・CRC・boot_id)と開く道の 2 分岐、読むだけの走査の関数と「命令ごとの開き方」の表のとおりの付け替え、保留の serve の複製の口の 503、release-hold の CLI、無事な終わり方の関数と 1 つの終わり方への寄せ、`CloseHandle`(閉じる手綱。書き込みの番の数え値と RAII、`open_failed`・`poisoned`、終わり方の 2 度呼びと `finish` の `Once`)、Store の Drop(`wrote_running` のときだけ)、SIGTERM と SIGINT の受け取り(`extern "C"`。受ける輪と閉じる役の分離)と終了コード、GC の D の sync をストアのロックの中へ、ログを開く前のデータのディレクトリの検めと log.rs の `create_dir_all` の撤去、印の nonce、ロックの socket の accept の輪と永続化済みの境界の答え、backup の境界までの写しと写し先の tmp/ の stage・検め・公開の後の削除、serve の束縛と A1・A2 の起動時の断りを開くより前へ、unit の ExecStop(system と user の `uniqnode-serve@.service`)、install の経路の 1 つずつの `mkdir` と根までの sync、`node_key` の tmp(`create_new`・0600・sync・rename)、テスト、SPEC §5(永続化)への 1 段落 | M |

S1 で一緒に直す文書(第 14 版への Claude のレビューの低 3): 配る unit のコメント「書き込み途中で
裂かれていても fsck なしで回復する(node/tests/crash.rs)」(docs/mop/systemd/system/uniqnode-serve@.service
の 37 行付近と user の 42 行付近。同じ boot_id の SIGKILL の後は保留で起きる、へ)、docs/mop/SYSTEMD.md
(116 行付近の同じ根拠、「停止」の「SIGTERM を受け取る手続きを持たないので即座に落ちる」、「更新」の
restart の説明、保留と release-hold の案内)、backup の `not_copied`(backup.rs の 285 行付近。
`open-marker` は写さない既知の名として扱い、毎回「知らない名前」に出さない)、backup.rs の冒頭の
コメントと docs/mop/BACKUP.md の「残骸の削除は MANIFEST を据える前」「ロックを取らない」(backup の写し元の検めの
(1)〜(3) の順と、持ち主への照会へ。第 18 版への Codex と Claude のレビュー)。

S1 はレビューで高の指摘が無いと確かめてから入る。FEED の F2 は S1 を前提にする。

本番への反映の前提(第 14 版への Claude のレビューの中 5): vega の主のストアの serve は、今も旧い名の
unit `uniqnode-serve.service`(と `uniqnode-viewer.service`・`uniqnode-backup.timer`)で動いている
(2026-10-01 に `systemctl cat` と /etc/systemd/system/ の一覧で確かめた。graph_a・graph_b は既に
`@` の名)。ExecStop は `@` のテンプレートにだけ入り、install は旧い名が据え先に残っていると何も置かずに
断る(install.rs の 43〜50 行付近の LEGACY_UNITS)ので、更新の手順(install の打ち直し)が通らない。
バイナリだけを差し替えると、ExecStop の無い unit で動き、SIGTERM の受け取りに漏れがあれば restart の
たびに `Running` が残って保留で起きる。S1 を本番へ入れる前に、docs/mop/SYSTEMD.md の「旧い名の unit
から移る」で `uniqnode-serve@default` ほかへ移す。操作者への sudo の依頼として、実行するホスト(vega)と
貼れる命令を添えて渡す。

vega のバイナリの共有(第 17 版への Claude のレビューの中 5): 旧い名の `uniqnode-serve.service` と、
graph_a・graph_b の `uniqnode-serve@graph_*.service` は、同じ /home/hikalium/.local/bin/uniqnode を走らせる
(2026-10-01 に /etc/systemd/system/ の drop-in の ExecStart= で確かめた)。install は既定でないインスタンス
を据えるときは旧い名を「取り合わない」として残したまま進み(install.rs の 2087〜2096 行付近)、共有の
バイナリを差し替える。したがって graph_* の据え直しを先に打つと、旧い serve の次の起動(restart・
再起動)が S1 のバイナリを ExecStop の無い unit で走らせる(API_AUTH の A2 のバイナリなら AF_NETLINK が
無く主の口が全部 403)。vega では、どのインスタンスの install も `@default` への移行の後に打つ。加えて、
install は据え先に旧い名の unit が残っている間は、インスタンスに依らず共有のバイナリの差し替えを断る
(理由と docs/mop/SYSTEMD.md の「旧い名の unit から移る」を言う)ように直す。これは API_AUTH の A1 と
この S1 のうち先に入る方に含め、完了条件の「install の断り」の試験で閉じる(第 18 版への Claude のレビューの
中 5)。

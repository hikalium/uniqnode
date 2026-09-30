# APPEND_FAILURE: ストアの書き込みに失敗した後、応答済みの書き込みを失わない(設計草案)

<a id="d973833f-4e2b-4fc8-8a49-42f6821b6a7a"></a>

版: 第 10 版(2026-10-01)。第 9 版(98e4b0c)への vega の Codex の再レビュー(高 1: shutdown の順、
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
     同じノードとして署名を続けられないので、pack と同じ重さで守る。
   - recover が採用した pack と reflog のファイルの中身(`sync_data`)。対象は、封印済みとして
     MANIFEST に載っている pack を除く全部(アクティブの pack と reflog の全セグメント)。write_all の
     後・sync の前にプロセスが落ちると、完全なレコードが page cache にだけ在る形で再開し、recover が
     それを採用する。そのまま同じオブジェクトの再送を「既に在る」として 200 にしたり、それを指す ref を
     書いたりすると、後の電源断で応答済みのものが消える(第 5 版への Codex の再レビューの高 2)。
   - packs/・reflog/・データのディレクトリ自身・データのディレクトリの親。祖先を作る道は無くす:
     `Store::open` の `create_dir_all` をやめ、データのディレクトリだけを `mkdir` で作る(親が無ければ
     理由を言って断る)。データのディレクトリの親自身の名前と、根からそこまでの経路は、既に在って
     永続しているものとし、この前提を文書と誤りの文に書く。install も同じ規則に揃える: 今の
     install.rs(2171 行付近)の `create_dir_all` をやめ、足りない経路の要素を上から 1 つずつ `mkdir`
     する。その後、作ったかどうかに依らず、データのディレクトリから根までの全ての要素について、
     その名前を持つディレクトリを下から順に sync する(install は root で走るので全て開ける。開けなければ
     install の失敗にする)。前の install が `mkdir` の後・親の sync の前で中断していても、再実行で
     「既に在る」要素の名前まで永続させる。install は serve を起こす前にこれを済ませるので、serve が
     開く時点で親の名前まで永続している(第 6 版・第 7 版への Codex の再レビューの高)。親を開けない(読めない)なら sync できないので、開くことの失敗にする(第 5 版への
     Codex の再レビューの高 1。「読めない祖先は飛ばす」は撤回した: 読めなくても書けて辿れる
     ディレクトリの下には名前を作れるので、飛ばす根拠にならない)。
   順は、recover の削除・切り詰め → 上の中身と名前の sync → 書き込みの受け付け(FEED の起動時の
   前進も含む)とする(Claude 低 7)。
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
   - `write_manifest`。包みは `atomic_write` ではなく `write_manifest` に掛ける(`atomic_write` は gc の
     参照表(gc.rs の 298 行付近)と backup の MANIFEST(backup.rs の 280 行付近)も書き、それらは
     ストアの持続的な書き込みではない。crystal の Claude 低)。
   - `gc_commit` の C-1 より後の全ての誤り(rename・ディレクトリの sync・MANIFEST)。
   書けない状態に入れたその要求自身にも、元の誤りではなく `WritesDisabled`(元の原因を reason と op と
   errno に持つ)を返す。最初の失敗の応答から 503 と案内が届き、送り手と FEED の「書けない 503 なら
   操作者に上げる」がその 1 回目から働く(第 3 版への Codex の再レビューの中 5)。
   以後、ストアの全ての書き込みの入口(`put_object`・`append_own_record`・`ingest_ref_record`・
   pack の封印・`write_manifest`・`gc_try_begin`・`gc_commit`)は、先頭で `write_failure` を見て、
   `StoreError::WritesDisabled` を返す。GC の C は、ロックを取った直後(`references_since` より前)にも
   見る(切り詰めに失敗した尻切れが残っていると、先に Corruption で落ちて理由を取り違えるため。
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
   - GC の D(確定の後、ロックの外で古い pack と参照表を消す。gc.rs の 681〜699 行付近)も、この
     判定の外に置く。消すのに失敗しても、その GC の実行が誤りで終わるだけでよい(FEED 第 4 版の
     再確認の Claude N-3)。MANIFEST に無い古い pack は、ふつう次の起動の recover が残骸として消す。
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
     random/boot_id`)・pid・時刻。状態は `Running`・`NoSpace`・`Io`・`Clean` の 4 つ。
   - 書く入口の開く道(serve・MCP の Local・書く CLI)は、ストアのロックを取った後、書き込みを受け
     付ける前に `Running` を書く。書けなければ開くことの失敗にする。置き場がデータのディレクトリ
     なので、system・user・手で起こした形・CLI のどれでも同じ場所で、`ReadWritePaths` も今のままで
     よく、ログアウトでも消えない。
   - 書けない状態に入るとき: kind が Io なら、切り詰めを試みるより前に `Io` を書く。kind が NoSpace
     (切り詰めとその sync が成功した場合だけ)なら、その後で `NoSpace` を書く。`NoSpace` を書けな
     ければ `Running` のまま残り、次の開く道は無事に終わらなかった扱い(書けない側)になる。
   - 無事な終わり方は、次の順で行う(第 9 版への Codex の再レビューの高 1): ストアのロックの中で
     「閉じている」状態へ移す(以後の全ての書き込みの入口と GC の確定は `WritesDisabled` と同じ扱いで
     断る。ロックを取れた時点で、進行中の持続的な書き込みは無い。書き込みは全てこのロックの中で
     行うため)→ `write_failure` が無いことを確かめる → `Clean` を書く → プロセスを終える。
     `write_failure` があれば印を書き換えずに終える。shutdown の API は今、終了の旗を返して http.rs が
     `process::exit(0)` を呼ぶだけなので、この順を踏む関数を置き、shutdown の API・CLI の正常な
     終わり・MCP の Local の終わりから呼ぶ。query の保存・health・他の要求のどれも、`Clean` を書いた
     後には持続的な書き込みをしない(閉じている状態がロックの中で断るため)。
   - 開くとき: 印が無い、`Clean`、または boot_id が今と違う(ホストを再起動した後で page cache は
     消えている)なら、recover に任せて普通に開く。同じ boot_id の `NoSpace` なら、切り詰めは永続して
     いるので普通に開く(空きを作ってから serve を再起動する、の戻し方がそのまま効く)。同じ boot_id
     の `Running`(前のプロセスが殺された・落ちた)と `Io` は、書けない状態(kind は Io、reason は
     「前のプロセスが無事に終わらなかった」か元の WriteFailure)で開き、読み出しは答える。
   - 戻し方(Io と、無事に終わらなかったもの): 既定はホストの再起動だけである。カーネルのログに
     誤りが無いことは再開の根拠にならない(書き戻しの誤りは報告された後は見えなくなり、sync の段の
     ENOSPC も Io に入る。第 9 版への Codex の再レビューの高 2)。ホストを再起動できないときの代わりは、
     ストアを置いたファイルシステムを umount して fsck し、mount し直して page cache を捨てた後に、
     `uniqnode store release-hold --data-dir <dir>` で印を `Clean` にすることである。release-hold は
     ストアのロックを取って行い(serve や他の CLI が開いている間は断る)、fsck と mount し直しは
     操作者の作業として案内の文に書く。
   - 代価: 同じブートの中で serve が SIGKILL・OOM・異常終了で落ちると、I/O の誤りが無くてもホストを
     再起動するまで書けない状態で起きる。systemd の stop と restart を無事な終わり方にするため、unit
     に `ExecStop=`(主の口へ `POST /v1/admin/shutdown` を送り、終わりを待つ)を足す。今の serve は
     SIGTERM を扱わないので、これが無いと stop のたびに `Running` が残る。`TimeoutStopSec` を過ぎて
     SIGKILL になった形は、書けない状態で起きる(安全な側)。
   試験は、実際の unit での stop と start で書けるまま起きること、SIGKILL の後に同じ boot_id で書け
   ない状態で起きること、boot_id が違えば普通に開くこと(boot_id の読み口は debug ビルドで差し替え
   られるようにする)、`Running` を書けないときに開くことが失敗すること、Io の後の shutdown で印が
   `Io` のまま残ること、NoSpace の後に serve を再起動すると書けること、shutdown と query の保存・
   health・他の書き込みの要求を競わせて `Clean` の後に持続的な書き込みが走らないこと、release-hold が
   開いているストアでは断られることを見る。
   空きを足しても自動では戻らない(プロセスの中では末尾の健全さを言えないため)。
6. serve は終了しない。検索と読み出しは答え続け、書き込みだけが断られる。終了して systemd の
   Restart=on-failure に開き直させる案は採らない: 空きが無いままなら再起動の輪になり、読み出しも
   止まる(FEED の Claude N2・N3)。

## 外から見える形

- `StoreError::WritesDisabled(WriteFailure)` を足す。HTTP では 503、本文は
  `{"error":"writes disabled (<kind>): <理由>; <5 の案内>","writes_disabled":{"reason":…,"op":…,
  "kind":"no_space"|"io","since":…}}`。案内を `error` の文字列そのものに入れるのは、MCP の Forward が
  応答の `error` だけを写す(mcp.rs の 1268 行付近)ので、そこで落ちないようにするためである。変換は
  api.rs の `store_error_response` の 1 箇所で、主の口・読み口の書く口(`--agent-writable`、グラフの
  書く口)・MCP(Local は api の関数を直接呼び、Forward は serve の 503 を受ける)が同じ変換を通る。
  `POST /v1/sync` は今 sync.rs の 393 行付近でストアの誤りを一律 500 にしているので、
  `SyncError::Store(WritesDisabled)` を同じ変換へ寄せる(Codex 中 4)。
- `/v1/status` に `writes_disabled`(null か上の形)を載せる。FEED の `GET /v1/feeds/{c}` も同じ形を
  写す。健全性の集計にも 1 項目足す(遷移で 1 度だけ記録。should/0129)。
- serve の記録に、入ったときに 1 行残す(`uniqnode: store: writes disabled: …`)。
- `Store::writes_disabled()` を公開し、ストアの外から周期的に書くもの(health.rs の保持表明、
  sync.rs の取り込み)は、書けない間は書かずに飛ばす。飛ばしたことは、入った遷移の 1 行と status が
  示すので、周期ごとには記録しない。
- CLI(`uniqnode ingest` などストアを直接開くもの)は、誤りを言って終了コード 1 で終える。
- `export_ref_records` は、全ての署名者について `seq <= signer_last_seq[signer]` に絞る(応答して
  いない 1 本をピアへ流さない。複製で受けた他の署名者のレコードも、追記の成功の後にメモリへ適用する
  ので同じことが起きる。Codex 低 6)。

## 関連して直すもの

- 書き込みの口を寄せる: 3 つの呼び手が `append_record` を直接呼ぶのをやめ、`Store` のメソッド
  `append_durable(&mut self, path, payload)` を通す。MANIFEST と `gc_commit` のファイル操作は、同じ
  `write_failure` を立てる小さな包み(`self.durable(|| …)`)を通す。
- `atomic_write` の tmp 名を呼び出しごとに一意にする(`write-<pid>-<単調増加の数>`)(欠陥 6)。
- 失敗の注入(外部の crate を足さない): gc.rs の `UNIQNODE_GC_CRASH_AFTER` と同じく、
  `cfg(debug_assertions)` のビルドだけが読む環境変数 `UNIQNODE_APPEND_FAULT=<種類>:<何回目>` を
  `append_durable` と包みが見る。release には入らない。実プロセスの serve を立てる node/tests の
  api 系のテストから使える。種類は次の 9 つ:
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
- GC の B の途中の tmp の書き込みの失敗と、D の削除の失敗は、書けない状態を立てない。
- `manifest-dirsync` を封印と `gc_commit` の C-3 に掛けて開き直しても、応答済みの ref から辿れる
  オブジェクトが全部読める。
- `torn` の ENOSPC は kind が no_space、`nospace-sync` と `sync` と `dirsync` は io になり、503 の本文と
  MCP の誤りの文に案内が載る。`POST /v1/sync` も 503 を返す。
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
- 開き直したときの `writes_disabled` は、kind で分かれる: NoSpace だったものは null に戻る。Io
  だったもの(と前のプロセスが無事に終わらなかったもの)は、同じ boot_id の `open-marker` がある間は
  Io のまま開き、ホストの再起動か(umount・fsck・mount し直しの後の)release-hold の後に null に戻る。

## 段取り

| 段 | 中身 | 大きさ |
|---|---|---|
| S1 | `append_durable` と包みへの寄せ、新しいセグメントのディレクトリの sync、`WriteFailure` と `WritesDisabled`、fstat と切り詰めの試み、封印と `gc_commit` の順の入れ替え、`atomic_write` の tmp 名、周期的な書き手の飛ばし、export の絞り、注入、HTTP と MCP の 503、status と健全性の欄、テスト、SPEC §5(永続化)への 1 段落 | M |

S1 はレビューで高の指摘が無いと確かめてから入る。FEED の F2 は S1 を前提にする。

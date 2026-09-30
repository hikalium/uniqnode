# APPEND_FAILURE: ストアの書き込みに失敗した後、応答済みの書き込みを失わない(設計草案)

<a id="d973833f-4e2b-4fc8-8a49-42f6821b6a7a"></a>

版: 第 3 版(2026-10-01)。第 1 版(a9eba65)への Claude のレビュー(高 1・中 4・低 4)、FEED 第 3 版の
再レビューの Claude B・C、FEED 第 4 版の再確認の Claude N-3・N-4、第 2 版(e7c6b61)への Codex の
レビュー(高 1・2、中 3〜5、低 6・7)を取り込んだ。次は Codex の再レビュー。
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
   元の誤りを返す。切り詰めは再起動の再生を軽くするための best-effort であり、正しさは 2 が持つ。
1a. `append_record` が新しいファイルを作ったとき(開く前に在るかを見るか、開いた後の長さが 0 の
   とき)は、最初の追記の sync の後に親ディレクトリも sync し、その成功までを追記の成功とする
   (Codex 高 2)。
2. ストアに `write_failure: Option<WriteFailure>` を持たせる。`WriteFailure` は `{reason: String,
   op: Write | Sync | DirSync | Manifest | GcCommit, errno, cleanup: Ok | Failed | NotTried,
   kind: NoSpace | Io, since: unix 秒}`。kind が NoSpace になるのは、op が Write で errno が ENOSPC か
   EDQUOT、かつ切り詰めとその sync が成功した場合だけである。sync の段の誤り(errno が ENOSPC でも)、
   切り詰めの失敗、ディレクトリの sync・MANIFEST・GC の確定の誤りは、すべて Io とする。sync の段の
   ENOSPC は、書いたページが捨てられた可能性があり、単なる空き不足とは扱えない(Codex 高 1)。
   次のどれかが誤りを返したら、そこに入れる:
   - `append_record` の、ファイルを開いた後の誤り(0 バイトで返った write の誤りも含める。
     ENOSPC は続くことが多く、区別しても得が無い)と、1a のディレクトリの sync。開くこと自体の誤りは、1 バイトも書いて
     いないので入れない(EMFILE のような一時的な誤りで書けなくならないように。Claude C)。
   - `write_manifest`(`atomic_write`)。
   - `gc_commit` の C-1 より後の全ての誤り(rename・ディレクトリの sync・MANIFEST)。
   以後、ストアの全ての書き込みの入口(`put_object`・`append_own_record`・`ingest_ref_record`・
   pack の封印・`write_manifest`・`gc_try_begin`・`gc_commit`)は、先頭で `write_failure` を見て、
   `StoreError::WritesDisabled` を返す。`POST /v1/admin/gc` は 503、`uniqnode gc` は終了コード 1 で
   断る。読み出しは続ける。
3. 順を「ディスクが先、メモリが後」にそろえる:
   - `seal_active_pack` は、番号を足した `sealed_packs` の写しで MANIFEST を書き、成功してから
     メモリの `sealed_packs` と `active_pack_number` を進める。
   - `gc_commit` の C-3 も写しで MANIFEST を書き、成功してからメモリの `sealed_packs` を書き換える。
     C-2 の後に C-3 が失敗したら、新 pack は packs/ に在るが MANIFEST に無い形で残り、2 により
     書けない状態に入る(次の起動の recover が、MANIFEST に無い新 pack の扱いを今の規則で決める)。
     `active_pack_number` を新 pack の次へ進めるのも C-3 の成功の後にする。
   - GC の D(確定の後、ロックの外で古い pack と参照表を消す。gc.rs の 681〜699 行付近)も、この
     判定の外に置く。消すのに失敗しても、MANIFEST に無い古い pack は次の起動の recover が残骸として
     消すので、その GC の実行が誤りで終わるだけでよい(FEED 第 4 版の再確認の Claude N-3)。
   - GC の B(PackWriter の tmp/ への書き込み)は、この判定の外に置く。tmp の書き込みの失敗は
     どこからも参照されず、GC が誤りで終わって tmp を消すだけで済む(今の gc.rs の 572 行付近の
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
   - Io: ホストの再起動かファイルシステムの点検をしてから serve を起こす。serve の再起動だけでは
     page cache に残った「正常に見える」末尾の後ろに書くことになりうる。
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
  api 系のテストから使える。種類は次の 5 つ:
  - `before`: 1 バイトも書かずに誤り。
  - `torn:<n>`: n バイトだけ書いて誤り。
  - `sync`: 全部書いてから sync の位置で誤り(切り詰めは成功させる)。
  - `sync-keep`: 全部書いてから sync の位置で誤り、切り詰めも失敗させる(完全な 1 本がディスクに
    残る。方針 4 の「最大 1 レコード違いうる」を作る)。
  - `manifest`: 次の `write_manifest` の rename の前で誤り。
  - `manifest-dirsync`: rename の後のディレクトリの sync で誤り(MANIFEST は新しい中身になる)。
  - `dirsync`: 新しいセグメントを作った追記の、親ディレクトリの sync で誤り。
  - `nospace-sync`: sync の段で ENOSPC を返す(kind が Io になることを見る)。
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
- 開き直すと `writes_disabled` は null に戻る。

## 段取り

| 段 | 中身 | 大きさ |
|---|---|---|
| S1 | `append_durable` と包みへの寄せ、新しいセグメントのディレクトリの sync、`WriteFailure` と `WritesDisabled`、fstat と切り詰めの試み、封印と `gc_commit` の順の入れ替え、`atomic_write` の tmp 名、周期的な書き手の飛ばし、export の絞り、注入、HTTP と MCP の 503、status と健全性の欄、テスト、SPEC §5(永続化)への 1 段落 | M |

S1 はレビューで高の指摘が無いと確かめてから入る。FEED の F2 は S1 を前提にする。

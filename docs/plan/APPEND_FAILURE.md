# APPEND_FAILURE: 追記に失敗した後、応答済みの書き込みを失わない(設計草案)

<a id="d973833f-4e2b-4fc8-8a49-42f6821b6a7a"></a>

版: 第 1 版(2026-10-01)。Claude 側のレビュー待ち。その後に Codex の再レビュー待ち(crystal が止まっている間)。
出所は lamalium の健全性点検(2026-09-30)の項目 10。FEED 第 2 版の再レビューで、Codex(H1a・H1b)と
Claude(N1)が独立に見つけた。[docs/plan/FEED.md](#fa8de6f9-59f8-4512-a815-9f41d305db15) の第 3 版の
「失敗の境界」は、ストア全体の規則としてこの文書の内容を前提にしている。FEED より先に、単独で入れる。

## 欠陥(今のコードの事実)

`append_record`(node/src/store.rs の 208 行付近)は、ファイルを append で開いて
`[len][crc][payload]` を `write_all` し、`sync_data` する。どこかで誤りが出ても、何も戻さずに
`Err` を返す。呼び手は 4 つある: `put_object`(pack)、`append_own_record`(自分の ref・pin・
保持表明)、`ingest_ref_record`(複製の受け側)、それに GC の中の書き直し。

1. reflog: `write_all` が途中で失敗すると(ENOSPC が典型)、reflog の末尾に尻切れのレコードが
   残る。呼び手は `Err` を受けて 500 を返すが、ストアは次の要求で平気で追記を続ける。次の
   レコードは尻切れの後ろに書かれ、sync も通って 200 が返る。再起動すると、再生(`replay` の
   reflog の段、store.rs の 710〜730 行付近)は最初の壊れたレコードで切り、`set_len` で切り詰める。
   尻切れの後ろにあった、応答済みの書き込みが全部消える。
2. pack: 同じく尻切れが残る。加えて `active_pack_length` は進まないので、次の `put_object` は
   実際には尻切れの後ろ(ファイルの本当の末尾)に書かれるのに、索引には古い長さからの offset を
   記録する。同じプロセスの中でも、そのオブジェクトを読むと別のバイトが返る(ID の検算で
   Corruption になる)。再起動すると 1 と同じく尻切れの後ろが消える。
3. 追記以外の書き込み: `seal_active_pack`(store.rs の 932 行付近)は、`sealed_packs` に番号を
   足してから `write_manifest()?` を呼ぶ。MANIFEST の書き込み(`atomic_write` の tmp 書き・fsync・
   rename)が失敗すると、メモリは「封印済み」、ディスクは「未封印」のまま、ストアは次の pack へ
   書き続ける。GC の新 pack の rename・MANIFEST の書き換え・古い pack の削除も、失敗すれば
   メモリとディスクがずれうる(FEED 第 3 版の再レビューの Claude B)。reflog のセグメントの封印は
   今のコードには無い(`sealed_reflogs` に足す道が無い)。
4. `sync_data` の失敗: 書いたバイトはディスクに残るかもしれず、残らないかもしれない。Linux は
   fsync に失敗した dirty page を clean と印して捨てることがあり、次の fsync は成功を返しうる
   (いわゆる fsyncgate)。したがって、同じファイルへの後の sync の成功は、前の書き込みの永続を
   保証しない。

## 方針

ストアの持続的な書き込み(追記・MANIFEST・GC のファイル操作)の I/O の誤りが 1 度でも出たら、
そのプロセスではもう書かない。応答済みの書き込みを守る唯一の
確実な方法は、「壊れているかもしれない末尾の後ろに書かない」ことである。

1. `append_record` は書く前のファイル長を、開いたハンドルの fstat(`file.metadata().len()`)で
   読む(メモリの数え値は使わない。reflog には長さの数え値が無い。Claude C)。`write_all` か
   `sync_data` が誤りを返したら、`set_len(前の長さ)` と `sync_all` を 1 度だけ試み、結果に
   関わらず元の誤りを返す。切り詰めは再起動の再生を軽くするための best-effort であり、正しさは
   次の 2 が持つ。
2. ストアに `write_failure: Option<String>` を持たせる。次のどれかが誤りを返したら、そこに理由と
   時刻を入れる:
   - `append_record` の書き込みか sync(開くこと自体の失敗は、1 バイトも書いていないので除く。
     EMFILE のような一時的な誤りで書けなくならないように)、
   - `write_manifest`(`atomic_write`)、
   - GC の新 pack の書き込み・rename・古い pack と表の削除。
   以後、ストアの全ての書き込みの入口(`put_object`・`append_own_record`・`ingest_ref_record`・
   pack の封印・`write_manifest`・GC の開始)は、先頭で `write_failure` を見て、
   `StoreError::WritesDisabled(理由)` を返す。`POST /v1/admin/gc` と `uniqnode gc` も 503 か終了
   コード 1 で断る。読み出しは続ける。
2a. `seal_active_pack` は順を入れ替える: 足した後の `sealed_packs` の写しで MANIFEST を書き、
   成功してから、メモリの `sealed_packs` と `active_pack_number` を進める。失敗すればメモリは
   前のまま(未封印)で、2 により書けない状態に入る。GC の MANIFEST の書き換えも同じく、ディスクが
   先、メモリが後の順にする(今の `gc_commit` の順を確かめ、違えば直す)。
3. メモリの表は、今と同じく sync まで成功した追記の分しか進めない(`apply_verified` と索引への
   挿入は `append_record` の成功の後)。したがって書けない状態のメモリの表は「最後に成功した
   追記まで」であり、ディスクの再生の結果とは最大 1 レコード違いうる(sync が失敗し、切り詰めも
   失敗したが、バイトはディスクに届いていた場合)。その 1 本は、5xx を返した要求の分であり、
   応答済みのものではない。送り手から見れば「結果不明」で、再送で決着する(内容アドレスと ref の
   no-op により、同じ中身の再送は重複を作らない)。
4. 書けない状態から戻るのは、ストアを開き直したときだけである(serve なら再起動)。空き容量を
   足しても自動では戻らない。自動で戻すには「前の失敗の後の末尾が健全である」ことを確かめる
   必要があり、3 の fsyncgate のためにプロセスの中ではそれが言えない。
5. serve は終了しない。検索と読み出しは答え続け、書き込みだけが断られる。終了して systemd の
   Restart=on-failure に開き直させる案は採らない: 空きが無いままなら再起動の輪になり、読み出しも
   止まり、Mutex の poison の懸念も増える(Claude N2・N3)。

## 外から見える形

- `StoreError::WritesDisabled(String)` を足す。HTTP では 503、本文は
  `{"error":"writes disabled: <理由>; restart serve after fixing the cause"}` の形。主の口・
  読み口の書く口(`--agent-writable`、グラフの書く口)・MCP の add_document と fetch_url が同じ
  変換を通る。
- `/v1/status` に `writes_disabled`(null か `{"reason":…,"since":<unix 秒>}`)を載せる。健全性の
  集計にも 1 項目足す(遷移で 1 度だけ記録。should/0129)。
- serve の記録に、入ったときに 1 行残す(`uniqnode: store: writes disabled: …`)。
- CLI(`uniqnode ingest` などストアを直接開くもの)は、誤りを言って終了コード 1 で終える。今の
  挙動(`Err` で終わる)と同じで、新しいのは「同じ呼び出しの中で後の追記をしない」ことだけ。

## 関連して直すもの

- 書き込みの口を 1 つに寄せる: 4 つの呼び手が `append_record` を直接呼ぶのをやめ、`Store` の
  メソッド `append_durable(&mut self, path, payload)` を通す。この口が 2 の判定・記録と、テスト用の
  失敗の注入を持つ。
- 失敗の注入: 外部の crate を足さない。`Store` に `#[doc(hidden)] pub fn inject_append_fault(&mut self,
  AppendFault)` を置く。`AppendFault` は `BeforeWrite`(書く前に誤り)・`Torn(n)`(n バイトだけ
  書いて誤り)・`AfterWriteSync`(全部書いてから sync の位置で誤り)の 3 つで、次の 1 回の
  `append_durable` にだけ効く。本番の道では Option が None のまま 1 回比べるだけである。
- GC の書き直し(新 pack への追記)も同じ口を通す。GC の rename・削除・MANIFEST は、同じ
  `write_failure` を立てる小さな包み(`self.durable(|| …)`)を通す。GC の途中で書けなくなったら、GC は誤りで
  終わり、MANIFEST は書かない(今の回復の道がそのまま効く: MANIFEST に無い新 pack は次の起動で
  捨てられる)。

## やらないこと

- 追記のやり直し(再試行)。やり直しても、前の失敗の後の末尾の健全さは言えない。
- 1 要求の中の複数の書き込みの原子化(文書 1 本の取り込みは、チャンクのオブジェクトを何本も
  置いてから ref を 1 本書く)。途中で書けなくなれば、置いたオブジェクトは孤児として残り、
  次の gc が回収する。ref が 1 本も書かれていなければ見えは変わらない。複数の ref にまたがる
  原子性が要るのは FEED だけで、それは FEED.md の pending の規則が持つ。
- 導出データ(ベクトルの控え embed.rs、記録 log.rs)の追記。これらは壊れても作り直せる。

## 完了条件(テストで固定する)

- 3 通りの注入(BeforeWrite・Torn・AfterWriteSync)を、reflog の追記と pack の追記のそれぞれに
  かける。加えて、pack の封印の MANIFEST の書き込みの失敗を注入し、メモリの `sealed_packs` が
  進まず、書けない状態に入り、開き直した後のストアが一貫していることを見る。書けない状態では
  `POST /v1/admin/gc` が 503 になる。どれでも、失敗した要求は 5xx、その後の書き込みは全部 503 で断られ、読み出し(文書・
  検索)は答え続ける。
- 注入の前に 200 を返した書き込みは、ストアを開き直した後に 1 つも消えていない。
- Torn の後に開き直すと、再生は尻切れを切り詰め、失敗した要求の分は見えない。AfterWriteSync で
  切り詰めも成功した場合も同じ。
- pack の Torn の後、同じプロセスの中で既存のオブジェクトの読み出しが正しい中身を返す
  (offset のずれた索引項目が生まれない)。
- `/v1/status` の `writes_disabled` が理由を持ち、開き直すと null に戻る。

## 段取り

| 段 | 中身 | 大きさ |
|---|---|---|
| S1 | `append_durable` への寄せ、MANIFEST と GC のファイル操作の包み、`seal_active_pack` の順の入れ替え、`write_failure` と `WritesDisabled`、切り詰めの試み、注入、HTTP と MCP の 503、status の欄、テスト、SPEC §5(永続化)への 1 段落 | M |

S1 はレビューで高の指摘が無いと確かめてから入る。FEED の F2 は S1 を前提にする。

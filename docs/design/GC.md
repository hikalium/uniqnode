# GC — 生きているオブジェクトの定義と、pack の書き直しによる物理回収

<a id="9b1ceac3-f3cf-4595-87cb-6e40ce0900e5"></a>

ストアは追記専用で、封印された pack は二度と変わらない。ref を上書きしても tombstone に
しても、指していたオブジェクトのバイト列は pack に残る。この文書は、そのうち何を「生きて
いる」と見なし、何を「孤児」と見なすかの定義と、孤児を含む封印済み pack を生きているものだけ
の新しい pack で置き換えてディスクを取り戻す回収(`uniqnode gc <dir>`、serve の
`POST /v1/admin/gc`)の手順・回復・出力の読み方を書く。消す判断は含まない: 既に誰からも指されて
いないものを回収するだけである。消す判断(機会層の evict)と `renditions/` の輸出除外は未実装
で、[docs/plan/PACK_GC.md](#f272eeda-8664-42da-9e5c-ef354bc3f3a7) にある。実装は
node/src/gc.rs(手順)と node/src/store.rs(ロックの中で索引・MANIFEST・アクティブ pack に
触れる相と、回復)。

## 生きているオブジェクトの定義

到達性で決める。根は次の和集合である。

- 全署名者の ref(自分のも他ノードのも)のうち target が null でないものの target
- 全 pin の root(min_replicas=0 で解除された pin は残っていないので、pin 表の鍵がそのまま)
- held=true の保持表明が 1 つでも残っている root(held=false しか残っていない root は
  含めない)

根から c1 の参照規約(SPEC §4.3: 値の中の `s256:` + 16 進 64 桁の文字列)で辿れるものが
生きている。それ以外は孤児である。ローカルに無い参照先は無視する(開世界)。

判定の家は 2 つで、どちらも 1 箇所である(should/0135)。オブジェクトのバイト列から参照を
拾うのは `store::references_in`(閉包の探索・逆引き索引・gc の参照表がみな呼ぶ)。根から辿る
探索と dangling の扱いは `store::closure_over` で、参照の出どころ(オブジェクトを読んで解釈
するか、解析済みの表を引くか)だけを呼び手が差し替える。API の閉包の答え
(`Store::reachable_closure`)も gc の生きている集合も、この 1 つの探索である。根の側は
`Store::list_refs`・`effective_pins`・`held_attested_roots` から組む(`gc::roots`)。

他ノードの ref が指す先を根に含めるのは、自分がその複製を機会層として持っている意味が
そこにあるからで、外せるのは evict(PACK_GC.md の段階 2)だけである。

孤児が生まれる経路は 3 つ: 同じ ref パスへの上書き(再取り込み)、tombstone、put_object の
後に set_ref に至らなかった残骸。どれも「昔は生きていた」か「一度も名を持たなかった」もの
なので、回収してよいことに疑いはない。逆に、同じ内容の再取り込みは孤児を作らない
(content-addressed なので同じバイト列は同じ ID に落ち、put_object はべき等)。

## 単位と対象

回収の単位は封印済み pack 1 本である。追記中の pack には触れない(書き手と競合する)。pack
ごとに孤児率(孤児のバイト数 / pack のバイト数)を出し、閾値(既定 0.25、
`gc::DEFAULT_THRESHOLD`)を厳密に超えた封印済み pack を対象と見る。境界は含めない(ちょうど
閾値は keep)ので、閾値 0 は「孤児が 1 バイトでもあれば対象」、閾値 1 はどの pack も対象に
しない。判定は `gc::exceeds_threshold` の 1 箇所にある。1 回の実行で対象の全部を 1 本の新しい
pack に写す。既定の閾値に実測の根拠はまだ無い(PACK_GC.md のセルフレビュー 1)。

## 起動の口

`uniqnode gc <dir> [--dry-run] [--threshold <割合>]` は在るストアだけを開く(空のディレクトリ
や存在しない道は初期化せず 1 で断る。fsck と同じ道)。開くのでロックを取り、serve が開いて
いるストアには「別プロセスが開いている」と言って 1 で終わる。走っている serve のストアには
`POST /v1/admin/gc` を打つ(下)。閾値は 0 以上 1 以下で、外れれば usage で落ちる。
`--dry-run` は数えるだけで、ストアのデータ(MANIFEST・pack・reflog・tmp)には何も書かず、終了
コードは 0。導出データの参照表(下)だけは作る。

どちらの口も `gc::run(&Mutex<Store>, GcOptions) -> GcReport` を呼ぶ。dry-run も同じ関数の
同じ道を通り、B・C・D を省くだけである(should/0135)。同じストアで回収が走っている間に
もう 1 つ頼まれると `gc::GC_ALREADY_RUNNING` の Invalid で断る(admin は 409)。

## 手順

ロックとは serve の中の `Mutex<Store>` である(CLI の `gc` は自分のプロセスで同じ Mutex を
持つ)。生きている集合の計算で時間がかかるのは c1 の構文解析(実測で 15.2 秒のうち 14.6 秒。
[docs/analysis/20260905-gc-dry-run.md](#491c8c79-620b-47d4-b914-65279e6a599e))なので、
解析はロックの外で済ませ、ロックの中では解析済みの参照表と根の表から閉包を辿るだけにする。
封印済み pack は不変なので、その参照表はロックなしで読んで作れる。追記中の pack だけは
変わり続けるので、先に封印してしまう。

```
S. ロックの中で:  アクティブ pack を封印する(番号 s。空なら封印しない)。MANIFEST を書く。
                  以後の書き込みは新しいアクティブ s+1 に入る(小さい)。追記の現在位置
                  (pack 番号とオフセット)をカーソルとして写し取る。dry-run では封印せず、
                  位置だけ写し取る。
P. ロックの外で:  封印済み pack(s を含む)ごとの参照表(オブジェクト ID → 参照先 ID の列)
                  を derived/refs/pack-NNNNNN から流用するか、pack を読んで作って置く。
                  時間がかかるのはここ。dry-run では追記中の pack も今ある分だけ読む
                  (表には置かない。書き込み途中の末尾は捨て、A が読み直す)。
A. ロックの中で:  根の表と索引を写し取り、カーソル以後に追記されたレコード(封印が挟まって
                  いれば複数の pack)だけを解析して参照表に足し、閉包を辿って生きている集合
                  L を得る。pack ごとの集計と対象の判定。対象があれば、以後 put_object が
                  「既に在る」と答えた ID の記録を始める。
B. ロックの外で:  対象 pack の各オブジェクトを索引の位置で読み、ハッシュを照合しながら、
                  ID ∈ L のものだけを tmp/gc-<pid>.pack へ追記する。fsync。
C. ロックの中で:  差分検査 → 差し替え(下)。
D. ロックの外で:  対象だった旧 pack ファイルと、その derived/refs/ の表を削除する。新 pack の
                  参照表を(写したオブジェクトの参照は手元にあるので pack を読み直さず)置く。
```

### 差分検査(C の前半)

A と C の間に書き手が動いているので、A で捨てると決めた集合 L̄ の要素が生き返っていないかを
見る。L̄ の要素が生き返る道は 2 つあり、どちらも漏れなく拾える。

- 新しい根から到達する。set_ref は target の存在を要求するので、孤児を指す新しい ref は
  作れる(生き返りは実際に起こる)。それは必ず新しい reflog レコードとして現れ、根の表(全署名
  者の ref・pin・attest から組む)はそれを全部適用した後の姿なので、A の根の表と C の根の表の
  差分が「A 以後に増えた根」である。増えた根から、A 以後に追記されたオブジェクト(カーソル以後を
  解析して参照表に足す)も含めて辿り、L̄ に入っているものを L に移す。
- put_object で「既に在る」と答えられ、ref を張る途中である。SPEC §5.3 の順序(オブジェクト
  → ref)を API 越しに 2 つの要求で踏む呼び手は、POST /v1/objects が孤児と同じバイト列で
  べき等に成功した直後に PUT /v1/refs を出す。C がその間に入って孤児を消すと、PUT が「target
  が存在しない」で落ちる。A 以後に put_object が既存と答えた ID(`Store::gc_begin_touch_log`
  / `gc_take_touch_log`)も L に移す。

移したものは B の成果物(まだ tmp/ にある新 pack)に写し足して fsync する。中止はしない。

### 差し替え(C の後半。`Store::gc_commit`)

順序は、回復の「封印済みでない最後の 1 本がアクティブ」という仮定を守るためにある。新しい
封印済み pack の番号がアクティブより大きくなると、アクティブが「最後」でなくなり、torn tail を
持っていれば破損と誤判定され、MANIFEST に無いので残骸として消される。

1. アクティブ pack を封印し MANIFEST を書く(番号 a)。S と C の間に書き込みが無ければ a は
   空なので、封印せず、その番号を新 pack に譲る(空の封印済み pack を作らない)。
2. 新 pack を a+1(a が空なら a)として packs/ へ rename し、ディレクトリを fsync する。
   写すものが 1 件も無ければ新 pack は作らない。
3. MANIFEST を書く: sealed_packs から対象を外し、新 pack を足す。
4. 索引を新しい位置に差し替え、対象に残った孤児を索引と used_bytes から外す。
   object_generation を進める。
5. アクティブを新 pack の次の番号で開く。

MANIFEST が参照するファイルは参照される前に fsync 済み(SPEC §5.2 MUST)を、新 pack の
rename と MANIFEST の rename の順で守る。MANIFEST は 1 と 3 で 2 回書く。1 を 3 に畳んで
1 回にすると、2 の後・3 の前で落ちたとき、追記した a が「MANIFEST に無く最後でもない pack」に
見えて残骸として消される(node/tests/gc_crash.rs がこの順序を固定する)。

## 参照表(derived/refs/)

`derived/refs/pack-NNNNNN` はテキストで、1 行目が `uniqnode-refs-1 <pack ファイルの大きさ>`、
以下 1 行に `<オブジェクト ID> <参照先 ID>...`(参照を持つオブジェクトだけ。参照の無い
オブジェクトと c1 でない blob は行を持たない)。流用の条件は、頭の形が合い、記録された大きさが
今の pack ファイルの大きさと一致し、末尾が改行で終わり、全部の語が ID の形であること。1 つでも
外れれば作り直す。導出データなので backup は写さず、消せば次の gc が作り直す。

## 回復の変更

MANIFEST が在るとき、MANIFEST に無く最後でもない pack は、回収が MANIFEST を書き換えた後・旧
pack を消す前(D の前)に落ちた残骸である。封印済みの一覧は MANIFEST に完全に書かれているので、
それ以外の道でこの形は生じない。回復は走査せず、ログに名を出して削除する。「最後」の判定には
番号も見る: 追記中の pack は封印済みのどれよりも大きい番号を持つので、最後であっても封印済みの
最大より小さい番号なら残骸である(回収が新 pack を作らずに旧 pack を MANIFEST から外した直後に
落ちた形)。

MANIFEST に無い最後の pack で、レコードが 1 件以上あり、その全部が前の pack の重複であるもの
は、回収が新 pack を packs/ に据えた後・MANIFEST を書く前(2 の後、3 の前)に落ちた形である
(put_object はべき等で、通常の追記は重複を作らない)。中身は全部前の pack に在るので、消しても
失うものは無い。残すと、以後の追記がその後ろに続いて、重複の分が二度と回収されなくなる。
これもログに名を出して削除する。

どちらの規則も MANIFEST が在るときだけ使う。MANIFEST が無い(初回封印前か、失った)ストアでは
全部が未封印に見えるので、従来どおり削除せず走査する。fsck はこのとき「MANIFEST に無く最後で
もない pack」を `unlisted_packs` として報告する(エラーではなく事実。封印されていたのか追記中
だったのかは、もう分からない)。MANIFEST が在れば開くときに消しているので、この報告は空である。

tmp/ は開くときに空にする。書きかけの新 pack(`tmp/gc-*.pack`)を含め、tmp/ に残るのは据えられ
なかったものだけである。

## クラッシュの各点

| 落ちる点 | 状態 | 回復 |
|---|---|---|
| S の後 | アクティブが封印され MANIFEST に載っている | 次の番号がアクティブになる。孤児は次回の対象 |
| B の途中 | tmp/ に書きかけの新 pack | 開くときに tmp/ が空になる。旧 pack は無傷 |
| C-1 の後 | (書き込みがあれば)アクティブが封印済み。新 pack は tmp/ | 同上。回収は次回やり直し |
| C-2 の後、C-3 の前 | packs/ に新 pack があるが MANIFEST に無い。最後で、中身は旧 pack と重複 | 重複だけの pack として削除される。旧 pack は無傷 |
| C-3 の後、D の前 | MANIFEST は新。旧 pack と derived/refs/ の表が残る | 旧 pack は「回収済みの残骸」として削除される。表は次の gc が(pack が無いので)読まない |

node/tests/gc_crash.rs が、テスト用の口(下)で各点に落として、開き直しの fsck が緑で生きている
ものが全件読め、孤児は消えているか次回の対象として残っているか、を固定する。

## テスト用の口

cfg(debug_assertions) のビルドだけが読む環境変数が 2 つある。release ビルドには入らない
(`gc::crash_point`・`gc::wait_after_copy`)。

- `UNIQNODE_GC_CRASH_AFTER=<S|B|C1|C2|C3>`: その相の直後(B は最初の 1 件を写した直後)に
  `std::process::abort()` する。
- `UNIQNODE_GC_WAIT_AFTER_B=<道>`: B の後に `<道>.ready` を作り、`<道>` が現れるまで待つ
  (30 秒で諦めて失敗する)。A と C の間に別の書き手を挟むテストが時機を作る。

## `POST /v1/admin/gc`

走っている serve のストアに回収を 1 回走らせる。ボディは省略可で、
`{"threshold": "0.25", "dry_run": false}`。c1 は小数を持たないので、割合は文字列(または 0 か
1 の整数)。範囲外・型違いは 400。既に走っていれば 409。ロックを持つのは S・A・C の間だけで、
その他の相の間は他の要求が答える(node/tests/gc.rs が B の後で止めて GET が答えることを見る)。
応答は報告をそのまま JSON にしたもので、`packs`(pack ごとの number・sealed・objects・bytes・
live_objects・live_bytes・garbage_bytes・compact)、`roots`・`objects`・`live_objects`・
`garbage_bytes`・`compact_bytes`、`sealed_in_seal_phase`・`tables_built`・`tables_reused`、
`compacted`(書き直した pack の番号)・`sealed_active`・`new_pack`・`revived_objects`・
`reclaimed_bytes`・`disk_bytes_freed`、`phases_ms`(S・P・A・B・C・D と locked = S+A+C)。
同じ内容を 1 行にしてログにも残す。serve への組み込み(周期または容量契機)はまだ無く、手で
打つ。

## 出力

pack ごとに 1 行、集計 1 行、何をしたかの 1 行、各相の所要 1 行。

```
pack 000001 sealed: objects 51668 bytes 269811924, live 269811526, garbage 398 (0.0%) -> compact
pack 000002 sealed: objects 3765 bytes 107021096, live 107021096, garbage 0 (0.0%) -> keep
pack 000003 active: objects 0 bytes 0, live 0, garbage 0 (0.0%) -> keep
gc: packs 3 (sealed 2, compact 1), objects 55433 live 55432 garbage 1, garbage bytes 398 (compact would reclaim 398), roots 202
gc: compacted packs 000001 -> new pack 000003, reclaimed 398 bytes (disk 413720 bytes), revived 0 objects, sealed in S: 000002, sealed in C: none (threshold 0)
gc: phases S 3 ms, P 14851 ms (tables built 2 reused 0), A 42 ms, B 2210 ms, C 51 ms, D 8 ms; locked (S+A+C) 96 ms
```

- `sealed` / `active`: MANIFEST が封印済みと言う pack か、追記中の pack か(A の時点。S で
  封印した pack は sealed に入る)。封印済みの全部と追記中の 1 本が、オブジェクトが無くても
  行になる。
- `objects`・`bytes`: その pack に索引が置いているオブジェクトの数とペイロードの合計。
  `used_bytes` と同じ会計で、レコードの 8 バイトの頭と、クラッシュ再送で重複追記された
  2 つ目以降は入らない。だからファイルの大きさより少し小さい。
- `live` / `garbage`: 上の定義で生きているバイト数と、そうでないバイト数。括弧は孤児率。
- `-> compact` / `-> keep`: 対象か。追記中の pack は孤児率がいくらでも keep。
- 集計行: pack の数(封印済み・対象)、オブジェクトの数(全体・生きている・孤児)、孤児の
  バイト数(追記中の pack の分も含む)と、対象の pack だけを書き直したときに戻るバイト数、
  根の数。
- 3 行目: 書き直した pack と新 pack の番号、used_bytes から戻ったバイト数と packs/ のファイル
  で見た減り、差分検査で生き返らせた数、S と C-1 で封印した pack。対象が無ければ
  `gc: nothing to compact (...)`。dry-run なら
  `gc: live set computed in <A の ms> ms (threshold ..., dry-run: nothing written)`。
- 4 行目: 各相の所要(ms)と、P で作った・流用した参照表の数、ロックを持っていた長さの合計
  (S+A+C)。実コーパスでの実測は
  [docs/analysis/20260905-gc-dry-run.md](#491c8c79-620b-47d4-b914-65279e6a599e)。

## 周辺への影響

- 逆引き索引(`ReferrerIndex`)は `Store::object_generation`(put で増え、回収で進む番号)で
  最新かを見る。件数で見ると、回収で k 件減った後に k 件増えた集合を同じと誤認する。
- 検索索引と埋め込みの索引の世代は collections/ 配下の ref の束縛で決まり、回収は束縛を
  変えないので影響しない(消えるのは束縛から辿れないものだけ)。
- 埋め込みキャッシュ(`derived/embeddings/`)には消えたオブジェクトのベクトルが残る。害は
  容量だけで、詰め直しは別の項目(PACK_GC.md のセルフレビュー 9)。
- backup は封印済み pack を MANIFEST で選んで写すので、回収の途中(D の前)の残骸を写さない。
  回収で写し元の MANIFEST から消えた pack が写し先に残るときの扱いは
  [docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2)。
- 分散検索・同期: 孤児は定義により誰の ref からも指されていないので、ピアから GET されうる
  のは「相手が古い ref を持っている」場合だけで、404 は開世界の正常な答えである。

## テスト

- 単体(node/src/gc.rs): 根の定義(上書き・tombstone・残骸・他ノードの ref・pin・attest・
  撤回・閉包)、閾値の境界、pack ごとの判定、回収そのもの(対象が消え生きているものが読め
  used_bytes が減り再び開いても同じ)、参照表の流用と作り直し。
- 統合(node/tests/gc.rs): 本番の入口で、dry-run が何も書かないこと、回収の前後の観測、
  2 回目が参照表を流用すること(mtime)、admin の dry_run と 409、B の後で止めている間の
  GET と、孤児を指す新しい ref・put し直しの生き返り、再起動後も読めること。
- クラッシュ(node/tests/gc_crash.rs): 各点で abort して回復。S と C の間に追記があるときの
  C-2 の後(C-1 の順序を固定)。MANIFEST を失ったストアで残骸の規則を使わないこと。

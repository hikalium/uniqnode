# FEED: lamalium の文書を crystal から専用の書き口で受ける(vega 側の設計)

<a id="fa8de6f9-59f8-4512-a815-9f41d305db15"></a>

2026-09-30 の操作者の裁定(案 C)を受けた、vega 側の設計である。crystal の timer が lamalium の
文書を HTTP で vega へ送り、vega はそれを専用の書き口で受けてコレクション lamalium に入れる。
送り手(crystal 側)の設計は lamalium 側が書き、双方の設計が別モデルのレビューを通ってから
実装に入る。経緯と他の案は [docs/plan/LAMALIUM.md](#68571059-94ed-4aa2-8ae0-b2862d1de44e) の
「L4 で要るもの」。

版: 第 2 版(2026-09-30)。第 1 版(004c305)への Codex のレビュー(H1・H2・M1〜M3)と Claude の
レビュー(1〜14)を取り込んだ。各節の末尾の括弧に、どの指摘への答えかを書く。

## 裁定の条件(lamalium 側から受けたもの)

- 新しい口(例 7446)は読み口と別で、書けるのは lamalium コレクションだけ、許す送信元は
  crystal 10.10.128.2 だけ(orion は許さない)、server-proxy は転送しない。
- `DELETE /v1/collections/{c}/documents/{name}` を足す。
- 1 回の送りは 1 つのコミットに固定し、文書の一覧(manifest)と、各文書の元のコミットを残す。
- 送りが失敗したら、前に公開していた中身はそのまま残る(途中まで送った状態で、コレクションに
  2 つのコミットが混ざらない)。
- 消去と改名を扱う。消去の規律は次の 3 つ:
  1. その回の PUT が 1 つでも失敗したら、古い名前を 1 つも消さない。
  2. 「前回送った名前の一覧」を更新するのは、その回の PUT と DELETE が全部成功した後だけ。
  3. 落ちた後の次の回は、同じコミットからやり直して同じ結果になる(べき等)。
- crystal 側の外向きの nft(`meta skuid`)とコンテナからの転送の遮断は crystal 側が用意する。

## 今のストアで何が原子的か(設計の前提)

- ref の書き込みは 1 本ずつである。`Store::set_ref` が reflog に 1 レコードを書いて
  `sync_data` し、成功すればすぐメモリの表に反映する(store.rs の `append_own_record`)。落ちた
  ときの単位は 1 レコードで、複数の ref をまとめて書くレコードも、前の値を条件にする書き込み
  (compare-and-swap)も無い。
- 検索の見え方は、ストアのロックを取った瞬間の ref の表で決まる(索引の世代は `collections/`
  の ref の束縛)。1 つの serve の中では、1 回のロックの中で書いた複数の ref の途中の状態は、
  検索にも索引の温めにも見えない。
- set_ref は target のオブジェクト 1 つが在ることしか見ない。gc は封印済みの pack を単位に
  回収するので、doc_rev は残ってチャンクだけが消える、ということが起こりうる。

したがって「ステージング用のコレクションに入れて差し替える」形は採らない。差し替えそのものが
ref を 1 本ずつ書く作業になり、途中で落ちれば混ざる。その上、ステージングのコレクションは
検索に見えてしまう(`collections/` の下は全部索引に入る)。

採るのは「オブジェクトだけ先に置き、ref は公開の 1 回で、1 回のロックの中でまとめて張る。張る
予定の表(manifest)を先に 1 本の ref で保存し、張る途中で止まったら最後まで張り切る(前へ
進める)」形である。保証の言い方は正確にこうなる: ストアを開くどの入口(serve・mcp・CLI)も、
開いた直後に途中の公開を前へ進め終えてから読み書きを始めるので、どの読み手も「前の公開」か
「この公開」のどちらかしか見ない。 途中の状態がディスクに在るのは、プロセスが止まってから
次に誰かがストアを開くまでの間だけで、その間は誰も読んでいない(Claude 3)。

## 口

serve に、読み口とは別の待ち受けを足す。

```
uniqnode serve … --listen-feed 10.10.128.1:7446 --feed-collection lamalium
```

- `--feed-collection` は 1 つだけ受ける(書けるコレクションは 1 つ)。門は、path の `{c}` が
  これと字句で一致することを最初に見る(Claude 4)。
- 通すのは下の「送り手が守る API」の 3 本と `/healthz`・`/v1/status` だけで、それ以外は 403。
  検索も文書の読み出しも通さない(送り手は読まない。読むのは読み口の仕事)。
- 門の判定は agent_door.rs と同じ形の関数(許可表を引数に取る)を別に置く。読み口の門と
  混ぜないのは、読み口の許可を広げる変更が書き口に漏れないようにするためである。
- 記録は読み口と同じ行の形で、印を `feed` にする。要求ヘッダ `X-Uniqnode-Task` と
  `X-Uniqnode-Agent` は読み口と同じく記録に写すだけで、判断には使わない。
- 本文の上限は書き口だけ 4 MiB(全体の上限は 64 MiB)。超えれば 413(Claude 7)。
- 読み口から lamalium を読ませるには、今と同じく `--agent-collections lamalium` を足す。
  書かせることは無い(下の「管理下のコレクション」)。

install と firewall:

- `--listen-feed <addr>`・`--feed-collection <c>`・`--feed-firewall-allow <addr,...>` を足し、
  drop-in の ExecStart に写す。本番の据え付けの 1 行は、今の本番の引数(`--serve-options`・
  `--listen-agent`・`--agent-writable`・`--agent-collections` の全部・`--after`・
  `--firewall-allow`)をそのまま残し、そこに足す形で書く(Claude 14)。
- nft は同じ実体の表 `inet uniqnode_<インスタンス>` の同じ chain に、口ごとに 1 規則を足す。
  `iifname != "wg1" ip daddr 10.10.128.1 tcp dport 7446 counter drop` と
  `ip daddr 10.10.128.1 tcp dport 7446 ip saddr != { 10.10.128.2 } counter drop` の 2 本。
  許すのは wg1 から来た crystal だけで、読み口と違って vega 自身(10.10.128.1)も許さない
  (裁定は「crystal だけ」。確認のためだけに vega の全プロセスを許可に入れない。Codex M3・
  Claude 12)。vega の wg1 の設定で crystal の peer の AllowedIPs が 10.10.128.2/32 である
  ことを F4 で確かめる(送信元の偽りを WireGuard の側で防ぐため)。
- したがって install は書き口に網越しで触れない。install の確認は (1) serve が書き口を
  束縛したという記録の行、(2) `nft list table` にその 2 規則が載っていること、の 2 つに限る。
  口が答えることの確認は F4 で crystal から行う(`GET /v1/feeds/lamalium` が 200、許していない
  コレクション名への PUT が 403、検索が 403)。
- 本番の据え付けは sudo が要るので、操作者に 1 つのコードブロック(実行するホストを明記し、
  `2>&1 | ts … | tee /tmp/uniqnode-install-feed.log` 付き)で渡す。

## 名前の文法(Claude 4・5)

- `{c}`: 今のコレクション名の規則のまま。書き口では `--feed-collection` と一致すること。
- `{commit}`: 16 進の小文字で、ちょうど 40 字(SHA-1)か 64 字(SHA-256)。
- `{path}`: 取り込み起点からの相対パスで、拡張子まで含めたものを文書名にする
  (`docs/design/X.md` の文書名は `docs/design/X.md`)。今の ingest は拡張子を落とすが、feed では
  落とさない。`a.md` と `a.txt` が同じ名前に潰れて後の方が黙って勝つことが無くなり、
  `git show <commit>:<文書名>` がそのまま引ける。
  - `/` で区切った各段は `[A-Za-z0-9._-]` の 1〜128 バイトで、空の段・`.`・`..` は断る。
  - 先頭の `/` は断る。全体は 512 バイト以下、深さは 16 段以下。
  - 拡張子は `md`・`markdown`・`txt` だけ(pdftotext と HTML の解析器に feed の入力を渡さない)。
  - パーセント符号は解かない。文法に合わない字はそのまま 400 になる。
- 同じ文法を ingest-git の側にも使い回せるよう、検査は 1 つの関数に置く。

## 管理下のコレクション(書き手は feed の transaction だけ。Codex H2・Claude 9)

管理下かどうかはストアの中身で決める: 自分の署名の ref `feeds/<c>/published` か
`feeds/<c>/pending` が在れば、コレクション c は feed の管理下である(serve の引数でなくストアで
決めるのは、serve を止めてストアを直接開く CLI にも同じ判定を効かせるため)。判定と拒否は
1 箇所(ストアの ref を書く関数の手前の層)に置き、次の入口のどれからも、管理下の
`collections/<c>/` と `feeds/` の ref は feed の transaction 以外では書けない(409「コレクション
… は feed が管理している」):

- 主の口の `PUT /v1/collections/{c}/documents/{name}` と `POST …/fetch`
- 主の口の汎用 `PUT /v1/refs/{path}`(`feeds/` の下は、管理下でなくても常に断る)
- 下の `DELETE /v1/collections/{c}/documents/{name}`
- MCP の `add_document`
- 読み口の `--agent-writable`(install が同じ名を断るのに加えて、serve も断る)
- ストアを直接開く CLI(`ingest`・`ingest-git`・`fetch`)

「強制」の抜け道は作らない。手で直したいとき(古い文書を消したい等)は feed の transaction を
通す: 主の口にだけ `POST /v1/feeds/{c}/edit {"base":<公開中の manifest>,"remove":[名…]}` を置き、
公開中の manifest からその名を除いた新しい manifest を、下の commit と同じ手順(pending →
張る → published)で公開する。新しい manifest の commit は元と同じで、`"edit":true` と除いた名を
持つ。こうすると published と実際の ref が食い違うことは無い。同じ run の再送は、documents が
違うので 409 になる(手で直したことが送り手に見える)。次のコミットの run は普通に通る。

最初の公開: base が null の commit は、コレクションに自分の署名の ref が 1 本も無いときだけ
通す(在れば 409 で、その名の一覧を返す)。既に在る中身を基準に取り込む道は作らない。
lamalium コレクションは今まだ作っていないので、この条件で困ることは無い。もし先に何かが
入っていたら、管理下になる前なので DELETE で消してから始める。

他の署名者の ref: 検索は今、どの署名者の `collections/` の ref も索引に入れる。管理下の
コレクションでは自分の署名の ref だけを索引に入れる(search.rs の visit_indexable_chunks に
1 つの条件)。vega の本番のストアは今ピアから同期していないが、同期を始めたときに、他の
ノードが同じ名前のコレクションへ書いたものが混ざらないようにする。

docs/mop/SYSTEMD.md の「git の木を定期に取り込む」(uniqnode-ingest-git@ の例)に、
feed の管理下のコレクションには使えない(409 で断られる)ことを書き足す。

## 送り手が守る API

1 回の送りを run と呼び、run はコミットの ID で名指す。

### 1. `GET /v1/feeds/{c}` — 今公開している中身を読む

```
200 {"collection":"lamalium",
     "published":{"commit":"<commit>","manifest":"s256:…","at":"<時刻>","edit":false,
                  "documents":{"docs/design/X.md":"s256:<doc_rev>", …}} | null,
     "pending":null | {"commit":"…","manifest":"s256:…"},
     "shrink_allowed_for":null | "<commit>"}
```

送り手は run の最初にこれを読み、`published.manifest` を run の基準(base)として固定する。
以後の 2 と 3 は全部この base を送る。「前回送った名前の一覧」はこの `documents` の鍵であり、
送り手が自分で持つ必要は無い(正典は vega 側の manifest)。初回は published が null で、base は
`null` を送る。GET も、答える前にロックの中で前進を試みる(下の「失敗の境界」)。

### 2. `PUT /v1/feeds/{c}/runs/{commit}/documents/{path}?base=<manifest|null>` — 文書を置く(ref は張らない)

本文は生のバイト列(UTF-8 の本文。4 MiB 以下)。serve は今の PUT documents と同じチャンク分けを
し、blob・チャンク・doc_rev をオブジェクトとして置くが、ref は張らない。比べる相手と
doc_rev の previous は、今の ref ではなく base の manifest の同じ名前の doc_rev から取る(今の
ingest は現在の ref を previous に入れるので、別の run が先に進むと同じ入力でも ID が変わる。
Codex M1)。

- base の doc_rev と内容(元のバイト列とチャンクの並び)が同じで、かつその doc_rev の meta の
  feed が c なら、新しい doc_rev を作らず、base の doc_rev をそのまま返す(`"unchanged":true`)。
  中身の変わらない文書は run ごとに ref を書かない(reflog が毎時 200 本ずつ伸びるのを避ける。
  RAG.md の項目 16)。meta の feed が c でない doc_rev(feed 以外の道で入ったもの)は、内容が
  同じでも「変わった」として作り直す(Claude 6)。
- 内容が違えば、doc_rev を次の形で作る:
  `{v:1, kind:"doc_rev", source, chunks, meta:{name:<path>, media, feed:<c>, commit:<commit>},
  previous:"<base の doc_rev の 16 進 64 字(s256: を付けない)>"}`。
  - meta の `commit` は「今の内容を最初に送った run のコミット」である。ファイルを最後に変えた
    コミット(`git log -1 -- <path>`)とは限らない(差し戻し・送りの取りこぼしがあれば違う)。
    run のコミットそのものは manifest が持つ(Claude 6)。
  - previous を `s256:` の無い 16 進にするのは、gc がそれを参照として辿らないようにするため
    である(SPEC §4.3 の参照規約は `s256:` + 64 字だけ)。付けると、公開中の doc_rev から
    過去の全版が辿れて、消した文書(誤って入れた秘密を含む)を gc で回収できなくなる。
    履歴は reflog と manifest の鎖が持つ(Claude 8)。
- 保証の範囲: 同じ (c, commit, base, path, 本文) なら、いつ何度置いても同じ ID が返る。base が
  違えば(別の run が先に公開した後なら)ID は変わりうる。そのときは 3 が 409 を返すので、
  送り手は 1 からやり直す(Codex M1)。
- 応答: `200 {"name":"<path>","doc_rev":"s256:…","unchanged":bool,"new_objects":N}`。
- `base` が公開中の manifest でなければ 409。
- 空きの下限: ストアのあるファイルシステムの空きが 2 GiB を切っていれば、置かずに 507 で断る
  (孤児でディスクを埋めない。前進に要る空きも残す。Claude 7)。
- 失敗は 4xx/5xx と理由の本文。送り手は 1 つでも失敗したら、その run の 3 を打たずに終える
  (規律 1)。置いたオブジェクトは ref から辿れない孤児になり、gc が回収する。
- meta の `feed` と `commit` は予約の鍵にする(主の口の `?meta.feed=` などは 400)。

### 3. `POST /v1/feeds/{c}/runs/{commit}/commit` — まとめて公開する

```
{"base":"<1 で固定した manifest>" | null,
 "documents":{"docs/design/X.md":"s256:<2 で返った doc_rev>", …}}
```

serve はストアのロックを 1 回取り、その中で次を順に行う。

0. 前進: `feeds/<c>/pending` が在れば、先にそれを最後まで張る(下の「失敗の境界」)。
1. 再送の判定を先に行う(Codex M1・Claude 11): 公開中の manifest の commit がこの `commit` で、
   `documents` も同じなら、何もせず 200(落ちた後のやり直しで、公開が済んでいた場合。
   規律 3)。commit が同じで `documents` が違えば 409(手の edit が入った後など)。
2. `base` が公開中の manifest と違えば 409(別の run か edit が先に公開した。送り手は 1 から
   やり直す)。manifest の ID を CAS の札に使う。
3. 各 doc_rev が、この run とこの base のものであることを確かめる(Codex M2・Claude 1・12):
   - base の manifest の同じ名前の doc_rev そのもの(変わらない文書)か、
   - kind が doc_rev で、meta の name が鍵と一致し、meta の feed が c、meta の commit がこの
     commit、previous が base の同じ名前の doc_rev の 16 進(base に無い名前なら previous が
     無い)であるもの。
   どちらでもなければ 409。さらに、その doc_rev の source と chunks の各オブジェクトまで
   ストアにあることを確かめる(閉包の検査)。欠けていれば 409(2 の後に gc が孤児として回収
   した。送り手は 2 から置き直す)。
4. 量の上限: documents は 10,000 件以下、manifest の直列化は 4 MiB 以下。超えれば 413(Claude 7)。
5. 縮みの歯止め(Claude 10): documents が空、または「新しい件数 < 0.75 × 公開中の件数、かつ
   減る件数 > 10」なら、`shrink_allowed_for` がこの commit でない限り 422 で断る。見るのは
   消える名前の数ではなく正味の減りなので、ディレクトリの改名(60 件が消えて 60 件が増える)は
   通り、30 件を落とす不具合も「件数が減った」ことでは止まらない代わりに、manifest の差として
   応答に残る(下)。422 の本文は公開中の件数・新しい件数・消える名前の一覧を持つ。
   操作者の承認は主の口にだけ置く `POST /v1/feeds/{c}/allow-shrink {"commit":"<C>"}` で、
   ref `feeds/<c>/allow_shrink` に記録し、その commit の公開が済んだら消す。送り手が承認を
   出すことは無い(送り手の API に承認の欄は無い)。
6. manifest オブジェクト
   `{"v":1,"kind":"feed_manifest","collection":c,"commit":…,"edit":false,
   "previous":"<base の manifest の 16 進(s256: を付けない)>","documents":{…}}` を置き、
   ref `feeds/<c>/pending` を張る。ここが下の「失敗の境界」である。previous を `s256:` の
   無い形にするのは 2 の doc_rev と同じ理由で、公開中の manifest から過去の manifest とその
   doc_rev が gc に辿られないようにするため(Claude 8)。
7. 名前ごとに `collections/<c>/<name>` を張る(今の target と同じなら書かない)。base にあって
   documents に無い名前は tombstone する(消去。改名は新しい名の張りと古い名の tombstone
   として、ここで同時に起きる)。
8. `feeds/<c>/published` を manifest に張り、`feeds/<c>/pending` を tombstone する。ここで
   初めて「前回送った名前の一覧」が新しくなる(規律 2)。
9. ロックを放してから索引の温めに合図する。

応答: `200 {"commit":…,"manifest":…,"bound":N,"deleted":N,"unchanged":N,"deleted_names":[…]}`。

ロックの長さ: 7 は名前 1 つにつき 1 回の fsync で、変わった文書の数だけかかる。9 の後の索引の
作り直し(本番の規模で約 10 秒)もロックの中で走る。毎時 1 回の公開なら、その間の検索が待つ
のは許容する(Claude 13)。

### 失敗の境界(Codex H1・Claude 2・3)

store.set_ref は ref を 1 本書くたびに reflog へ書いて sync し、成功すればすぐメモリの表に
反映する。したがって 7 の途中で 1 本が I/O の誤りを返したとき、そのままロックを放すと、
検索は「一部が新しく一部が古い」表を見る。これを外に出さないために、境界を次のように引く。

- 6 で pending を張り終える前の失敗: 公開は何も変わっていない。4xx/5xx と理由を返す。
  送り手は「前の公開が残っている」と扱って良い。
- pending を張り終えた後の失敗(7・8 のどこか): 結果は「前へ進めることが確定」である。
  serve はロックを持ったまま前進(7・8)をもう 1 度だけやり直す。それも失敗したら、ロックを
  持ったまま終了コード 75 でプロセスを終える(応答は返らず、接続が切れる)。ロックを放さずに
  終えるので、途中の表を読む要求は 1 つも無い。reflog の末尾が書きかけなら、次に開いたときの
  再生が切り詰める(store.rs の再生)。serve の unit は Restart=on-failure で、止めるのは終了
  コード 1 と 2 だけ(RestartPreventExitStatus=1 2)なので、75 は systemd が起こし直す。
- 前進の関数: 「pending の manifest に従って 7・8 を最後までやる」を 1 つの関数に置き、
  ストアを開く共通の道(serve・mcp・CLI が通る `Store::open` の直後の 1 箇所)で、索引の温め
  (start_index_warmer)と listener より先に呼ぶ。加えて commit・edit・GET の先頭(ロックの中)
  でも呼ぶ。7 は「今の target と同じなら書かない」ので、何度呼んでも同じ結果になる。
- 前進そのものが失敗したとき: ストアを開く道は失敗を返し、serve は listener を開かずに
  終了コード 75 で終える(systemd が 2 秒ごとに起こし直し、journal に理由が残る)。CLI と mcp は
  理由を言って終了コード 1 で終える。前進が済むまで、どの入口も検索も読み出しも始めない。
- 状態の報告: `/v1/status` に feed ごとの `published_at`(最後の公開の時刻)と、
  `pending`(前進が済んでいない manifest があれば)を載せる。送り手と監視は published_at の
  古さで「公開が止まっている」ことを見る(Claude 2・10)。

送り手から見ると、3 の応答が 200 なら公開済み、4xx なら公開されていない、接続が切れた・5xx
なら「結果不明」である。結果不明のときは 1 からやり直す: 公開が済んでいれば 3 の 1 で 200 の
no-op、済んでいなければ普通の run になる(規律 3)。

`feeds/` の ref は検索の対象外(索引は `collections/` だけを見る)で、gc の根になる。公開中の
manifest と pending の manifest、そこから `s256:` で辿れる doc_rev とその閉包は回収されない。
過去の manifest と doc_rev は previous が参照でないので辿られず、公開から外れれば回収される。

### 送り手の 1 回の流れ(まとめ)

```
GET  /v1/feeds/lamalium                                     → published.manifest を base に固定
PUT  /v1/feeds/lamalium/runs/<C>/documents/<path>?base=<B>  × 全文書(1 つでも失敗したら終わる)
POST /v1/feeds/lamalium/runs/<C>/commit {base, documents}
       200 → 終わり
       409 → 最初からやり直す(送り手の側で回数に上限を置く)
       413・422 → 操作者に上げる(422 は最初の 1 回だけ知らせ、以後の run は同じ 422 を黙って受ける)
       接続が切れた・5xx → 結果不明。最初からやり直す(公開済みなら no-op で 200)
```

送り手は消すものを自分で計算しない(3 が manifest の差から出す)。送り手が守るのは「その
コミットの対象の全文書を送り、全部の PUT が 200 だったときだけ commit を打つ」ことと、
「同じ文書名に潰れる 2 つのパスがあれば送らない」(文書名は拡張子まで含むので、ふつうは
起きない)ことと、「公開中より古いコミットを送らない」(`git merge-base --is-ancestor
<公開中の commit> <C>` で確かめる。vega 側は祖先関係を知らないので、ここは送り手の責任)
ことである(Claude 11)。

## `DELETE /v1/collections/{c}/documents/{name}`

主の口(127.0.0.1:7440)に足す。`collections/<c>/<name>` を tombstone し、200 `{name, seq}`
(無ければ 404)。索引の温めに合図する。管理下でないコレクションの文書を名前で消すための
もので、今の `PUT /v1/refs/… {"target":null}` と同じ効果を文書の名前で打てる形にしたもので
ある。管理下のコレクションには 409 を返し、代わりに `POST /v1/feeds/{c}/edit` を使うよう
本文で言う。書き口には出さない。

## 別に起票すること

- 今の ingest の doc_rev も previous を `s256:` 付きで持つので、同じ文書の過去の全版が公開中の
  版から辿れ、gc で回収されない(GC.md の「同じ ref パスへの上書き」で孤児が生まれるという
  記述と食い違う)。feed とは独立の既存の問題として RAG.md に起票し、GC.md を直す。
  2026-09-05 の dry-run で孤児が 1 件しか無かったことと整合する。

## やらないこと

- 読み口(7441)から書き口の API を通すこと。
- ステージング用のコレクション(上の理由)。
- 書き口からの検索・読み出し。
- 複数のコレクションを 1 つの書き口で受けること(要るなら口を増やす)。
- 管理下のコレクションへの「強制」の書き込み(直すなら edit を通す)。

## 実装の段と粒度

| 段 | 内容 | 粒度 |
|---|---|---|
| F1 | `DELETE /v1/collections/{c}/documents/{name}`(主の口。管理下なら 409) | S |
| F2 | feed の 3 本(読む・置く・公開する)と edit・allow-shrink、名前の文法、閉包の検査、管理下の判定と各入口の拒否、前進の関数(ストアを開く共通の道と要求の中)、失敗の境界(やり直し・終了コード 75)、status の欄、SPEC と design(INGEST・GC・新しい FEED)と SYSTEMD.md の更新 | L |
| F3 | serve の `--listen-feed`・`--feed-collection` と門・本文の上限、install の引数・nft の 2 規則・確認 | M |
| F4 | 本番の据え付け(操作者に sudo の 1 ブロック。今の本番の引数を全部残す)、wg1 の AllowedIPs の確認、crystal からの疎通と初回の run の確認 | S |

F2 の完了条件(テストで固定する):

- 7 の各 ref の書き込み・8 の published の張り・pending の tombstone のそれぞれで set_ref に
  I/O の誤りを注入すると(プロセスを殺すだけでなく、誤りを返させる)、プロセスは応答を返さずに
  終了コード 75 で終わり、その間に検索は途中の表を見ない。次に開いたとき(serve・mcp・CLI の
  どれでも)`GET /v1/feeds/{c}` は pending が null で、コレクションの ref が manifest と一致する。
- 前進そのものが失敗すると、serve は listener を開かず 75 で終わる。前進が終わるまで listener も
  索引の温めも始まらない。
- 6 より前の失敗(409・413・422)と、2 の途中で送りをやめたときは、公開中の中身が変わらない。
- 2 と 3 の間に gc を走らせてチャンクを回収させると、3 は 409 を返し、置き直せば通る。
- 管理下のコレクションへの、主の口の文書 PUT・fetch・汎用 ref の PUT・DELETE、MCP の
  add_document、読み口の書く口、ストアを直接開く CLI の取り込みが、どれも 409 で断られる。
  edit を通した消去の後、GET の documents と検索が見る ref が一致する。
- 別の run の doc_rev、別の feed の doc_rev、base の違う doc_rev、閉包の欠けた doc_rev を 3 に
  渡すと 409。同じ commit で documents の違う再送は 409、同じものの再送は 200。
- 文法に合わない path・commit・拡張子は 400。コレクションに先に ref があると、最初の公開は 409。
- 公開から外れた文書の doc_rev とチャンクが、次の gc で回収される(previous を辿らない)。
- 縮みの歯止め: 正味の減りが条件を満たすと 422 で消える名前の一覧が返り、allow-shrink の後は
  その commit だけが通る。

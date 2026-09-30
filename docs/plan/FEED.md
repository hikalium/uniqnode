# FEED: lamalium の文書を crystal から専用の書き口で受ける(vega 側の設計)

<a id="fa8de6f9-59f8-4512-a815-9f41d305db15"></a>

2026-09-30 の操作者の裁定(案 C)を受けた、vega 側の設計である。crystal の timer が lamalium の
文書を HTTP で vega へ送り、vega はそれを専用の書き口で受けてコレクション lamalium に入れる。
送り手(crystal 側)の設計は lamalium 側が書き、双方の設計が別モデルのレビューを通ってから
実装に入る。経緯と他の案は [docs/plan/LAMALIUM.md](#68571059-94ed-4aa2-8ae0-b2862d1de44e) の
「L4 で要るもの」。

版: 第 3 版(2026-10-01)。第 1 版(004c305)への Codex のレビュー(H1・H2・M1〜M3)と Claude の
レビュー(1〜14)、第 2 版(708bcdc)への再レビュー(Codex H1a・H1b・M4、Claude N1〜N10)を
取り込んだ。各節の末尾の括弧に、どの指摘への答えかを書く。

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
- reflog と pack の追記(store.rs の `append_record`)は、書きかけや sync の失敗の後に巻き戻さない。
  次に開いたときの再生は、最初の壊れたレコードから後ろを切り捨てる。したがって追記に失敗した
  後に同じプロセスで追記を続けると、その後ろに書いて応答まで済ませたものが、次の起動で消える
  (他のコレクションへの書き込みも含む)。sync_data が失敗したときは、完全なレコードが
  ディスクに残って次の起動で有効になるかもしれず、残らないかもしれない(結果不明)
  (Codex H1a・H1b・Claude N1)。

したがって「ステージング用のコレクションに入れて差し替える」形は採らない。差し替えそのものが
ref を 1 本ずつ書く作業になり、途中で落ちれば混ざる。その上、ステージングのコレクションは
検索に見えてしまう(`collections/` の下は全部索引に入る)。

採るのは「オブジェクトだけ先に置き、ref は公開の 1 回で、1 回のロックの中でまとめて張る。張る
予定の表(manifest)を先に 1 本の ref で保存し、張る途中で止まったら最後まで張り切る(前へ
進める)」形である。読み手の側の保証は、次の 1 つの規則で作る: 管理下のコレクション c は、
メモリの ref の表に `feeds/<c>/pending` が在る間、検索にも読み出しにも見せない(索引に入れず、
c を名指した検索と文書の読み出しは 503)。正常な公開では pending はロックの中で張られて同じ
ロックの中で消えるので、誰にも見えない。途中で止まったときだけ pending が残り、c は「前の公開」
でも「混ざった表」でもなく「見えない」になる。前へ進め終えれば pending が消えて見える。
したがって、どの読み手も c について「前の公開」か「この公開」か「一時的に見えない」のどれかしか
見ず、2 つのコミットが混ざった表は見ない。これはストアを開くどの入口(serve・mcp・CLI)でも
同じで、ディスクに途中の状態が残ったまま別のプロセスが開いても成り立つ(Claude 3)。

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
- 送り手も同じ定義を使う(この節の文言が正典で、送り手の実装はこれを写す)。文法に合わない
  追跡ファイル(空白・`+`・`@`・非 ASCII を含むなど)は、送り手が送らずに範囲から外し、commit の
  本文の `skipped` に並べて報告する。vega は `skipped` を manifest に写し、GET と status で見せる。
  1 つのファイルの名前のせいで以後の run が全部止まる(PUT が 400 → 規律 1 で commit しない)
  ことは起きない。外したファイルは一度も公開されていないので、外しても消去は起きない
  (Claude N10)。

## 管理下のコレクション(書き手は feed の transaction だけ。Codex H2・Claude 9)

管理下かどうかはストアの中身で決める: 自分の署名の ref `feeds/<c>/published` か
`feeds/<c>/pending` が在れば、コレクション c は feed の管理下である(serve の引数でなくストアで
決めるのは、serve を止めてストアを直接開く CLI にも同じ判定を効かせるため)。判定と拒否は
`Store::set_ref` の中の 1 箇所に置く(`collections/<c>/` と `feeds/` の下を書こうとしたら見る)。
feed の transaction だけが使う別の入口(例 `Store::set_ref_as_feed`)を用意し、それだけが管理下の
ref を書ける(Claude N4)。したがって次の入口のどれからも、管理下の
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

最初の公開: base が null の commit は、コレクションに target が null でない自分の署名の ref が
1 本も無いときだけ通す(在れば 409 で、その名の一覧を返す)。tombstone だけが残っている名前は
空とみなす(先に DELETE で片付けた場合。Codex M4)。既に在る中身を基準に取り込む道は作らない。
lamalium コレクションは今まだ作っていないので、この条件で困ることは無い。もし先に何かが
入っていたら、管理下になる前なので DELETE で消してから始める。

他の署名者の ref: 検索は今、どの署名者の `collections/` の ref も索引に入れる。管理下の
コレクションでは自分の署名の ref だけを索引に入れる(search.rs の visit_indexable_chunks に
1 つの条件)。vega の本番のストアは今ピアから同期していないが、同期を始めたときに、他の
ノードが同じ名前のコレクションへ書いたものが混ざらないようにする。

feed を退役させる(管理下から外す)ときは、主の口にだけ置く `POST /v1/feeds/{c}/retire
{"manifest":"<公開中の manifest>"}` で `feeds/<c>/published` を tombstone する(pending が在れば
409)。コレクションの ref はそのまま残り、以後は普通のコレクションとして主の口から書ける
(Claude N4)。

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
     "shrink_allowance":null | {"base":"s256:…","max_deleted":N},
     "skipped":[…], "writes_disabled":null | "<理由>"}
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
 "documents":{"docs/design/X.md":"s256:<2 で返った doc_rev>", …},
 "skipped":["<文法に合わず送らなかったパス>", …]}
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
5. 縮みの歯止め(Claude 10・N7): documents が空、または「新しい件数 < 0.75 × 公開中の件数、かつ
   減る件数 > 10」なら 422 で断る。ただし、有効な縮みの承認があり、その base がこの run の base と
   同じで、消える名前の数がその max_deleted 以下なら通す。見るのは消える名前の数ではなく正味の
   減りなので、ディレクトリの改名(60 件が消えて 60 件が増える)は通る。30 件を落とす不具合は
   件数では止まらないが、manifest の差として応答の deleted_names に残る。422 の本文は公開中の
   件数・新しい件数・消える名前の一覧を持つ。
   操作者の承認は主の口にだけ置く `POST /v1/feeds/{c}/allow-shrink {"base":"<公開中の manifest>",
   "max_deleted":N}` で、ref `feeds/<c>/allow_shrink` に記録する。承認を commit でなく base に
   束ねるのは、承認の後に lamalium が次のコミットへ進んでも、同じ base の上の run なら承認が
   効くようにするためである。承認は、それを使った公開が成功したとき(8 の中で)と、別の公開で
   base が進んだときに無効になる(8 が tombstone する)。送り手が承認を出すことは無い(送り手の
   API に承認の欄は無い)。
6. manifest オブジェクト
   `{"v":1,"kind":"feed_manifest","collection":c,"commit":…,"edit":false,
   "previous":"<base の manifest の 16 進(s256: を付けない)>","documents":{…}}` を置き、
   ref `feeds/<c>/pending` を張る。ここから先が下の「失敗の境界」の書き込みの段である。previous を `s256:` の
   無い形にするのは 2 の doc_rev と同じ理由で、公開中の manifest から過去の manifest とその
   doc_rev が gc に辿られないようにするため(Claude 8)。
7. 名前ごとに `collections/<c>/<name>` を張る(今の target と同じなら書かない)。base にあって
   documents に無い名前は tombstone する(消去。改名は新しい名の張りと古い名の tombstone
   として、ここで同時に起きる)。
8. `feeds/<c>/published` を manifest に張り、`feeds/<c>/allow_shrink` が在れば tombstone し、
   最後に `feeds/<c>/pending` を tombstone する。ここで初めて「前回送った名前の一覧」が新しくなり
   (規律 2)、c が再び見える。
9. ロックを放してから索引の温めに合図する。

応答: `200 {"commit":…,"manifest":…,"bound":N,"deleted":N,"unchanged":N,"deleted_names":[…]}`。

ロックの長さ: 7 は名前 1 つにつき 1 回の fsync で、変わった文書の数だけかかる。9 の後の索引の
作り直し(本番の規模で約 10 秒)もロックの中で走る。毎時 1 回の公開なら、その間の検索が待つ
のは許容する(Claude 13)。

### 失敗の境界(Codex H1・H1a・H1b・Claude 2・3・N1・N2・N3)

失敗を 2 種類に分ける。

- 検証の失敗(3 の 0〜5 で、何も書き始める前に見つかるもの。409・413・422 と 400): 公開は何も
  変わっていない。4xx と理由を返し、serve は普通に動き続ける。送り手は「前の公開が残っている」と
  扱って良い。
- 書き込みの失敗(6 の manifest の put_object から後ろ、および 2 の put_object での I/O の誤り。
  書きかけ・sync の失敗を含む): 結果は「不明」である。完全なレコードがディスクに残って次の
  起動で有効になるかもしれず、残らないかもしれない(Codex H1b)。

書き込みの失敗への対処は、feed に限らずストア全体の規則として置く(Codex H1a・Claude N1)。

1. ストアの追記(`append_record`)が誤りを返したら、同じプロセスでは二度と追記しない。
   ストアは「書けない」状態に入り、以後の書き込みは全部 503(理由つき)で断る。読み出しは
   続ける。やり直しはしない(壊れた末尾の後ろに書くと、次の起動でそれが消えるから)。
2. 追記の前の長さを覚えておき、失敗したら set_len でそこまで切り詰めて sync するのを 1 度だけ
   試みる(できれば次の起動の再生が楽になる。できなくても再生が切り詰める。どちらでも 1 の
   「書けない」状態には入る)。
3. メモリの表は、sync まで成功した追記の分しか進まない(今の apply_verified の順のまま)。
   したがって書けない状態のメモリの表は「最後に成功した追記まで」で、ディスクの再生の結果とは
   最大 1 レコードだけ違いうる(結果不明の 1 本)。
4. 公開の途中(pending を張った後)で書けなくなった場合、メモリには pending が残るので、上の
   「前提」の規則で c は見えなくなる。混ざった表は見えない。pending を張る追記そのものが失敗
   した場合、メモリには pending が無く、コレクションの ref も 1 本も変わっていないので、前の
   公開が見え続ける(ディスクには pending が残ったかもしれず、そのときは次の起動で前進する)。
5. 書けない状態から戻るのは、ストアを開き直したとき(serve の再起動)だけである。/v1/status と
   `GET /v1/feeds/{c}` の `writes_disabled` に理由を載せ、serve の記録に 1 行残す。serve は
   終了しない。検索と読み口は、他のコレクションについて答え続ける(Claude N2)。プロセスを
   終わらせないので、ロックの poison も起きない(Claude N3)。

前進(pending の manifest に従って 7・8 を最後までやる)は 1 つの関数に置き、ストアを開く共通の
道(serve・mcp・CLI が通る `Store::open` の直後の 1 箇所)で呼ぶ。加えて commit・edit・GET の
先頭(ロックの中)でも呼ぶ。7 は「今の target と同じなら書かない」ので、何度呼んでも同じ
結果になる。前進が失敗したとき(書き込みの失敗なら上の規則で書けない状態に入る。manifest が
読めないなどの検証の失敗なら c だけを止める)、ストアを開く道は失敗を返さない: その c は pending
が残るので見えないまま、feed の書き込みは 503 で断り、/v1/status の `pending` と理由に載せる。
serve は起き、listener を開き、他のコレクションは普通に答える。再試行は、書けない状態でなければ
commit・edit・GET のたびに行う(Claude N2)。CLI は前進に失敗したら理由を言って終了コード 1 で
終える。

`/v1/status` には feed ごとの `published_at`(最後の公開の時刻)・`pending`・`writes_disabled` を
載せる。送り手と監視は published_at の古さで「公開が止まっている」ことを見る(Claude 2・10)。

送り手から見ると、3 の応答が 200 なら公開済み、4xx なら公開されていない、503・5xx・接続が
切れたなら「結果不明」である。結果不明のときは次の run で 1 からやり直す: 公開が済んでいれば
3 の 1 で 200 の no-op、済んでいなければ普通の run になる(規律 3)。503 の本文が書けない状態を
言っていれば、操作者に上げる(vega の再起動が要る)。

`feeds/` の ref は検索の対象外(索引は `collections/` だけを見る)で、gc の根になる。公開中の
manifest と pending の manifest、そこから `s256:` で辿れる doc_rev とその閉包は回収されない。
過去の manifest と doc_rev は previous が参照でないので辿られず、公開から外れれば回収される。
previous の形(`s256:` を付けない 16 進 64 字)は SPEC §4.3 に「参照ではない ID の書き方」として
1 文足し、符号化と復号は 1 つの helper に置く。previous で辿れる履歴は best-effort であり、
回収された版は辿れない(Claude N9)。

### 送り手の 1 回の流れ(まとめ)

```
GET  /v1/feeds/lamalium                                     → published.manifest を base に固定
PUT  /v1/feeds/lamalium/runs/<C>/documents/<path>?base=<B>  × 全文書(1 つでも失敗したら終わる)
POST /v1/feeds/lamalium/runs/<C>/commit {base, documents}
       200 → 終わり
       409 → 最初からやり直す(送り手の側で回数に上限を置く)
       413・422 → 操作者に上げる(422 は最初の 1 回だけ知らせ、以後の run は同じ 422 を黙って受ける)
       503・5xx・接続が切れた → 結果不明。次の run で最初からやり直す(公開済みなら no-op で 200)。
                               503 の本文が「書けない」なら操作者に上げる
```

送り手は消すものを自分で計算しない(3 が manifest の差から出す)。送り手が守るのは「その
コミットの対象の全文書を送り、全部の PUT が 200 だったときだけ commit を打つ」ことと、
「同じ文書名に潰れる 2 つのパスがあれば送らない」(文書名は拡張子まで含むので、ふつうは
起きない)ことと、「公開中より古いコミットを送らない」(`git merge-base --is-ancestor
<公開中の commit> <C>` で確かめる。vega 側は祖先関係を知らないので、ここは送り手の責任)
ことである(Claude 11)。1 の GET で `published.commit` がこの run のコミットで `edit` が true なら、
その run は完了として扱い、送らない(手の edit の後、次のコミットまで毎時 409 になるのを
避ける。Claude N5)。

## `DELETE /v1/collections/{c}/documents/{name}`

主の口(127.0.0.1:7440)に足す。`collections/<c>/<name>` を tombstone し、200 `{name, seq}`
(無ければ 404)。索引の温めに合図する。管理下でないコレクションの文書を名前で消すための
もので、今の `PUT /v1/refs/… {"target":null}` と同じ効果を文書の名前で打てる形にしたもので
ある。管理下のコレクションには 409 を返し、代わりに `POST /v1/feeds/{c}/edit` を使うよう
本文で言う。書き口には出さない。

## 別に起票すること

- 今の ingest の doc_rev も previous を `s256:` 付きで持つので、同じ文書の過去の全版が公開中の
  版から辿れ、gc で回収されない(GC.md の「同じ ref パスへの上書き」で孤児が生まれるという
  記述と食い違う)。feed とは独立の既存の問題として RAG.md の項目 18 に起票した。
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
| F2 | ストアの「書けない」状態(追記の失敗の後は二度と追記しない・切り詰めの試み・503)、pending の間は c を見せない規則(索引と読み出し)、feed の 3 本(読む・置く・公開する)と edit・allow-shrink・retire、名前の文法、閉包の検査、管理下の判定(`Store::set_ref` の中)と feed 専用の入口、前進の関数(ストアを開く共通の道と要求の中)、status の欄、空き容量を読む statvfs の FFI(依存を足さない)、SPEC(§4.3 の previous の形)と design(INGEST・GC・新しい FEED)と SYSTEMD.md の更新 | L |
| F3 | serve の `--listen-feed`・`--feed-collection` と門・本文の上限、install の引数・nft の 2 規則・確認 | M |
| F4 | 本番の据え付け(操作者に sudo の 1 ブロック。今の本番の引数を全部残す)、wg1 の AllowedIPs の確認、crystal からの疎通と初回の run の確認 | S |

F2 の完了条件(テストで固定する):

- 追記の失敗を 3 通り注入する: 書く前の誤り、書きかけ(一部だけ書いて誤り)、書き終えた後の
  sync の誤り。どれでも、ストアは書けない状態に入って以後の書き込みを 503 で断り、serve は
  終了せず、他のコレクションの検索は答え続ける。開き直すと、応答が 200 だった書き込みは
  1 つも消えていない。
- 7 の各 ref の書き込み・8 の published の張り・pending の tombstone のそれぞれで上の失敗を
  注入すると、c は検索にも読み出しにも見えなくなり(混ざった表は見えない)、開き直すと前進して
  pending が null になり、コレクションの ref が manifest と一致する。pending を張る追記そのものが
  失敗したときは、前の公開が見え続ける。
- 前進が失敗しても serve は起きて listener を開き、他のコレクションは答え、c は見えず、status に
  理由が載る。
- 6 より前の失敗(400・409・413・422)と、2 の途中で送りをやめたときは、公開中の中身が変わらない。
- 2 と 3 の間に gc を走らせてチャンクを回収させると、3 は 409 を返し、置き直せば通る。
- 管理下のコレクションへの、主の口の文書 PUT・fetch・汎用 ref の PUT・DELETE、MCP の
  add_document、読み口の書く口、ストアを直接開く CLI の取り込みが、どれも 409 で断られる。
  edit を通した消去の後、GET の documents と検索が見る ref が一致する。retire の後は普通の
  コレクションとして書ける。
- 別の run の doc_rev、別の feed の doc_rev、base の違う doc_rev、閉包の欠けた doc_rev を 3 に
  渡すと 409。同じ commit で documents の違う再送は 409、同じものの再送は 200。
- 文法に合わない path・commit・拡張子は 400。target の在る ref がコレクションに先にあると、最初の
  公開は 409。tombstone だけが残っているときは通る。
- 公開から外れた文書の doc_rev とチャンクが、次の gc で回収される(previous を辿らない)。
- 縮みの歯止め: 正味の減りが条件を満たすと 422 で消える名前の一覧が返る。同じ base への
  allow-shrink の後は、別のコミットの run でも max_deleted 以下なら通り、公開が成功すると承認は
  消える。

# FEED: lamalium の文書を crystal から専用の書き口で受ける(vega 側の設計)

<a id="fa8de6f9-59f8-4512-a815-9f41d305db15"></a>

2026-09-30 の操作者の裁定(案 C)を受けた、vega 側の設計である。crystal の timer が lamalium の
文書を HTTP で vega へ送り、vega はそれを専用の書き口で受けてコレクション lamalium に入れる。
送り手(crystal 側)の設計は lamalium 側が書き、双方の設計が別モデルのレビューを通ってから
実装に入る。経緯と他の案は [docs/plan/LAMALIUM.md](#68571059-94ed-4aa2-8ae0-b2862d1de44e) の
「L4 で要るもの」。

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
  `sync_data` し、落ちたときの単位は 1 レコードになる(store.rs の `append_own_record`)。
  複数の ref をまとめて書くレコードも、前の値を条件にする書き込み(compare-and-swap)も無い。
- 検索の見え方は、ストアのロックを取った瞬間の ref の表で決まる(索引の世代は
  `collections/` の ref の束縛)。1 回のロックの中で書いた複数の ref は、同じプロセスの読み手
  (検索・索引の温め)からは途中の状態が見えない。ただし途中で落ちると、書いた分だけが残る。

したがって「ステージング用のコレクションに入れて差し替える」形は採らない。差し替えそのものが
ref を 1 本ずつ書く作業になり、途中で落ちれば混ざる。その上、ステージングのコレクションは
検索に見えてしまう(`collections/` の下は全部索引に入る)。

採るのは「オブジェクトだけ先に置き、ref はコミットの 1 回でまとめて張る。張る途中で落ちたら、
起動時に最後まで張り切る(前へ進める)」形である。張る前に、張る予定の表(manifest)を
1 本の ref で保存するので、途中で落ちても何を張るべきかは残る。

## 口

serve に、読み口とは別の待ち受けを足す。

```
uniqnode serve … --listen-feed 10.10.128.1:7446 --feed-collection lamalium
```

- `--feed-collection` は 1 つだけ受ける(書けるコレクションは 1 つ)。
- 通すのは下の「送り手が守る API」の 3 本と `/healthz`・`/v1/status` だけで、それ以外は 403。
  検索も文書の読み出しも通さない(送り手は読まない。読むのは読み口の仕事)。
- 門の判定は agent_door.rs と同じ形の関数(許可表を引数に取る)を別に置く。読み口の門と
  混ぜないのは、読み口の許可を広げる変更が書き口に漏れないようにするためである。
- 記録は読み口と同じ行の形で、印を `feed` にする。要求ヘッダ `X-Uniqnode-Task` と
  `X-Uniqnode-Agent` は読み口と同じく記録に写すだけで、判断には使わない。
- 読み口の許可表(`--agent-writable`)に lamalium を入れてはならない。書き手は書き口 1 つに
  限る(install が両方に同じ名があれば断る)。読み口から lamalium を読ませるには、今と同じく
  `--agent-collections lamalium` を足す。

install:

- `--listen-feed <addr>`・`--feed-collection <c>`・`--feed-firewall-allow <addr,...>` を足し、
  drop-in の ExecStart に写す。
- nft は同じ実体の表 `inet uniqnode_<インスタンス>` の同じ chain に、口ごとに 1 規則を足す。
  `ip daddr 10.10.128.1 tcp dport 7446 ip saddr != { 10.10.128.2, 10.10.128.1 } counter drop`
  (自分の IP を許すのは、install の確認が同じ機械から届くため。読み口と同じ理由)。
- 確認を 1 本足す。許していない名への run の PUT が 403、検索が 403、`GET /v1/feeds/lamalium` が
  200 であることを見る(試し書きはしない)。
- 本番の据え付けは sudo が要るので、操作者に 1 つのコードブロック(`2>&1 | ts … | tee
  /tmp/uniqnode-install-feed.log` 付き)で渡す。

## 送り手が守る API

1 回の送りを run と呼び、run はコミットの ID(40 桁の 16 進)で名指す。名前 `{name}` は
取り込み起点からの相対パスから拡張子を除いたもの(`docs/design/X.md` なら `docs/design/X`)で、
PUT の最後の段には拡張子を付けて送る(serve が拡張子で種別を決める。今の PUT documents と同じ)。

### 1. `GET /v1/feeds/{c}` — 今公開している中身を読む

```
200 {"collection":"lamalium",
     "published":{"commit":"<40 桁>","manifest":"s256:…","at":"<時刻>",
                  "documents":{"docs/design/X":"s256:<doc_rev>", …}} | null,
     "pending":null | {"commit":"…","manifest":"s256:…"}}
```

送り手は run の最初にこれを読み、`published.commit` を覚える(下の 3 で `previous` に送る)。
「前回送った名前の一覧」はこの `documents` の鍵であり、送り手が自分で持つ必要は無い
(持っても良いが、正典は vega 側の manifest である)。`pending` が null でないのは、張る途中で
落ちて、まだ前へ進めていない状態である(serve の起動時に進めるので、ふつうは見えない)。

### 2. `PUT /v1/feeds/{c}/runs/{commit}/documents/{name}.{ext}` — 文書を置く(ref は張らない)

本文は生のバイト列。serve は今の PUT documents と同じ抽出とチャンク分けをし、blob・チャンク・
doc_rev をオブジェクトとして置くが、ref は張らない。

- 公開中の同じ名前の doc_rev と内容(元のバイト列とチャンクの並び)が同じなら、新しい doc_rev を
  作らず、公開中の doc_rev をそのまま返す(`"unchanged":true`)。これで、中身の変わらない文書は
  run ごとに ref を書かない(reflog が毎時 200 本ずつ伸びるのを避ける。RAG.md の項目 16)。
- 内容が違えば、doc_rev の meta に `source_commit=<commit>` と `feed=<c>` を入れて作る。つまり
  各文書の `source_commit` は「今の内容を最初に送ったコミット」である。run のコミットそのもの
  は manifest が持つ。
- 応答: `200 {"name":"…","doc_rev":"s256:…","unchanged":bool,"new_objects":N}`。
- 失敗は 4xx/5xx と理由の本文。送り手は 1 つでも失敗したら、その run の 3 を打たずに終える
  (規律 1)。置いたオブジェクトは ref から辿れない孤児になり、gc が回収する。
- 同じ run の同じ文書を何度置いても同じ ID が返る(内容で決まる)。落ちた後のやり直しは、
  そのまま全部置き直せば良い(規律 3)。

### 3. `POST /v1/feeds/{c}/runs/{commit}/commit` — まとめて公開する

```
{"previous":"<公開中のコミット>" | null,
 "documents":{"docs/design/X":"s256:<2 で返った doc_rev>", …},
 "allow_shrink":false}
```

serve はストアのロックを 1 回取り、その中で次を順に行う。

1. `previous` が公開中のコミットと違えば 409(別の run が先に公開した。送り手は 1 からやり直す)。
   公開中のコミットが既にこの `commit` で、`documents` も同じなら、何もせず 200 を返す
   (落ちた後のやり直しで、公開が済んでいた場合。規律 3)。
2. `documents` の各 doc_rev がストアにあり、kind が doc_rev で、meta の name が鍵と一致する
   ことを確かめる。無ければ 409(2 の後に gc が孤児として回収した。送り手は 2 から置き直す)。
3. 縮みの歯止め: `documents` が空、または公開中の名前の 25% 超(かつ 10 件超)を消すことに
   なるなら、`allow_shrink` が true でない限り 422 で断る。空の一覧で全部消える事故を防ぐ。
4. manifest オブジェクト `{"v":1,"kind":"feed_manifest","collection":c,"commit":…,
   "previous":<前の manifest>,"documents":{…}}` を置き、ref `feeds/<c>/pending` を張る。
   これが「張る予定の表」の保存で、ここから先で落ちても起動時に前へ進められる。
5. 名前ごとに `collections/<c>/<name>` を張る(今の target と同じなら書かない)。公開中に
   あって `documents` に無い名前は tombstone する(消去。改名は新しい名の張りと古い名の
   tombstone として、ここで同時に起きる)。
6. `feeds/<c>/published` を manifest に張り、`feeds/<c>/pending` を tombstone する。
   ここで初めて「前回送った名前の一覧」が新しくなる(規律 2)。
7. ロックを放してから索引の温めに合図する。

応答: `200 {"commit":…,"manifest":…,"bound":N,"deleted":N,"unchanged":N}`。

5 と 6 は同じロックの中なので、検索は「前の公開」か「この公開」のどちらかしか見ない。途中で
落ちた場合は、次の起動で listener を開く前に `feeds/<c>/pending` を見つけ、その manifest に
従って 5・6 を最後までやる(前へ進める)。5 は「今の target と同じなら書かない」ので、何度
やっても同じ結果になる。4 より前で落ちた場合は、公開は何も変わっていない。

`feeds/` の ref は検索の対象外(索引は `collections/` だけを見る)で、gc の根になるので、
公開中の manifest と、そこから辿れる doc_rev は回収されない。manifest の `previous` を
辿れば公開の履歴が読める。

### 送り手の 1 回の流れ(まとめ)

```
GET  /v1/feeds/lamalium                                  → published.commit を previous に
PUT  /v1/feeds/lamalium/runs/<C>/documents/<name>.<ext>  × 全文書(1 つでも失敗したら終わる)
POST /v1/feeds/lamalium/runs/<C>/commit {previous, documents, allow_shrink:false}
       409 → 最初からやり直す / 422 → 操作者に上げる / 200 → 終わり
```

送り手は消すものを自分で計算しない(3 が manifest の差から出す)。送り手が守るのは「その
コミットの対象の全文書を送り、全部の PUT が 200 だったときだけ commit を打つ」ことだけである。

## `DELETE /v1/collections/{c}/documents/{name}`

主の口(127.0.0.1:7440)に足す。`collections/<c>/<name>` を tombstone し、200 `{name, seq}`
(無ければ 404)。索引の温めに合図する。操作者が手で古い文書を消すためのもので、書き口には
出さない(書き口の消去は 3 の manifest の差で行う。2 つの道で消すと、manifest と実際の ref が
食い違う)。今の `PUT /v1/refs/… {"target":null}` と同じ効果を、文書の名前で打てる形にした
ものである。

## やらないこと

- 読み口(7441)から書き口の API を通すこと。
- ステージング用のコレクション(上の理由)。
- 書き口からの検索・読み出し。
- 複数のコレクションを 1 つの書き口で受けること(要るなら口を増やす)。

## 実装の段と粒度

| 段 | 内容 | 粒度 |
|---|---|---|
| F1 | `DELETE /v1/collections/{c}/documents/{name}`(主の口) | S |
| F2 | feed の 3 本(置く・公開する・読む)と、起動時の前進(pending の張り切り)。主の口にも出して、テストはそこで打つ | M |
| F3 | serve の `--listen-feed`・`--feed-collection` と門、install の引数・nft の規則・確認 | M |
| F4 | 本番の据え付け(操作者に sudo の 1 ブロック)と、crystal からの疎通・初回の run の確認 | S |

F2 の完了条件: 3 の 5 の途中で serve を殺しても、再起動後の `GET /v1/feeds/{c}` が pending
null で、コレクションの ref が manifest と一致する(テストで固定)。PUT の途中で送りをやめても、
公開中の中身が変わらない。

# DISTRIBUTED_SEARCH — 分散検索(SPEC §7.1 の kind:search・署名付き QUERY・順位の融合)

<a id="e577f6db-659e-4eb8-a152-3b7780e4a9d1"></a>

読み手は、分散検索のコード(node/src/distributed_search.rs と、node/src/api.rs の
POST /v1/search・POST /v1/peer/query)を読む者と、複数のDBノードに知識を分けて運用する者。
検索そのものの実装は [docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580)、
1 台に閉じたクエリ(kind:object / kind:refs)と開世界の決着は
[docs/design/OVERVIEW.md](#cfa85cc2-0d72-49f2-b628-b9622589851b) と SPEC §7 にある。
この文書は分散検索の現在の実装を記述する。

問いはテキストのまま運ばれ、各DBノードが自分の索引で検索して、自分の top-k を順位として
返す。返ってきた順位は要求側で RRF に掛けて 1 本にまとめる。順位だけを使うので、DBノード
間で得点を較正する必要がない。ベクトルは線に載らない(埋め込みの模型が揃っている必要が
ない。SPEC §7.1 の MUST)。

## 誰が何を決めるか

| 判断 | 家 |
|---|---|
| QUERY / ANSWER の形と署名・検証 | node/src/distributed_search.rs |
| 答える相手か・見せてよいコレクションか | 同 answer_policy(材料は peers.json) |
| 決着の 3 値(found / scope_empty / timed_out) | crate::query::settlement |
| 検索の方式・劣化・引用 | node/src/api.rs の run_search |
| 順位の融合 | crate::embed::fuse_by_rank |

分散検索は検索の判断を持たない。ピアからの QUERY に答えるときも、自分のために引くときも、
通るのは同じ run_search である(検索の判断を二重に実装しない。should/0135)。

## 線の上の形

QUERY(要求側 → 応答側。POST /v1/peer/query の本文)。sig を除いた c1 の正規形が署名対象で、
鍵は origin そのもの(DBノードID = Ed25519 公開鍵。SPEC §6.1)である。

```jsonc
{ "v": 1, "kind": "search", "query_id": "<16 進 32 桁>", "origin": "<DBノードID>",
  "payload": { "query": "…", "collection": "notes", "top_k": 10, "method": "hybrid" },
  "budget_ms": 2000, "at": 1786940271, "sig": "<16 進 128 桁>" }
```

ANSWER(応答側 → 要求側。同じ HTTP 応答の本文)。

```jsonc
{ "v": 1, "query_id": "…", "responder": "<DBノードID>",
  "payload": { "method": "bm25", "degraded": "…", "filtered_low_information": 2,
               "results": [ { "citation": { … }, "id": "s256:…", "snippet": "…" } ] },
  "at": 1786940271, "sig": "…" }
```

- payload は POST /v1/search の要求・応答と同じ形である(要求は search_request_value、
  引用は citation_value がそれぞれ 1 箇所で組む。形を二重に定義しない。should/0135)。
- ANSWER は得点を載せない。融合は順位しか使わないので要らず、載せれば小数が入って c1 の
  正規形(整数のみ)から外れ、署名対象を別に定義することになる。順位は results の並び
  そのものである。
- scope は線に載せない。転送は 1 ホップだけ(SPEC §7.1 の MUST)なので受け手は使わず、
  載せれば自分のピア一覧を配って回ることになる。要求側の絞り込み(どの宛先へ送るか、
  min_trust_level 以上か)は送信前に効く。
- 宛先はメッセージに書かない。同じバイト列を全ピアへ送るので、署名は 1 回で済む。
- kind:object と kind:refs はこの口では受けない(400)。応答が自己認証的なそちらは
  GET で運ぶ(SPEC §7.1 の L2 の具体化)。

## 双方向フィルタ

SPEC §7.1 の「要求側は送信先を絞り、応答側も要求者と自分の共有ポリシーで応答可否を判定
する」を、peers.json の 3 つの項で実装する。ピアレコードの形は SPEC §6.3 のものである。

```jsonc
{ "peers": [ { "address": "10.0.0.3:7440", "node_id": "<相手のDBノードID>",
               "trust_level": 50, "share": { "collections": ["notes"] } } ] }
```

- node_id は、このエントリが主張する相手の DBノードID である。出ていく側では健全性エンジンが
  接触時に実際の node_id と照合し、入ってくる側では要求者の認証に使う。書かなければ相手を
  名指しで認証できないので、そのピアからの kind:search には答えない。証明書付きのエントリは
  証明書の node_id が同じ役を果たし、両方書いてあって食い違うエントリは、どちらを信じるかを
  こちらで決めずに捨てる(must/0022)。
- trust_level は 0 が「このピアには答えない」で、それ以外の数値の意味は運用者が決める
  (順序だけがプロトコルの意味を持つ)。既定は 50(定数 DEFAULT_TRUST_LEVEL)。
- share.collections は、そのピアへ出してよいコレクションである。書かなければ全部。
  要求の collection とは交差を取る(CollectionScope::intersect)。形の読めない share は、
  黙って全部を共有せず何も共有しない側へ倒す(共有ポリシーの読み違いは意図しない開示に
  なるため)。
- 答えた相手も照合する。peers.json が node_id を主張しているのに別の DBノードが署名した
  ANSWER は受け取らない。
- 応答側は、答えた要求も断った要求も 1 行残す。断りだけを残すと「誰も来なかった」と
  「来て答えた」が記録から区別できない(should/0111)。

share が限るのは見つけ方であって、取り出し方ではない。ノードローカル API の
GET /v1/objects/{id} は、ID を知っている相手にオブジェクトを返す(content-addressed で
自己認証的だから誰に渡してもよい、という §7.1 の前提の裏返しである)。したがって
「共有していないコレクション」は、検索からは辿れないが、チャンクの ID を別の道で知った
相手には読める。実測 2026-08-17: 共有していない secrets の ID を手で渡すと、
POST /v1/query(kind:object)は found でその本文を取り寄せた
([docs/analysis/20260817-distributed-search-acceptance.md](#08a2c68d-7c50-4aaf-990b-54919ef5a39c))。
オブジェクト単位の共有ポリシーは残作業である
([docs/plan/RAG.md](#86363f4a-3df6-4aa2-9c64-b99aa5cb4e7b))。

要求者の認証は署名で行う。名乗り(origin)を信じるのではなく、sig を origin の鍵で検証して
初めて「この DBノードが問うている」と言える。署名は再送を防がないので、at が
QUERY_FRESHNESS_SECONDS(300 秒)の窓の外にある封筒は受けない。窓を広く採るのは、DBノード
間の時計のずれで正当な問いを落とさないためである。

断り方は「誰が直せるか」で分ける: 400 は封筒の組み立て(呼び出し側が直す)、401 は名乗りと
署名と鮮度、403 は共有ポリシーの判断(運用者が peers.json を直す)。要求側から見れば
401 も 403 も沈黙(情報ゼロ)であり、理由は応答の peers の note に残る。

## 散布と決着

- 宛先ごとに 1 本の要求を出し、届かなければ RETRY_INTERVAL(250ms)ごとに予算まで問い直す
  (沈黙は終端ではない。SPEC §7.2)。1 往復の期限は最長 15 秒(定数 PEER_REQUEST_TIMEOUT)で、
  予算が短ければそちらが先に来る。
- 予算はクエリの属性ではなく観測の打ち切りである(既定 2000ms、要求の budget_ms で変えられる。
  0..=60000)。
- 決着は 3 値である(判定は crate::query::settlement。kind:object のクエリと同じ関数):
  - found: どこかに当たりがあった(ローカルの当たりも含む)。
  - scope_empty: スコープ内の全員が肯定的に「私の索引には無い」と言った。予算前に決着する。
  - timed_out: 予算が切れ、沈黙したピアが残っている。
  「存在しない」を表す決着値は無い。空の ANSWER は「私の索引には該当がない」という言明で
  あって沈黙ではないので、ピアの状態は empty(answered / silent と区別される)。
- 自分もクエリの参加者である。ローカルの順位は散布の前に run_search で引き、共有ポリシーの
  絞り込みは掛からない(絞りが要るのはピアに答えるときだけである)。
- 散布のあいだ store のロックは持たない。取るのは QUERY に署名する一瞬だけで、ネットワーク
  待ちのあいだ API 全体を塞がない(sync と同じロックの規律)。
- ピアが沈黙したら、その理由(接続できない・403 で断られた・署名が合わない)を標準エラーにも
  1 行出す。沈黙そのものは情報ゼロだが、理由は運用者が直せることが多い。

## 順位の融合

- 融合は RRF である(crate::embed::fuse_by_rank。定数 k = 60)。ローカルの順位と、答えた
  ピアごとの順位を、それぞれ 1 本の順位列として持ち込む。
- 同一視の鍵はチャンクのオブジェクト ID である。content-addressed なので、同じ本文の
  チャンクはどのDBノードから来ても同じ ID を持ち、1 件にまとまって順位が足し合わされる
  (両方のDBノードが上位に置いたチャンクは、片方だけのものより上に来る)。
- 抜粋と引用は最初に見た側のものを使う。ローカルを先に入れるので、手元にもあるチャンクは
  手元の引用で答える。
- 得点は融合得点なので、応答の score_semantics は必ず "rrf" である。応答の method は
  このDBノードのローカルの検索が使った方式で、ピア側の方式と劣化は peers の note が持つ
  (ピアはピアの装備で答える。埋め込みを備えたDBノードと備えないDBノードが混ざってよい)。

## POST /v1/search に散布を頼む

```jsonc
// 要求。peers を書かなければ、これまでどおりローカルだけで答える(既定)。
// peers: true は peers.json のスコープ全体、配列はその宛先だけ。
{ "query": "世代の整合 token_estimate", "peers": true, "budget_ms": 3000,
  "min_trust_level": 50 }

// 応答。ローカルだけの応答に outcome・peers・各件の sources が足された形である。
{ "method": "bm25", "outcome": "found",
  "peers": [ { "address": "127.0.0.1:7452", "node_id": "3f00773d…", "hits": 1,
               "state": "answered" } ],
  "results": [
    { "citation": { "at": 1786940271, "breadcrumbs": ["分散設計", "世代の整合"],
        "collection": "notes", "document": "memo_ja", "position": 0 },
      "id": "s256:1fe778db…", "score": 0.01639344262295082,
      "snippet": "転置索引は導出データであり…", "sources": ["local"] },
    { "citation": { "at": 1786940271, "breadcrumbs": ["Retrieval notes", "Chunker internals"],
        "collection": "notes", "document": "memo_en", "position": 0 },
      "id": "s256:f32b9537…", "score": 0.01639344262295082,
      "snippet": "The helper token_estimate returns…", "sources": ["127.0.0.1:7452"] } ],
  "score_semantics": "rrf" }
```

- 既定を「散布しない」にしてあるのは、問いの文そのものが情報であり、他のDBノードへ配るか
  どうかは呼び手が選ぶことだからである。peers を書かない要求の応答は、この機能が入る前と
  1 バイトも変わらない。
- min_trust_level は要求側のフィルタで、これ未満の trust_level のピアへは問いを送らない。
  明示のアドレスを書いたときは、その宛先へ送る(peers.json に無いアドレスも宛先にできるが、
  相手の DBノードID を知らないので、答えた相手の照合はしない)。
- peers の各項は、宛先・答えた相手の DBノードID・件数・状態(pending / answered / empty /
  silent)と、沈黙の理由か相手が言った劣化の理由(note)である。
- degraded と filtered_low_information は、このDBノードのローカルの検索についての報告である。

## 意図的に持たないもの

- 分散検索のクエリハンドル。kind:object / kind:refs は GET /v1/queries/{id} で回答の単調増加
  集合を観測できるが、kind:search は予算付きのブロッキングだけである(SPEC §7.2 のハンドルは
  SHOULD)。残作業は [docs/plan/RAG.md](#86363f4a-3df6-4aa2-9c64-b99aa5cb4e7b) にある。
- multi-hop の転送(SPEC §7.1 の MUST NOT)。1 ホップだけなので、query_id は応答の照合に
  使うだけで、重複排除の表は持たない。
- ピアが持っているチャンクの全文の自動取得。融合した結果は他のDBノードにしかない
  チャンクの ID を含むので、全文は既存の POST /v1/query(kind:object)で取り寄せる。
  ID は内容ハッシュなので、取り寄せた本文は受け取った側で検証できる。
- ピアの埋め込みの装備を揃えること。方式は各DBノードが自分の装備で決め、返るのは順位である。

## 既知の癖

- 融合は順位だけを見るので、1 件ずつしか返さなかったDBノードが 2 つ並ぶと、両者の得点は
  同じ 1/(60+1) になり、順序は ID で決まる(実測 2026-08-17: 2 台に 1 件ずつの構成で
  score 0.01639344262295082 が 2 件。
  [docs/analysis/20260817-distributed-search-acceptance.md](#08a2c68d-7c50-4aaf-990b-54919ef5a39c))。
  得点は同一応答内の順位付けにだけ意味を持つという意味論は、ローカルの検索と同じである。
- 相手のローカルの索引が初回構築なら、その待ち(実データ規模で約 9 秒。
  [docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580) の「既知の癖」)は予算の中で
  起きる。既定の 2000ms では間に合わず沈黙になるので、初回は budget_ms を伸ばすか、相手側で
  一度検索して索引を温めておく。
- MCP の search ツールは散布しない。エージェントから使う口は今のところ 1 台に閉じている
  ([docs/design/MCP.md](#dacd474d-424a-45d5-a278-766fc2465dd9))。

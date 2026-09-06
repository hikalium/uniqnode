# MCP — LLM エージェント向けアダプタ(標準入出力の JSON-RPC 2.0・読む search / fetch と、許したコレクションへ書く add_document / fetch_url)

<a id="dacd474d-424a-45d5-a278-766fc2465dd9"></a>

読み手は、MCP(Model Context Protocol)アダプタのコード(node/src/mcp.rs、node/src/main.rs の
mcp サブコマンド)を読む者と、このサーバを LLM エージェントに登録して使う者。この文書は
アダプタの現在の実装を記述する。検索そのものの実装は
[docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580)、引用の規則は取り込み層
([docs/design/INGEST.md](#47d69a3e-c39a-4e76-9814-e9c24240293b))にある。

この層は薄い被せ物である。方式の既定・劣化の判断・引用の組み立ては node/src/api.rs の
run_search が持ち、全文の取得は同じく fetch_object が持つ。REST の POST /v1/search と MCP の
search ツールは、要求の読み取り(parse_search_request)から順位付けまで同じ関数を通る。
書き込みも同じで、文書の取り込みは api.rs の put_document、URL からの取り込みは fetch_into が
持つ(REST の PUT /v1/collections/{c}/documents/{name} と POST /v1/collections/{c}/fetch の本体
そのもの)。アダプタがするのは JSON-RPC の封筒の付け外しと、LLM が読む形への整形だけである
(検索の判断も取り込みの判断も二重に実装しない。should/0135)。

書き込みは既定で閉じている。`--writable <コレクション名>` で許したコレクションにだけ書け、
1 つも許していなければ書くツールは一覧に載らない(「書き込みを許す」の節)。

後ろ盾には二つの形がある(node/src/mcp.rs の Backend)。走っている serve の REST へ転送する形と、
自分でストアを開く形である。前者はストアの排他ロックを取らないので、エージェントを繋いだまま
取り込みと埋め込みを回せる。どちらの形でも整形は同じ関数を通る。

## 起動と標準入出力の規律

- 起動は CLI のサブコマンドである:
  `uniqnode mcp <data_dir> [--serve-url <url>] [--writable <コレクション名>]... [--embed <url>] [--embedder <id>] [--rerank <url>] [--reranker <id>] [ログの指定]`。
  `--serve-url` を与えれば走っている serve へ転送し、与えなければ serve と同じ ApiContext を
  組んでストアを直接開く。`--writable` は書き込みを許すコレクションで、繰り返せる(既定は
  読むだけ。「書き込みを許す」の節)。どちらも HTTP の代わりに標準入出力で話す。ストアを直接開く形で
  埋め込みを装備するのは `--embed` を明示したときだけ、順位の取り直し
  ([docs/design/SEARCH.md](#6df75ea0-ad86-460a-be04-29660201a7fb))を装備するのは `--rerank` を
  明示したときだけで、どちらも起動時に相手の生存は確かめない(serve と同じ。should/0114)。
  組み立ても serve と同じ関数(node/src/main.rs の embedder_from・reranker_from)を通るので、
  serve で効く指定はこの形でも同じ意味で効く。
- 標準出力はプロトコル専用である(MCP の stdio 転送の規定)。1 行が 1 メッセージで、ログが 1 行
  でも混ざれば相手の解析はその行で壊れる。起動の知らせも、劣化の理由も、知らない通知の記録も、
  すべて標準エラーへ出す。同じ行は既定で `<data_dir>/logs/mcp.log` にも残る。登録した相手が
  標準エラーを吸ってしまうので、利用者が読める記録はそのファイルだけである
  ([docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004))。
- 応答の直列化は c1 の正規形(SPEC §4.1)を通す。正規形は空白を持たず制御文字を \u00xx に畳む
  ので、本文にどんな改行が混ざっても 1 メッセージが 1 行に収まることが直列化の側から保証される
  (行に収める処理を別に実装しない。should/0135)。
- 1 行読んで 1 行書き、書くたびに flush する。パイプの buffer に応答を残したまま次を待つと、
  双方が相手を待って止まる。標準入力の EOF は相手が閉じた合図で、そこで終わる。
- 要求の解析にも c1 を使う。c1 は整数しか持たないので、小数を含む要求は解析誤り(-32700)で
  断る。黙って読み飛ばさない(must/0022)。JSON-RPC の封筒も、これらのツールの引数も、文字列・
  整数・真偽値・オブジェクトだけで足りるので、実用上この制限に当たらない。

## ハンドシェイクとメソッド

| メソッド | 応答 |
|---|---|
| `initialize` | protocolVersion・capabilities・serverInfo・instructions |
| `notifications/initialized` | 返さない(受理して黙る) |
| `notifications/cancelled` | 返さない(受理して黙る) |
| `ping` | 空の結果 `{}` |
| `tools/list` | `result.tools` に読む 2 本(`--writable` があれば書く 2 本も) |
| `tools/call` | ツールの結果(content と isError) |

- 答えるプロトコル版は 2025-06-18 である(定数 PROTOCOL_VERSION)。相手が別の版を求めても誤りに
  はしない。規定は「サーバが対応する版を答える」であり、食い違いは標準エラーに残す。Claude Code
  は 2025-11-25 を求めたうえでこの引き下げを受け入れる(実測。接続に成功する)。
- capabilities には実装しているものだけを載せる。tools だけで、listChanged は false である。
- serverInfo の name は定数 SERVER_NAME("uniqnode")、version は実行ファイルの版、title は表示名。
  instructions には「search で問い、返った出典を答えに添え、抜粋で足りなければ fetch で全文を
  読む」という使い方を 1 文で入れる。書き込みを許しているときだけ、「『覚えておいて』なら
  add_document、URL を取り込めなら fetch_url で、許されたコレクションに入れる」の 1 文が続く
  (許していないのに書けると言わない)。
- 応答を返すのは id を持つ要求だけである。id を持たない通知に応答を返すと相手の解析が壊れる。
  id が 0 のメッセージは要求であって通知ではない(取り違えると応答が消える)。
- 知らないメソッドは、要求なら -32601 で「何を知らないのか」を言う。通知なら応答を返せないので
  捨てるほかないが、標準エラーには残す(must/0022)。
- ツールの一覧は `result.tools` に入る。result そのものを配列にすると、相手はツールを 1 本も
  見つけられないまま「接続はできた」状態になる。

## 二つの形(走っている serve へ転送する / ストアを直接開く)

| 形 | 起こし方 | ストアの排他ロック | 検索・取得・書き込み |
|---|---|---|---|
| 転送する形 | `uniqnode mcp <dir> --serve-url http://127.0.0.1:7440` | 取らない | 走っている serve の REST を呼ぶ |
| 直接開く形 | `uniqnode mcp <dir>` | 起動から終了まで持つ | 自分の ApiContext で答える(REST と同じ関数を直接呼ぶ) |

- 既定は直接開く形のままである。引数を足す形にしたのは、既に登録されている
  `uniqnode mcp <dir>` を壊さないためと、serve を走らせないストアに 1 人で向かう道を残すため
  である。運用として勧めるのは転送する形で、登録の例もそちらを先に置く。
- 名前を `--serve` ではなく `--serve-url` にしたのは、値が URL であること(`host:port` では
  ない)を呼び手に見せるためと、serve サブコマンドと読み違えないためである。値の形は
  `--embed <url>` と揃う。
- 転送する形でもデータディレクトリは要る。開くためではなく、届かないときに「どの serve を
  起こせばよいか」を言うためである(案内は `uniqnode serve <dir> <host:port>`)。
- 転送する形が呼ぶ REST は五つ: POST /v1/search(検索)、GET /v1/objects/{id}(全文)、
  GET /v1/objects/{id}/citation(その出典)、PUT /v1/collections/{c}/documents/{name}
  (add_document)、POST /v1/collections/{c}/fetch(fetch_url)。書き込みの二つは、直接開く形
  では同じ本体(api.rs の put_document・fetch_into)を関数として呼ぶ。答え(状態と JSON の
  本文)を読むのはどちらの形でも mcp.rs の written_document 1 箇所で、serve の 4xx/5xx は
  その本文の error をそのままツールの失敗の理由にする(黙って飲まない。must/0022)。
  応答から SearchResults と Fetched を組み直し、
  描くのは両方の形が同じ render_search / render_fetch である(整形を二重に実装しない。
  should/0135)。線の上の形は書き手(search_response_body・citation_value)と読み手
  (parse_search_response・citation_from_json)を api.rs の同じ節に並べて置き、噛み合うことを
  単体試験 rest_search_bodies_survive_a_round_trip が確かめる。
- 検索の判断は転送する形でも serve 側の run_search が 1 回だけ行う。MCP 側は判断せず、応答の
  method・degraded・citation をそのまま描く。劣化の理由も serve の言葉のまま出る。
- 埋め込みも順位の取り直しも、装備するのは転送先の serve である。転送する形に `--embed`
  (`--embedder` も)や `--rerank`(`--reranker` も)を渡すのは矛盾なので、併用は起動時に
  理由を標準エラーへ出して exit 2 で断る(黙って無視しない。must/0022。viewer も同じ判定で
  断る)。同じ理由で、ベクトルを作るだけの `embed`
  命令も `--rerank` / `--reranker` を受けない(読み手を serve と共有しているので字面は通るが、
  効かせる先が無い指定は断る)。
- serve に届かないときは、原因(接続できない、など)と起動コマンドを添えて、ツールの結果
  (isError: true)と標準エラーの両方に出す。転送する形の 1 要求の期限は 60 秒である(初回の
  検索は serve 側の索引構築を待つため、クエリ埋め込みの 15 秒より長く採る)。

## 公開するツール

- search の引数は POST /v1/search のボディと同じ形である: query(必須。空でない文字列で、索引語
  を最低 1 語含む)、collection(省略時は全コレクション)、top_k(1..=1000、省略時 10)、method
  ("bm25" / "embedding" / "hybrid"、省略時はこのノードの装備と top_k に従う。
  [docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580))、include_low_information
  (省略時 false)。読み取りと検証は parse_search_request の一箇所にあり、誤りの文言は REST の
  400 の本文と同じものが JSON-RPC の message に載る。
- search の説明文は、モデルが読む唯一の手引きである。方式と得点の意味だけでなく、問い方を
  そこに書く: 英語で問い、文書が使う英語の術語と略語をそのまま入れること。実データで測ると、
  同じ 14 主題を純日本語で問うと MRR 0.332、日本語の文に英語術語を混ぜると 0.869、英語だけなら
  0.705〜0.798 だった(実測 2026-08-17。
  [docs/analysis/20260817-real-corpus-search-quality.md](#faeda9ac-5e9e-4091-8122-2fba9f80c8db))。
  抽象的な助言では効かないので、良い例(which field reports the period at which the HPET main
  counter increments)と悪い例(高精度イベントタイマの主計数器が増える周期。この訳語は HPET
  仕様書のどのページにも無い)を 1 つずつ書き、日本語の言い方しか分からないときは日英を併記した
  1 本のクエリにする(訳が外れても片方が当たる。併記の MRR は 0.671〜0.804)ことも書く。
- 説明文に出る top_k の境目は定数 EMBEDDING_ONLY_TOP_K から組み立てる。説明に書いた既定と
  実装が選ぶ既定が食い違わないためである(must/0023)。method の enum の 3 つの文字列を
  SearchMethod の as_str から出すのと同じ理由である。
- fetch の引数は id だけ(必須。search が返すチャンクのオブジェクト ID。`s256:` と 16 進 64 桁)。
- method の enum に並ぶ 3 つの文字列は SearchMethod の as_str から出す。ツールの説明に書いた値と
  実装が受け付ける値が食い違わないためである(must/0023)。
- search と fetch は読むだけなので、annotations は readOnlyHint と idempotentHint が true、
  destructiveHint と openWorldHint が false である。
- add_document の引数は collection(必須。書き込みを許したコレクション名)、name(必須。
  文書名。拡張子なし)、text(必須。空でない本文)、media(省略可。"markdown" か "text"。
  省略時は markdown)。転送する形は `PUT /v1/collections/{c}/documents/{name}.md`(text なら
  `.txt`)に本文をそのまま送り、直接開く形は同じ本体 put_document を呼ぶ。種別の判定は
  取り込みの拡張子の表(node/src/ingest.rs の media_for_extension)が持ち、mcp.rs の
  media → 拡張子の表 ADD_DOCUMENT_MEDIA はその逆引きで、両者が噛み合うことを単体試験
  add_document_media_round_trips_through_the_extension_table が確かめる(should/0135)。
  html と pdf は受けない: テキスト知識を入れる口であって、原本を運ぶ口ではない(URL の
  原本は fetch_url が取る)。
- name の規則(document_name_error): 空でなく、`/ ? # %` と空白・制御文字を含まず、`.` で
  始まらず、URL から導く名前と同じ上限(node/src/fetch.rs の MAX_NAME_CHARS)以内。拡張子の
  付いた名前(memo.md)は断る(PUT documents が拡張子を足すので二重になる。判定は同じ
  拡張子の表に問う)。ref 名の末尾になり、転送する形では URL の道にそのまま載るので、道を
  壊す字と階層を作る `/` を断つ。日本語の 1 語は通る。
- fetch_url の引数は collection(必須)、url(必須。http か https)、name(省略可。規則は
  add_document と同じ。省けば URL から導く)。転送する形は `POST /v1/collections/{c}/fetch`
  に同じ JSON を送り、直接開く形は同じ本体 fetch_into を呼ぶ。url が取りに行ける形かは
  node/src/fetch.rs の validate_url に問い、外れていれば取りに行く前に要求の誤り(-32602)で
  返す(serve も同じ判定で 400 を返すが、要求の組み立ての誤りとして先に言う)。
- 書くツールの説明文には、いつ使うか(利用者が『覚えておいて』『保存して』『この URL を取り
  込んで』と言ったとき)といつ使わないか(仕様書や原本のコレクションには書かない・頼まれて
  いないものを残さない・読むだけなら取り込まない)と、書ける先の一覧をそのまま書く。模型が
  読むのは説明文だけであり、断られる先を試させない。
- add_document の annotations は readOnlyHint false、destructiveHint false(上書きは起こるが
  前版は残り、何も消さない)、idempotentHint true(同じ内容は同じ ID に落ち、繰り返しても
  状態が変わらない)、openWorldHint false。fetch_url は idempotentHint false(取るたびに相手の
  内容が変わりうる)で openWorldHint true(網に出る)である。

## search の応答(出典の描き方)

応答は生の JSON ではなく、LLM がそのまま読む整形テキストである。この層の役目はエージェントが
出典付きで答えられるようにすることであり、出典の読めない応答は用を成さない。

```
検索: 世代の整合(方式 bm25、得点の意味 bm25、1 件)

1. notes/search_ja 位置 1
   見出し: 分散設計 > 世代の整合
   取得日時: 2026-08-16T18:22:37Z
   チャンク ID: s256:1fe778dbd7f9ec4a923cbce2b7f75892d2e23ab4cbb0a1276849d7a2aa7148ab
   得点: 7.6674
   抜粋: 転置索引は導出データであり、世代の整合はオブジェクト数と署名者ごとの最終列番号で確かめる。
   原本(文書全体): /v1/objects/s256:1fe778db…/rendition/source

全文が要るときは fetch ツールにチャンク ID を渡す。
```

- 各件の 1 行目は「<コレクション名>/<文書名> 位置 <添字>」で、文書名は ref パスから
  `collections/<コレクション名>/` を除いた残り、位置は chunks 列の添字である。PDF のチャンクは
  文書名の後ろに `p.<ページ>` が付く。見出しの行はその下で、入れ子の見出しを ` > ` でつなぐ。
  見出しを持たないチャンク(PDF がそうである)は空欄のまま黙らず「(なし)」と書く。
- 「原本(文書全体)」の行は、検索の応答の source_url
  ([docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580))をそのまま写したものである。
  抜粋の周りや紙面そのものが要るときの道で、根は問い合わせ先の serve である(相対の道)。
  serve が言わなかった件には行を出さない。
- 取得日時は引用の at、すなわちその版を見えに置いた署名付き ref レコードの時刻(SPEC §4.4)である。
  その版がこのノードに取り込まれた時刻であって、原典が書かれた時刻ではない。UTC で描く
  (純関数 format_unix_time。外部クレートを持たない(must/0008)ので暦の計算は自前で、
  グレゴリオ暦の 400 年周期を使う閉じた式である)。
- 一致が無ければ「一致なし。」と書く。コレクションで絞ったときは条件を見出し行に書き戻すので、
  空振りが絞り込みのせいかどうかが読み手に分かる。
- 要求した方式で答えられなかったときは「劣化: <方式> で答えた(<理由>)」の行が入る。同じことを
  標準エラーにも書く(黙って劣化しない。should/0128)。劣化の条件は REST と同一である
  ([docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580))。融合を求めて BM25 が
  1 件も一致しなかったとき(順位が埋め込み単独と同じになるとき)も、この行で言う。
- 低情報チャンクを落としたときは「低情報チャンク <n> 件を応答から落とした(…。残すには
  include_low_information: true)」の行が入る。件数と戻し方の両方を書くのは、読み手がモデルで
  あり、次の手を選べるようにするためである。同じことを標準エラーにも書く。
- 末尾の案内は定数 FETCH_HINT から出す。ツールの説明と同じ事実なので文言は 1 箇所が持つ
  (must/0023)。
- 得点は同じ応答の中の順位付けにだけ意味があり、応答をまたいだ比較や絶対値の閾値には使えない。
  同じことを search ツールの説明にも書いて、モデルが得点を絶対値として扱わないようにしている。

## add_document / fetch_url の応答

どちらも 1 行で、何が入ったかと、新規か上書きかを言う。上書きなら前版の doc_rev を添える
(消していないことが読める)。同じ内容の再実行は「変わらず」で、新規オブジェクトは 0 である。
ID は先頭 8 桁に短縮する(全文が要るときは REST で引ける)。

```
入れた: notes/hpet-memo(新規。doc_rev s256:4e40f68c…、新規オブジェクト 3)。検索に出る。
入れた: notes/hpet-memo(上書き。前版 s256:4e40f68c…。doc_rev s256:9b12c0aa…、新規オブジェクト 2)。検索に出る。
入れた: notes/hpet-memo(変わらず(同じ内容が既にある)。doc_rev s256:4e40f68c…、新規オブジェクト 0)。検索に出る。
取り込んだ: web/page(html、final_url http://127.0.0.1:34795/page.html、新規。doc_rev s256:7bea4dee…、新規オブジェクト 3、落とした外部依存 3 件(images 1・scripts 2))。検索に出る。
```

- 新規・上書き・変わらずの区別は、取り込みの応答の ref_updated と previous
  ([docs/design/INGEST.md](#47d69a3e-c39a-4e76-9814-e9c24240293b) の API)から読む。判断は
  ingest_document が持ち、この層は写すだけである。
- fetch_url は取り込んだ名前(name を省いたときは URL から導いた名前)・種別・転送後の URL を
  言い、HTML なら自足化で落とした外部依存の数(dropped)を種類ごとに添える。0 件の種類は
  書かない。
- 「検索に出る」は事実である: 索引は世代で作り直されるので、次の search から出る(転送する形
  でも serve の索引が同じ世代で動く)。

## fetch の応答

REST 側は GET /v1/objects/{id} のままで、そちらは生のオブジェクトのバイト列だけを返す。MCP の
fetch は同じ store の呼び出し(fetch_object)に出典を添える薄い層である。出典は検索と同じ索引から
組むので、search が示した出典と fetch が示す出典は一致する。転送する形は全文と出典を別々に取る:
バイト列は GET /v1/objects/{id}、出典は GET /v1/objects/{id}/citation である
([docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580))。バイト列がチャンクなのか・
チャンクでない c1 オブジェクトなのか・テキストでないのかの見分けは classify_object の 1 箇所に
あり、ストアから読んでも REST から読んでも同じ答えになる(should/0135)。

```
チャンク s256:1fe778dbd7f9ec4a923cbce2b7f75892d2e23ab4cbb0a1276849d7a2aa7148ab の全文
出典: notes/search_ja 位置 1 / 見出し: 分散設計 > 世代の整合 / 取得日時: 2026-08-16T18:22:37Z
---
転置索引は導出データであり、世代の整合はオブジェクト数と署名者ごとの最終列番号で確かめる。
```

- 引用は見え(collections/ 配下の現行 doc_rev)にあるチャンクだけが持つ。ID で取れても見えに無い
  チャンク(旧版など)は、出典をでっち上げずに「見えの索引に無い」と言う。
- チャンクでない c1 オブジェクト(doc_rev・注釈など)は、正規形のまま示す。
- テキストでないバイト列(PDF の原文 blob など)は全文を示せないので、大きさだけを言って
  isError で返す。本文の要るチャンクは search が返す ID で取る。
- ローカルに無い ID は isError で「このDBノードは持っていない」と言う。持っていないことは
  ローカルな事実であって、不存在の言明ではない(SPEC §7.2/§10)。

## 誤りの区分

| 区分 | 返し方 | 当たるもの |
|---|---|---|
| 解析できない JSON | error -32700(id は null) | 壊れた行、小数を含む要求 |
| 封筒の誤り | error -32600 | jsonrpc が "2.0" でない、method が無い |
| 知らないメソッド | error -32601 | resources/list など未実装のメソッド |
| 要求の組み立ての誤り | error -32602 | params が無い、知らないツール名、query が無い、top_k が範囲外、method が 3 つ以外、id の形が不正、`--writable` の無い mcp で書くツールを呼ぶ、collection が無い、name が規則に外れる、text が空、media が 2 つ以外、url が http/https でない |
| ツール実行の失敗 | result の isError: true | ストアを読めない、走っている serve に届かない、保持していない ID、テキストでない blob、許していないコレクションへの書き込み、書き込みの口の 4xx/5xx(取れない URL の 502、取り込めない種別の 415、curl や pdftotext の無い 503 など。理由は serve の本文のまま) |

- 分ける基準は「誰が直せるか」である。プロトコルの誤りは要求の組み立てが誤っている話で、モデル
  ではなく呼び出し側が直す。ツール実行の失敗はモデルが読んで次の手を選べる話なので、結果として
  返す。許していないコレクションへの書き込みが後者なのは、応答が「どこなら書けるか」を言い、
  モデルがそこへ入れ直せるからである。
- 書き込みの失敗も標準エラー(と mcp.log)に残る(応答を読まない運用者にも見えるように)。
- 解析できない行が来ても接続は終わらない。次の要求はそのまま通る。

## 意図的に持たないもの

- resources / prompts / logging の capability。実装していないものを capabilities に載せない。
- tools/list のページ送り。ツールは多くて 4 本なので nextCursor を載せない。
- 消す口。tombstone や gc を模型から引かせない(消せる口は CLI と REST にある)。
- outputSchema と structuredContent。応答は整形テキストだけである。
- JSON-RPC のバッチ。2025-06-18 で仕様から削除されている。

## 書き込みを許す(`--writable`)

既定は読むだけである。`--writable <コレクション名>` を与えたコレクションにだけ、add_document と
fetch_url で書ける。指定は繰り返せる(`--writable notes --writable web`)。

- 1 つも与えなければ、書くツールは tools/list に載らない(search と fetch の 2 本のまま)。
  書けない相手に書くツールを見せると、模型は試して断られるだけである。載せていないツールを
  呼ばれたら、要求の誤り(-32602)で「--writable が 1 つも無いので読むだけ」と言う。
- 与えたコレクション以外への書き込みは、ツールの失敗(isError)で
  「<c> は書き込みを許していない(--writable で許すのは: notes, web)」と断る。書ける先を
  言うのは、模型がそこへ入れ直せるようにするためである。断った書き込みはストアに何も残さない
  (統合試験 add_document_refuses_a_collection_that_is_not_writable_without_touching_the_store)。
- 許した先は、書くツールの説明文と initialize の instructions にも並ぶ。模型が読むのはそこ
  だけである。
- コレクション名は空でなく `/` を含まない 1 語で、外れていれば起動時に exit 2 で断る
  (黙って捨てると、許したつもりの先に書けない)。viewer は同じ読み手を共有するが書く口を
  持たないので、`--writable` を受けたら断る(must/0022 の同型)。
- 自己置換(下の節)は引数ごと新しいイメージへ渡すので、差し替え後も同じ先に書ける。
- 直接開く形は起動から終了まで排他ロックを持つので、書き込みもそのロックの下で行う。転送する
  形の書き込みは serve が行う(curl と pdftotext を待つあいだロックを放す規律は serve のもの
  で、この層は関与しない)。
- [docs/design/RENDITION.md](#6046eeca-1d95-4d47-87da-13f86c7710dc) の「MCP は写しを作らせ
  ない」はそのままである。写し(ページ画像・ページ PDF)は消せない導出データで、生成の引き金は
  ビューワだけが引く。ここで開けるのは利用者の指示で文書を足す口であり、入れたものは普通の
  文書として ref を tombstone すれば見えから消え、gc が回収する
  ([docs/design/GC.md](#9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。模型が勝手に増やさない
  ように、説明文で「利用者が頼んだときだけ」と言い、書ける先を限る。

## Claude Code への登録

走っている serve へ転送する形(勧める形。常駐したまま取り込みと埋め込みが回せる):

```
claude mcp add --transport stdio uniqnode -- /path/to/uniqnode mcp /path/to/data \
  --serve-url http://127.0.0.1:7440
```

エージェントから文書を足させるなら、許すコレクションを添える(既定は読むだけ):

```
claude mcp add --transport stdio uniqnode -- /path/to/uniqnode mcp /path/to/data \
  --serve-url http://127.0.0.1:7440 --writable notes --writable web
```

転送先の serve は別に起こしておく。埋め込みと順位の取り直しを装備するのはこちらである:

```
uniqnode serve /path/to/data 127.0.0.1:7440 --embed http://127.0.0.1:8083/v1/embeddings \
  --rerank http://127.0.0.1:8084/v1/rerank
```

serve を走らせないときは、ストアを直接開く形で登録する(この形に限り、埋め込みと順位の
取り直しの引数を serve と同じ意味でそのまま後ろに足せる):

```
claude mcp add --transport stdio uniqnode -- /path/to/uniqnode mcp /path/to/data
claude mcp add --transport stdio uniqnode -- /path/to/uniqnode mcp /path/to/data \
  --embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank
```

リポジトリに置いて共有する `.mcp.json` の形:

```json
{
  "mcpServers": {
    "uniqnode": {
      "type": "stdio",
      "command": "/path/to/uniqnode",
      "args": ["mcp", "/path/to/data", "--serve-url", "http://127.0.0.1:7440"]
    }
  }
}
```

- 実行ファイルもデータディレクトリも絶対パスで書く。サーバの作業ディレクトリを決めるのは登録側
  であり、相対パスが何を指すかはこちらから保証できない。
- ツールはモデルからは `mcp__uniqnode__search` と `mcp__uniqnode__fetch`(`--writable` が
  あれば `mcp__uniqnode__add_document` と `mcp__uniqnode__fetch_url` も)として見える。前半の
  uniqnode は登録した名前なので、別の名前で登録すればツール名もその名前になる。
- 接続を確かめる最短の道は、標準入力に initialize を 1 行流して protocolVersion が返ることを見る
  ことである(標準出力に JSON-RPC 以外が出ていないことも同時に分かる)。

## 自己置換(実行ファイルが更新されたとき)

Claude Code に登録した stdio サーバは子プロセスとして起こされ、公式の説明どおり自動では繋ぎ
直されない(Stdio servers are local processes and are not reconnected automatically)。実行
ファイルを作り直しても走り続けるのは古いイメージのままで、セッションを再起動しない限り新しい
実装は効かない。そこでサーバ自身が更新を見つけ、自分を exec で差し替える。execve はプロセス
イメージを入れ替えるがファイル記述子は保つので、相手が握るパイプの反対側と PID は変わらず、
差し替えはクライアントから見えない。

- 検出は、起動時に控えた実行ファイル(std::env::current_exe)の更新時刻・大きさ・inode の比較で
  ある。パスを起動時に控えるのは、置き換えられた後の /proc/self/exe が "(deleted)" 付きの読めない
  道になるためである。
- 差し替えの位置は「応答を書き終えた直後、次の読み込みの前」だけである。メッセージの処理の途中で
  入れ替えると、読んだ要求が旧イメージと共に消える。
- 先読みバッファが空であることを確かめてから exec する。BufReader が次のメッセージまで読んで
  いれば、そのバイト列は旧イメージと共に消えるので、差し替えを次の機会に回す。標準入力の
  BufReader を自分で持つのはこの検査のためで、容量は std::io::Stdin の内部バッファ(8KiB)より
  大きく採る(見えない場所にバイトが溜まらないため)。
- exec の前に、新しいイメージを子プロセスとして起こして健全さを確かめる(`uniqnode selfcheck`。
  ツールの記述を実際に組み立てて 1 行を標準出力に書く)。見るのは終了コード 0 と標準出力の印
  (定数 SELF_CHECK_MARKER)の両方である。終了コードだけを見ると、たまたま 0 で終わる別の実行
  ファイルを健全とみなす。落ちたら差し替えず、理由を標準エラーに出して旧イメージのまま動き
  続ける。ここで死ぬと、避けたかったセッション再起動をこちらから招くことになる。
- 検査に落ちた姿は控え直す。同じ実行ファイルを応答のたびに検査し直さず、次にまた変わったときに
  試す。子プロセスの待ちは 10 秒で打ち切り、終わらないイメージも健全とはみなさない。
- 新しいイメージは initialize を受けた事実を忘れている(Claude Code は再送しない)。交渉した
  プロトコル版と相手の名乗りを環境変数 UNIQNODE_MCP_HANDSHAKE_PROTOCOL /
  UNIQNODE_MCP_HANDSHAKE_CLIENT で引き継ぎ、新しいイメージは起動時にそれを標準エラーに残す。
  このアダプタは initialize の前でも tools/list と tools/call に答えるので、忘れてツールが
  止まることはない。引き継ぐのは、記録が切れないことと、次の差し替えへ同じ内容を渡すためである。
- ストアの排他ロックは CLOEXEC 付きの FD なので exec で解放され、新しいイメージが取り直す
  (取り直しの隙間は acquire_lock の 250ms 有界再試行が吸収する。
  docs/analysis/20260816-lock-inheritance-race.md)。転送する形ではそもそもロックを持たない。
- 差し替えたことは標準エラーに残す(黙って入れ替えない)。
- 実測 2026-08-17: 転送する形で常駐中に cargo build で実行ファイルを置き換えると(inode
  99526521 → 99525865)、次の応答の直後に exec が起き、PID は 4090627 のまま同じ標準入出力で
  検索が通り続けた。標準エラーには「実行ファイルが更新された。自分を exec で差し替える」に
  続いて、新しいイメージの「ハンドシェイク済みとして起動した(相手の protocol 2025-11-25、
  client claude-code/2.1.232)」が並ぶ。壊れたイメージ(自己検査の命令名を潰した複製)を置いた
  ときは「新しい実行ファイルの自己検査に落ちた。差し替えず、旧イメージのまま続ける:
  … selfcheck が exit status: 2 で終わった」を出して差し替えず、以後の応答も旧イメージが返した。

## 運用上の制約

- 標準エラーは登録した相手の中で消えるので、何が起きたかを後から読むときは
  `<data_dir>/logs/mcp.log` を見る。既定で残り、UTC の時刻と pid が行頭に付く
  ([docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004))。会話ごとに別のプロセスが
  立つので、同じファイルに複数の pid の行が混じる。
- 転送する形はストアを開かないので、常駐したまま CLI を回せる。実測 2026-08-17:
  `uniqnode mcp <dir> --serve-url http://127.0.0.1:7440` の常駐中に serve を止め、同じ
  ディレクトリへ ingest(chunks=3)・embed(5 チャンクを bge-m3 で。終了コード 0)・status を
  走らせるとすべて通り、serve を起こし直すと同じ MCP プロセスが新しい文書を hybrid で検索した。
  MCP のプロセスは一度も再起動していない。改善のループはこの形で回す。
- ストアを直接開く形では、従来どおりストアを二重に開けない。MCP サーバは起動時にストアの排他ロックを
  取り(抽象名前空間の Unix ソケット。名前はデータディレクトリの正規化パスのハッシュ)、常駐して
  いるあいだ、ストアを開く CLI の命令はすべて断られる。実測 2026-08-17: `uniqnode mcp <dir>` の
  常駐中に ingest・embed・status を走らせると、どれも「<dir> は別プロセスが開いている」を標準
  エラーに出して終了コード 1 で終わる。MCP を終わらせた直後に同じ ingest は成功する。serve と
  同じ制約である。
- したがってこの形を使うときは、文書の取り込みとコーパスの埋め込みは MCP サーバを止めてから
  走らせる。標準入力を閉じればサーバは終わるので、エージェント側の接続を切れば足りる。
- 最初の search は索引の構築ぶんだけ待つ(実データ規模で約 9 秒。
  [docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580) の「既知の癖」)。以後は世代が
  ずれるまでキャッシュ済みの索引で応える。転送する形では、その待ちも索引も serve の側にある。

# AGENT_DOOR — serve の読み口(第 2 の TcpListener・許可表・チャンク限定・読める集合・許したコレクションへの PUT・束縛の再試行)

<a id="02f79aec-2f12-41e6-bede-1557d4719e4d"></a>

読み手は、別の機械で走る LLM エージェントに uniqnode を引かせ、書かせたい者と、
node/src/agent_door.rs を読む者。この文書は読み口の現在の実装を記述する。読み口が委ねる先の API
(POST /v1/search・GET /v1/objects/{id}・PUT /v1/collections/{c}/documents/{name} など)の中身は
[docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580)・
[docs/design/INGEST.md](#47d69a3e-c39a-4e76-9814-e9c24240293b) と SPEC §10 にあり、記録の
置き場は [docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004) にある。ここが決めるのは、
第 2 の口に何を通し、何を断り、どう束縛し、何を残すかだけである。

## 目的

主の口(serve の `<listen>`。既定 127.0.0.1:7440)は admin・sync・pins・fetch・PUT を持つ
管理の口で、機械の外に出さない。だが別の機械のエージェントに知識を引かせたいとき、必要なのは
「検索して、当たったチャンクの全文と出典を読む」であり、覚えさせたいときに足すのは「許した
コレクションに文書を 1 件入れる」だけである。読み口はそのためだけの第 2 の TcpListener で、
`uniqnode serve <dir> <listen> --listen-agent <addr> [--agent-writable <c>]...
[--agent-collections <c>]...` で束縛する。
`--listen-agent` を与えなければ第 2 の口は存在しない。`--agent-writable` を 1 つも与えなければ
読み口は読むだけである(第 1 段)。`--agent-collections` を 1 つも与えなければ、読み口からは
全コレクションが読める(既定は絞らない)。

読み口は門でしかない。(method, path) の許可表を 1 つ持ち、表に無い要求は断り、表にある要求だけを
主の口と同じ `api::handle` に委ねる。何を返すか(検索の方式・劣化・引用の組み立て・取り込み)の
判断は node/src/api.rs の 1 箇所にあり、読み口はそれを 2 度書かない(should/0135)。

境界は 3 つで、この実装が持つのは 3 つ目だけである: 束縛先を WireGuard の口(10.10.128.1
のような、その網からしか届かないアドレス)に限ること、firewall が相手の機械からの接続だけを
許すこと、そして許可表。認証と TLS は持たない。

## 許可表

node/src/agent_door.rs の `admit` の match がこの表そのものである。行の順も同じ。`admit` は
`--agent-writable` と `--agent-collections` の集合を引数に取り、PUT の行は前者で、search が
名指しした `collection` は後者で決まる(どちらも判断はこの 1 箇所)。

| method | path | 委ねる先での意味 | 読み口だけの扱い |
|---|---|---|---|
| GET | /healthz | 生存 | |
| GET | /v1/status | node_id・件数・容量 | |
| POST | /v1/search | 検索 | 本文に `peers` 鍵があれば 400。`collection` が読める集合の外なら 403、省略なら集合が share になる |
| GET | /v1/objects/{id} | オブジェクトの生バイト列 | id は `c1::is_object_id` の形に限る。応答が kind:"chunk" でなければ 403。出典のコレクションが読める集合の外なら 403 |
| GET | /v1/objects/{id}/citation | 出典 | id は `c1::is_object_id` の形に限る。出典のコレクションが読める集合の外なら 403 |
| GET | /v1/collections | コレクションの一覧 | 読める集合にあるものだけ |
| PUT | /v1/collections/{c}/documents/{name} | 文書 1 件の取り込み(第 2 段) | c が `--agent-writable` の集合にあるときだけ。集合に無い c は許した一覧を言って 403。集合が空なら表に無い |

表に無いものはすべて 403 である: admin(gc・shutdown)・sync・pins・peers・refs・closure・
referrers・rendition・fetch・query・POST objects、集合に無いコレクションへの PUT、そして同じ
path の別の method(`POST /v1/status` など)。`fetch` は `--agent-writable` でコレクションを
許していても通さない: 読み口の向こうのコンテナが網に出る道になるからで、URL の取り込みは
操作者の仕事のままである。

## 断りの本文

- 表に無い要求: 403 `{"error":"agent door: <METHOD> <path> は許可されていない"}`。method と
  path は要求の字面をそのまま言う(何を試して断られたかが、応答だけで分かる)。
  `--agent-writable` が 1 つも無いときの PUT もこれである。
- 許していないコレクションへの PUT: 403 `{"error":"agent door: コレクション <c> は書けない
  (--agent-writable で許したのは <c1>, <c2>)"}`。一覧は与えられた順、`, ` 区切り。
- 読める集合の外を読もうとしたとき: 403 `{"error":"agent door: コレクション <c> は読めない
  (--agent-collections で許したのは <c1>, <c2>)"}`。search の `collection` でも、objects と
  citation の出典でも同じ文言である(門から見ればどちらも同じ「集合の外を読もうとした」で
  ある)。文言を組むのは `agent_door::unreadable_refusal` の 1 箇所で、install の確認も
  そこから期待値を作る(must/0023)。
- peers 付きの search: 400 `{"error":"agent door: peers は使えない"}`。散布は他のDBノードへ問いの
  文を配る行為で、読み口の向こうのエージェントに選ばせない。鍵があれば値によらず断る
  (`false` や `null` を主の口と同じく「散布しない」と読み替えると、その解釈が変わったとき
  読み口だけ古い答えを出す)。本文が JSON として壊れているときは断らず委ね、主の口と同じ
  400 が返る。
- チャンクでないオブジェクト: 403 `{"error":"agent door: チャンクでないオブジェクトは許可されて
  いない"}`。
- 記録に写せない申告のヘッダ: 400 `{"error":"agent door: X-Uniqnode-Task の値は
  [A-Za-z0-9_.:/-] の 1..=64 字"}`(X-Uniqnode-Agent も同文)。下の「誰が要求したかの申告」。

## 読める集合(`--agent-collections`)

`--agent-collections <c>`(複数可。名前の形は `--agent-writable` と同じ 1 語で、検査も同じ
1 箇所。`--listen-agent` が無いのに与えれば serve は 2 で終わる)を与えると、読み口から
読めるコレクションがその集合に限られる。1 つも与えなければ、読み口は今までどおり全
コレクションを読める(既定は変えない。絞るのは、絞ると決めた運用だけである)。

判断は 1 つしかない: 集合を `search::CollectionScope` に直す `agent_door::readable_scope`
(空なら `All`、そうでなければ `Only(集合)`)である。効く場所は 4 つで、どれもその 1 つを
呼ぶ(should/0135):

1. `POST /v1/search`: 要求が `collection` を名指ししていて集合の外なら、索引を引く前に 403。
   省略なら、集合を応答側の共有ポリシー(share)として `api::handle_search_within` に渡し、
   要求の絞り込みとの交差は `run_search` の既存の 1 箇所が取る(ピアの QUERY に答える道と
   同じ仕組みである)。名指しを 403 にするのは、交差に任せると集合の外を指した検索が
   「0 件 + peers.json の share を語る degraded」になり、読み手には一致が無いのか見て
   いないのかが読めないからである。
2. `GET /v1/collections`: 集合にあるコレクションだけを数えて返す。指しても 403 になる名前を
   一覧に並べない(一覧は「次に何を collection に指せるか」の表である)。
3. `GET /v1/objects/{id}`: そのチャンクの出典のコレクションが集合の外なら 403。id さえ
   知っていれば読めてしまう道を、検索と同じ集合で塞ぐ(チャンク限定と同じ理由・同じ場所)。
4. `GET /v1/objects/{id}/citation`: 同じ判定。

出典が引けない id(見えに無い旧版、持っていない ID、チャンクでないもの)は今までどおりで
ある: 404 か、チャンクでないことによる 403 が返る。出典を引くのは集合を絞っているときだけ
で、絞っていない読み口に索引の引き当てを足さない。

install は `--agent-collections <c>` を drop-in に
`Environment="UNIQNODE_AGENT_COLLECTIONS=<c1> <c2>"` と ExecStart= 末尾の
`--agent-collections <c>` の並びとして書き(`--agent-writable` と全く同じ流儀)、確認に
「許していない名を collection に指した検索が 403 の文言で断られる」を足す(読むだけの確認で、
本番のデータには触らない)。

## 書く口(第 2 段)と出所の meta

`--agent-writable <c>`(複数可。名前は空でなく / と空白を含まない 1 語。`--listen-agent` が
無いのに与えれば serve は 2 で終わる)で許したコレクションへの
`PUT /v1/collections/{c}/documents/{name}` は、主の口と同じ `api::put_document` に委ねられる。
本文・拡張子・応答(doc_rev・new_objects・ref_updated・previous)は主の口の PUT と同じで、
INGEST の「CLI と API」節にある。門はコレクション名だけを見る(path の分け方は
`api::document_path` の 1 箇所)。

出所は PUT の query で渡す: `?meta.<key>=<value>`(複数可、`&` 区切り)。key は
`[a-z0-9_]{1,32}`、value は %XX をデコードして 1..=200 字(`+` は空白に読み替えない)。
写る先は doc_rev.meta で、取り込みが決める name・media・extractor と並ぶ(その 3 つは query で
名乗れず 400)。同じ鍵の繰り返し、`meta.` 以外の鍵、壊れた %、UTF-8 にならない値も 400 で
理由を言い、何も書かれない。読み方は `api::parse_meta_query` の 1 箇所で、主の口の PUT でも
同じに通る(読み口だけの機能ではない)。lamalium は `?meta.agent=<id>&meta.task=<id>` を
付ける想定である。

- meta は doc_rev にだけ写る。検索の応答の citation と `GET /v1/objects/{id}/citation` の形は
  変えない(collection・document・position・page・breadcrumbs・at のまま)。出所を読むには主の口で
  `GET /v1/objects/{doc_rev}` を引く。読み口の objects はチャンクしか通さないので、読み口からは
  doc_rev(と meta)は読めない。
- 同じ本文の再 PUT は meta が違っても no-op である(同一内容の判定は source と chunks 列で、
  meta を見ない。URL からの取り込みの fetched_at と同じ扱い)。応答の ref_updated が false で
  それを言い、doc_rev も meta も最初のまま残る。
- 応答の検め(`screen`)は PUT の行には掛からない。応答は取り込みの結果の JSON で、blob が
  外へ出る経路ではない。

install は `--agent-writable <c>` を drop-in に `Environment="UNIQNODE_AGENT_WRITABLE=<c1> <c2>"`
と ExecStart= 末尾の `--agent-writable <c>` の並びとして書き、確認に「許したコレクションへの
拡張子の無い PUT が門を越えて 400 で止まり(何も書かない)、許していないコレクションへの PUT が
403 の文言で断られる」を足す([docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2))。

## 誰が要求したかの申告(X-Uniqnode-Task・X-Uniqnode-Agent)

読み口を通る要求は、2 つの要求ヘッダで「どのタスクの、どのエージェントが要求したか」を
名乗れる。読む口にも書く口にも同じに掛かる(読み口を通る要求すべて)。

- `X-Uniqnode-Task`(記録の行の `task=`)と `X-Uniqnode-Agent`(同 `agent=`)。値の字種は
  `[A-Za-z0-9_.:/-]` の 1..=64 字である。`/` を許すのは lamalium のタスク id が `tasks/chat` の
  形だからで、名の 3 つ組(応答に出す字面・引くときの小文字の名・行の欄の名)は
  `agent_door::RECORDED_HEADERS` の 1 箇所にある(must/0023)。
- ヘッダが無いのは正常である(何も足さない)。字種の外・65 字以上・空の値は 400 で理由を言い、
  その要求は許可表にも api::handle にも届かない(何も起きない)。記録に書けない値を受け取った
  まま通すと、その 1 行が誰の要求だったのかを言えなくなるからである(must/0022)。
- 判断には一切入らない。許可表も検索も読める集合もこの値を見ないので、同じ要求はヘッダの
  有無によらず同じ答えを返す。効くのは記録の行の末尾だけである。
- 書く口の `?meta.task=`(と `?meta.agent=`)とは別物である: ヘッダは「誰がこの 1 本を要求
  したか」の記録で、応答の後には serve の log にしか残らない。meta は「誰がこの文書を書いたか」
  の不変の言明で、doc_rev に写って文書と共に残る(同じ値を両方に載せるかどうかは呼び手の
  自由である)。

## objects をチャンクに限る理由

`GET /v1/objects/{id}` は主の口では何でも返す: チャンク、doc_rev、注釈、そして PDF の原文の
blob。LLM は幻覚の id や、検索結果からコピーする途中で切れた id も渡す。そのまま流すと、
数 MB のバイナリが観測へ流れる経路が境界にできる。だから読み口は、id の形が正しくないものを
表に無い要求として 403 で断り、形が正しくて 200 が返っても、応答が kind:"chunk" のオブジェクト
でなければ 403 に差し替える。見分けは `api::classify_object` の 1 箇所で、MCP の fetch が
ストアから直に読んだときと同じ判断である。持っていない id は主の口と同じ 404 で、門は在否を
隠さない(404 はローカルな事実であって不存在の言明ではない。SPEC §7.2)。

## 束縛と再試行

読み口のアドレスは WireGuard の口で、wg1 が上がる前は存在しない(bind が EADDRNOTAVAIL で
落ちる)。その前に serve が起きるのは正常な順序なので、読み口の bind 失敗で主の口を殺さない。

- 主の口は従来どおり「ストアを開く → 装備する → 束縛する → `listening on <addr>`」の順で
  先に上がる。標準出力のこの 1 行の取り決めは変えない。
- 読み口は裏のスレッドで束縛する。失敗は log に残し(must/0022)、
  `agent_door::BIND_RETRY_INTERVAL`(5 秒)ごとに取りに行き続ける。同じ理由が続くあいだは
  同じ行を積まず、理由が変われば記す。
- 束縛できたら標準出力に `listening on <addr> (agent door)` の 1 行を出す(末尾の印は
  `agent_door::LISTENING_LINE_SUFFIX`。主の口の行と見分けるためのもので、起動スクリプトと
  試験はこの印で 2 本目の行を選ぶ)。同じ行を log にも残す。標準出力が閉じていても読み口は
  止めない(println! の panic ではなく log に倒す)。
- `--listen-agent` は serve だけの引数である。mcp と viewer は読み手を共有しているので字面は
  通るが、効かせる先が無いので黙って捨てずに 2 で終わる(viewer の `--writable` と同じ扱い)。
  `--agent-writable` も同じで、加えて `--listen-agent` の無い serve に与えても 2 で終わる
  (書く許可は読み口に掛かるもので、読み口が無ければ効かせる先が無い)。
  `--agent-collections` も全く同じ扱いである(読む許可も読み口に掛かる)。

## 記録

読み口への要求は 1 本につき 1 行、serve の log(既定 `<data_dir>/logs/serve.log`。標準エラー
にも同じ行)に残る:

```
<UTC 時刻> [pid N] agent <peer addr> <METHOD> <path> <status> <ms>[ task=<値>][ agent=<値>]
```

通した要求も断った要求も残す。誰が何を試したかは、通した要求と同じだけ読みたい記録である。
行頭の印 `agent` は `agent_door::LOG_MARK` で、試験はこれで行を選ぶ(must/0023)。主の口への
要求はこの行にならない。PUT も同じ 1 行で(path には `?meta.…` の query がそのまま載る)、
誰が何を書いたかは doc_rev.meta と、この行の対で追える。起動時には
`読み口から書けるコレクション: <c1>, <c2>` の 1 行を残す(`--agent-writable` があるときだけ)。
`--agent-collections` があれば `読み口から読めるコレクション: <c1>, <c2>` の 1 行も同じように
残す(絞ったことが、後から記録だけで読める)。末尾の `task=` と `agent=` は要求ヘッダから
写したもので、在るぶんだけ付く(上の「誰が要求したかの申告」)。

## 試験の場所

node/tests/agent_door.rs。実プロセスの serve を `--listen-agent 127.0.0.1:0` で起こし、標準出力の
2 本の `listening on` から両方の口を読んで、生 HTTP/1.1 で当てる(should/0138)。許可表の各行が
通ること(status は主の口と同じ node_id、search、objects のチャンク、citation、collections は
門を通ること)、表に無い 17 本が method と path を言う 403 で断られ PUT がストアに届いて
いないこと、peers 付きの search が 400、blob と c1 オブジェクトが id の形が正しくても 403、
読み口が指定したアドレス(127.0.0.2)にしか束縛せず主の口には許可表が掛からないこと、
`--listen-agent` 無しでは標準出力が主の口の 1 行だけであること、塞がれたポートを指しても
主の口が上がり失敗が log に残りポートが空けば再試行で束縛すること、要求ごとの log の行、
mcp と viewer が引数を断ること。書く口は、`--agent-writable notes` で notes への PUT が 200 に
なり主の口の `GET /v1/objects/{doc_rev}` に meta が写っていること、検索の citation と /citation に
meta が出ないこと、other への PUT が許した一覧を言う 403、fetch は許しても 403、meta の鍵や値の
形が違えば 400 で何も入らないこと、`--agent-writable` だけで `--listen-agent` 無しは 2 で終わる
こと。読める集合は、2 つのコレクションに文書を入れて、許した名を指した検索が返ること、
許していない名を指した検索が文言つきの 403 で断られること(同じ検索は主の口では通る)、
collection を省いた検索の結果と `GET /v1/collections` に集合の外が 1 つも出ないこと、集合の外の
チャンクは ID を知っていても objects と citation が 403(同じ ID が主の口では 200)であること、
`--agent-collections` 無しなら全部読めること、`--listen-agent` 無しの指定と名前の形の誤りが
2 で終わること。申告のヘッダは、付ければ記録の行の末尾に `task=` と `agent=` が出ること
(通した要求にも断った要求にも。片方だけなら片方だけ)、付けなければ行が今までどおりの
5 欄であること、字種の外の値は 400 で理由を言い PUT がストアに届かないこと。
主の口でも同じ query が通ることは node/tests/api.rs にある。各試験の冒頭に、許可表の
どの行を消すとどの assert が落ちるかを記してある(should/0137)。許可表そのものの単体試験は
node/src/agent_door.rs に、query の読み方の単体試験は node/src/api.rs にある。install の
drop-in の文面と「読み口が無ければ断る」は node/tests/install.rs にある。

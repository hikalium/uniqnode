# AGENT_DOOR — serve の読み口(第 2 の TcpListener・許可表・チャンク限定・束縛の再試行)

<a id="02f79aec-2f12-41e6-bede-1557d4719e4d"></a>

読み手は、別の機械で走る LLM エージェントに uniqnode を引かせたい者と、node/src/agent_door.rs を
読む者。この文書は読み口の現在の実装を記述する。読み口が委ねる先の API
(POST /v1/search・GET /v1/objects/{id} など)の中身は
[docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580) と SPEC §10 にあり、記録の
置き場は [docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004) にある。ここが決めるのは、
第 2 の口に何を通し、何を断り、どう束縛し、何を残すかだけである。

## 目的

主の口(serve の `<listen>`。既定 127.0.0.1:7440)は admin・sync・pins・fetch・PUT を持つ
管理の口で、機械の外に出さない。だが別の機械のエージェントに知識を引かせたいとき、必要なのは
「検索して、当たったチャンクの全文と出典を読む」だけである。読み口はそのためだけの第 2 の
TcpListener で、`uniqnode serve <dir> <listen> --listen-agent <addr>` で束縛する。`--listen-agent`
を与えなければ第 2 の口は存在しない。

読み口は門でしかない。(method, path) の許可表を 1 つ持ち、表に無い要求は断り、表にある要求だけを
主の口と同じ `api::handle` に委ねる。何を返すか(検索の方式・劣化・引用の組み立て)の判断は
node/src/api.rs の 1 箇所にあり、読み口はそれを 2 度書かない(should/0135)。

境界は 3 つで、この実装が持つのは 3 つ目だけである: 束縛先を WireGuard の口(10.10.128.1
のような、その網からしか届かないアドレス)に限ること、firewall が相手の機械からの接続だけを
許すこと、そして許可表。認証と TLS は持たない。

## 許可表

node/src/agent_door.rs の `admit` の match がこの表そのものである。行の順も同じ。

| method | path | 委ねる先での意味 | 読み口だけの扱い |
|---|---|---|---|
| GET | /healthz | 生存 | |
| GET | /v1/status | node_id・件数・容量 | |
| POST | /v1/search | 検索 | 本文に `peers` 鍵があれば 400 |
| GET | /v1/objects/{id} | オブジェクトの生バイト列 | id は `c1::is_object_id` の形に限る。応答が kind:"chunk" でなければ 403 |
| GET | /v1/objects/{id}/citation | 出典 | id は `c1::is_object_id` の形に限る |
| GET | /v1/collections | コレクションの一覧 | |

表に無いものはすべて 403 である: admin(gc・shutdown)・sync・pins・peers・refs・closure・
referrers・rendition・fetch・query・PUT・POST objects、そして同じ path の別の method
(`POST /v1/status` など)。`fetch` を通さないのは、読み口の向こうのコンテナが網に出る道に
なるからで、URL の取り込みは操作者の仕事のままである。

## 断りの本文

- 表に無い要求: 403 `{"error":"agent door: <METHOD> <path> は許可されていない"}`。method と
  path は要求の字面をそのまま言う(何を試して断られたかが、応答だけで分かる)。
- peers 付きの search: 400 `{"error":"agent door: peers は使えない"}`。散布は他のDBノードへ問いの
  文を配る行為で、読み口の向こうのエージェントに選ばせない。鍵があれば値によらず断る
  (`false` や `null` を主の口と同じく「散布しない」と読み替えると、その解釈が変わったとき
  読み口だけ古い答えを出す)。本文が JSON として壊れているときは断らず委ね、主の口と同じ
  400 が返る。
- チャンクでないオブジェクト: 403 `{"error":"agent door: チャンクでないオブジェクトは許可されて
  いない"}`。

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

## 記録

読み口への要求は 1 本につき 1 行、serve の log(既定 `<data_dir>/logs/serve.log`。標準エラー
にも同じ行)に残る:

```
<UTC 時刻> [pid N] agent <peer addr> <METHOD> <path> <status> <ms>
```

通した要求も断った要求も残す。誰が何を試したかは、通した要求と同じだけ読みたい記録である。
行頭の印 `agent` は `agent_door::LOG_MARK` で、試験はこれで行を選ぶ(must/0023)。主の口への
要求はこの行にならない。

## 試験の場所

node/tests/agent_door.rs。実プロセスの serve を `--listen-agent 127.0.0.1:0` で起こし、標準出力の
2 本の `listening on` から両方の口を読んで、生 HTTP/1.1 で当てる(should/0138)。許可表の各行が
通ること(status は主の口と同じ node_id、search、objects のチャンク、citation、collections は
門を通ること)、表に無い 17 本が method と path を言う 403 で断られ PUT がストアに届いて
いないこと、peers 付きの search が 400、blob と c1 オブジェクトが id の形が正しくても 403、
読み口が指定したアドレス(127.0.0.2)にしか束縛せず主の口には許可表が掛からないこと、
`--listen-agent` 無しでは標準出力が主の口の 1 行だけであること、塞がれたポートを指しても
主の口が上がり失敗が log に残りポートが空けば再試行で束縛すること、要求ごとの log の行、
mcp と viewer が引数を断ること。各試験の冒頭に、許可表のどの行を消すとどの assert が
落ちるかを記してある(should/0137)。許可表そのものの単体試験は node/src/agent_door.rs にある。

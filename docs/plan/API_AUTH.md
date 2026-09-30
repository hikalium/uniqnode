# API_AUTH: ノードローカル API の信頼の境界を、SPEC §10 と実物で揃える(設計草案)

<a id="abde9b3c-75f8-453b-988e-bfb1e178c771"></a>

版: 第 2 版(2026-10-01)。第 1 版(a9eba65)への Claude のレビュー(高 1〜3・中 4〜6・低 7〜9)を
取り込んだ。次は Codex のレビュー。
出所は lamalium の健全性点検(2026-09-30)の項目 12。SPEC.md §10 は「認証は当面固定トークン」と
言うが、コードにトークンは無い。表にも実装済みの口が載っていない。FEED の F2 は、主の口にだけ
`edit`・`retire`・`allow-shrink`・`rollback` を置いて操作者の歯止めにする設計なので、その前に
「主の口を誰が叩けるか」を決める([docs/plan/FEED.md](#fa8de6f9-59f8-4512-a815-9f41d305db15))。

## 今の事実(2026-10-01 のコードと vega)

| 待ち受け | 本番の束縛 | 認証 | 通すもの |
|---|---|---|---|
| 主の口(serve の `<listen>`) | 127.0.0.1:7440(graph_a・graph_b は 7444・7442) | 無い。送信元・Host・Origin・Content-Type も見ない(http.rs が見るヘッダは connection・content-length・transfer-encoding・expect だけ) | 全部(`/v1/admin/shutdown`・`/v1/admin/gc`・`PUT /v1/refs/…`・`POST /v1/sync`・`POST /v1/collections/{c}/fetch` を含む) |
| 読み口(`--listen-agent`) | 10.10.128.1:7441・7445・7443(wg1) | 無い。送信元は記録に写すだけ | agent_door.rs の許可表: 読む口(chunk だけの objects と citation、読めるコレクションに絞った検索、`peers` は 400)、許したコレクションへの文書の PUT、許したグラフの読み書き。外は 403。守りは許可表・wg1 への束縛・install の firewall の 3 重 |
| viewer(`--viewer-listen`) | 0.0.0.0:7450(本番の命令が渡す。wg1 の 10.10.128.1:7450 も含む) | 無い。firewall も無い | 読むだけの転送だが、許可表は無い: 全 ref の一覧(`GET /v1/refs`)、どんなオブジェクトでも(`GET /v1/objects/{id}`)、全コレクションの検索(`peers` も素通し) |
| MCP | stdio | 起動した利用者 | 主の口への HTTP |

vega には hikalium(uid 1000。serve・viewer・MCP を走らせる)の他に、別の利用者 lamalium
(uid 1001)がいて常駐を走らせている。docker もある(host network のコンテナは 127.0.0.1 に届く)。

`POST /v1/peer/query` だけは、中身の QUERY の署名を検証する(§7.1)。これは認証ではなく、
散布された問いの出所の検証である。

コードには在るが §10 の表に無い口: `POST /v1/admin/gc`、`GET /v1/objects/{id}/citation`、
`GET /v1/objects/{id}/rendition`、`GET /v1/objects/{id}/rendition/{alias}`、`GET /v1/closure/{id}`、
`POST /v1/collections/{c}/fetch`、viewer の `GET /`。表に在ってコードに無い口は無い。

## 脅威(第 1 版で見落としたもの)

1. 同じ機械の別の uid(lamalium、docker のコンテナ)が 127.0.0.1:7440 を叩く。読み口の許可表
   (書けるのは lamalium-notes だけ)は、lamalium のプロセスが vega で走った時点で意味を失う
   (Claude 高 1)。
2. 操作者のブラウザで開いた任意のページが、プリフライトの要らない要求(form、`no-cors` の
   `text/plain` の POST)で主の口と viewer を叩く。shutdown・gc・fetch(任意の URL を取りに
   行かせる)・sync・objects・pins が通る。DNS rebinding なら同じ生成元になり、読み出しも PUT も
   通る(Claude 高 2)。
3. viewer が読み口より緩い網越しの口になっている。グラフ層、feeds/ の ref、今後足すコレクション
   (lamalium)が、LAN と wg1 から読める(Claude 高 3・低 9)。

## 決めること(案)

トークンは入れない。信頼の境界を「誰の uid か」「どの Host・生成元か」「どの許可表か」で置く。

### 1. 主の口: hikalium と root のプロセスだけ、ブラウザは通さない

- 束縛はループバックの IP リテラルだけ(`127.0.0.0/8` と `[::1]`)。`localhost` のような名前・
  unspecified(`0.0.0.0`・`[::]`)・IPv4 射影(`[::ffff:127.0.0.1]`)は断る(Claude 低 7)。
  serve 自身が起動時に断り、明示のフラグ `--listen-nonloopback` があるときだけ通して、起動の
  たびに警告を記録する(Claude 中 5)。install は同じ判定を据え付けの前に早く当てる。
- uid の制限: install は、読み口の firewall と同じ表 `inet uniqnode_<インスタンス>` の output の
  chain に `oif lo tcp dport <主の口> meta skuid != { <常駐の利用者>, 0 } counter reject` を置く
  (serve の ExecStartPre=+nft -f で起動のたびに入る。`--firewall-allow` が無くても、`--system`
  なら常に入れる)。root を許すのは、root はストアのファイルを直接読めるので守る意味が無いから
  である。vega では hikalium と root だけが主の口に届き、lamalium の uid と、root 以外で走る
  コンテナは届かない(Claude 高 1 の直し方 (a))。UNIX ソケットへの移行(直し方 (b))は、
  MCP・viewer・install の確認・テストの全部の相手先を替える大きな変更なので採らない。
- ブラウザの遮断: http.rs に共通の門を置き、主の口と viewer の両方に効かせる(Claude 高 2)。
  - `Host` が束縛先の字面(`127.0.0.1:<port>`・`[::1]:<port>`。viewer はその束縛先の字面)と
    一致しなければ 421。DNS rebinding はここで止まる。自前のクライアント(http.rs)は
    `Host: <address>` を送るので影響しない。
  - `Origin` が付いているか、`Sec-Fetch-Site` が `same-origin` と `none` 以外なら、主の口は 403。
    viewer は自分の生成元(`http://<束縛先>`)だけを許す。
  - 本文を持つ POST・PUT(文書の PUT を除く)は `Content-Type: application/json` を要る。無ければ
    415。これで form と `text/plain` の単純な要求が通らない。文書の PUT は本文が生のバイト列なので、
    Origin の検査が守る。
- 主の口に届くプロセスは全部、操作者とみなす。hikalium で走る Claude Code のセッションも含む。
  FEED の allow-shrink・edit・retire・rollback の「操作者の承認」は、この意味である(Claude 中 6
  の直し方 (b))。人の手に限りたくなったら(エージェントが操作者の確認なしに縮みを承認したら困る、
  となったら)、そのときに sudo でしか届かない管理用のソケットを設計する。これは操作者に確かめる
  (下の「操作者に聞くこと」)。

### 2. 網越しに届く口は、用途ごとの待ち受けと許可表

読み口(`--listen-agent`)、FEED の書き口(`--listen-feed`。FEED.md で設計中)、そして viewer。
許可表の外は 403。束縛は wg1 のような限られた網のアドレスにし、install の firewall で送信元を
絞る。

### 3. viewer を網越しの口として許可表の下に置く(Claude 高 3)

viewer の転送に、読み口と同じ許可の判定を通す: 読めるコレクションの集合(`--viewer-collections`。
既定は空で、serve の `--agent-collections` と同じ形)、objects は chunk だけ、検索の `peers` は
400、`GET /v1/refs` は読めるコレクションの `collections/` の ref だけに絞った一覧(コレクションの
選択肢に使うのはそれだけ。VIEWER.md の「コレクションの選択肢」)。判定の関数は agent_door.rs の
`admit` と `screen` を許可表を引数に取る形にして共用する。

vega の viewer を 0.0.0.0 に置き続けるか(LAN と wg1 に出すか)は操作者の判断で、下で聞く。
置き続けるなら、許可表に加えて `--firewall-allow` と同じ形の送信元の制限を viewer にも足す。
VIEWER.md の「ネットワークへ出すなら暗号化・認証済みチャネルの上に置く(SPEC §6.2)」との
食い違いは、§6.2 の適用範囲をピアのプロトコルに限ると明記し、VIEWER.md の文を「網へ出すなら
許可表と送信元の制限の下に置く」に直す(Claude 低 8)。

### 4. ピアの口を SPEC の上で主の口から切り離す(Claude 中 4)

§6.3 の例(`10.0.0.2:7440`)、§7.1 と §7.3 の具体化、DISTRIBUTED_SEARCH.md の例は、ピアが
相手の主の口を叩く前提で書かれている。主の口をループバックに限るなら、ピアの要求は別の
待ち受け(ピア口。未実装で、§12 の未決事項に足す)で受けると書く。ピア口が通す最小の集合を
今決めておく: `GET /v1/status`、`GET /v1/replication/signers`、`GET /v1/replication/refs`、
`GET /v1/objects/{id}`、`GET /v1/refs/{name}`、`POST /v1/peer/query`。DISTRIBUTED_SEARCH.md が
既知の穴として書く「objects の GET は share を迂回する」は、ピア口の許可表の課題として書き残す。
試験の 2 ノード(node/tests)は 127.0.0.1 同士なので、今のままの主の口で動き続ける。

採らない案 B(主の口に固定トークン): uniqnode の利用者の 0600 のファイルに置けば別の uid と
ブラウザからは守れるが、同じことは uid の制限とブラウザの門で、MCP・CLI・viewer・install の
確認・テストにトークンを通す配管なしに得られる。第 1 版の「トークンでは守れる相手が増えない」は
誤りだった(Claude 高 1)。

## SPEC の書き直し(案)

§10 の冒頭の 1 文を、次のように差し替える:

> 管理と利用のための HTTP API。認証は持たず、信頼の境界は待ち受けごとに置く。主の口は
> ループバックにだけ束縛し、同じ機械の常駐の利用者と root のプロセスだけが届くよう firewall で
> 絞り(それらのプロセスは操作者とみなす)、全ての口を通す。ブラウザからの要求は Host・Origin・
> Content-Type の検査で断る。網越しに届く口(読み口・viewer、設計中の書き口とピア口)は用途ごとの
> 別の待ち受けで、各々の許可表の外を 403 で断る。詳細スキーマは実装マイルストーンで確定する。

表に次の行を足す: `POST /v1/admin/gc`、`GET /v1/objects/{id}/citation`、
`GET /v1/objects/{id}/rendition` と `/rendition/{alias}`、`GET /v1/closure/{id}`、
`POST /v1/collections/{c}/fetch`。表の下に待ち受けごとの許可の短い表を置き、読み口と viewer の
許可表の正典は agent_door.rs と docs/design/ の読み口の節である、と書く。§6.2・§6.3・§7・§12 を
上の 3 と 4 のとおり直す。

## 操作者に聞くこと

1. vega の viewer は 0.0.0.0:7450 に束縛していて、LAN と wg1 のどの機械からも全コレクション
   (lamalium のノートを含む)と全 ref が読める。これは意図どおりか。どこから開く必要があるか
   (vega の上のブラウザだけか、LAN の機械からもか)。答えで `--viewer-listen` と viewer の
   送信元の制限の値を決める。3 の許可表は、答えに関わらず入れる。
2. 主の口に届く hikalium のプロセス(Claude Code のセッションを含む)を、FEED の縮みの承認・
   edit・retire・rollback について「操作者」とみなしてよいか。よくなければ、sudo でしか届かない
   管理用の口を別に設計する。

答えを待つ間も、1 と 2 に依らない部分(主の口の束縛の検査・uid の制限・ブラウザの門・SPEC の
表と §7 の切り離し)は進められる。

## 完了条件

- serve と install が、`0.0.0.0:7440`・`[::]:7440`・`[::ffff:127.0.0.1]:7440`・`10.10.128.1:7440`・
  `localhost:7440` を理由を言って断り、`127.0.0.1:7440`・`127.0.0.2:7440`・`[::1]:7440` を通す
  テストがある。`--listen-nonloopback` で通り、警告が記録される。
- 主の口と viewer が、Host の違う要求に 421、Origin 付きの(viewer では自分以外の生成元の)要求に
  403、Content-Type の無い JSON の POST に 415 を返すテストがある。http.rs のクライアント・MCP・
  viewer の転送・install の確認は今までどおり通る。
- install の nft の規則ファイルに主の口の uid の規則が載り、install の確認がそれを見る。
- viewer が、読めない コレクションの検索に 403、chunk 以外の objects に 403、`peers` に 400 を返し、
  `GET /v1/refs` が読めるコレクションの ref だけを返すテストがある。
- SPEC §6.2・§6.3・§7・§10・§12 と VIEWER.md・DISTRIBUTED_SEARCH.md が上の形になっている。
- FEED.md の「口」の節が、この境界を引いている。

## 段取り

| 段 | 中身 | 大きさ |
|---|---|---|
| A1 | serve と install の束縛の検査、http.rs のブラウザの門(Host・Origin・Content-Type)、SPEC と design の書き直し | M |
| A2 | install の主の口の uid の規則(nft の output)と確認 | S |
| A3 | viewer の許可表(agent_door.rs の判定の共用、`--viewer-collections`)。本番の値は操作者の答え 1 で決める | M |

A1〜A3 はレビューで高の指摘が無いと確かめてから入る。FEED の F2 は A1 と A2 の後にする。
本番への反映(A2・A3 の据え付け)は、操作者への sudo の依頼として渡す。

# API_AUTH: ノードローカル API の信頼の境界を、SPEC §10 と実物で揃える(設計草案)

<a id="abde9b3c-75f8-453b-988e-bfb1e178c771"></a>

版: 第 4 版(2026-10-01)。第 3 版(c1067dc)への Codex の再レビュー(uid の判定の高、中 3・4、低の
namespace)と、crystal の Codex の再レビュー(同じ高を実際に再現)を取り込んだ。第 2 版(e7c6b61)への Codex のレビュー 2 本(vega の Codex の 1〜12、crystal の
Codex の H1・H2・M1)を取り込んだ。viewer の扱い(3 と A3)は操作者の確認待ちで、第 2 版のまま置く。
第 1 版(a9eba65)への Claude のレビュー(高 1〜3・中 4〜6・低 7〜9)を
取り込んだ。次は Codex のレビュー。
出所は lamalium の健全性点検(2026-09-30)の項目 12。SPEC.md §10 は「認証は当面固定トークン」と
言うが、コードにトークンは無い。表にも実装済みの口が載っていない。FEED の F2 は、主の口にだけ
`edit`・`retire`・`allow-shrink`・`rollback` を置いて操作者の歯止めにする設計なので、その前に
「主の口を誰が叩けるか」を決める([docs/plan/FEED.md](#fa8de6f9-59f8-4512-a815-9f41d305db15))。

## 今の事実(2026-10-01 のコードと vega)

表の「本番の束縛」の列と下の利用者の段落は、2026-10-01 00:2x JST に vega で `ss -ltnp`・`getent passwd`・
`id` を読んで観測したもの。他の列はコードの読みである(Codex 低 12)。

| 待ち受け | 本番の束縛 | 認証 | 通すもの |
|---|---|---|---|
| 主の口(serve の `<listen>`) | 127.0.0.1:7440(graph_a・graph_b は 7444・7442) | 無い。送信元・Host・Origin・Content-Type も見ない(http.rs が見るヘッダは connection・content-length・transfer-encoding・expect だけ) | 全部(`/v1/admin/shutdown`・`/v1/admin/gc`・`PUT /v1/refs/…`・`POST /v1/sync`・`POST /v1/collections/{c}/fetch` を含む) |
| 読み口(`--listen-agent`) | 10.10.128.1:7441・7445・7443(wg1) | 無い。送信元は記録に写すだけ | agent_door.rs の許可表: 読む口(chunk だけの objects と citation、読めるコレクションに絞った検索、`peers` は 400)、許したコレクションへの文書の PUT、許したグラフの読み書き。外は 403。守りは許可表・wg1 への束縛・install の firewall の 3 重 |
| viewer(`--viewer-listen`) | 0.0.0.0:7450(本番の命令が渡す。wg1 の 10.10.128.1:7450 も含む) | 無い。firewall も無い | 読むだけの転送だが、許可表は無い: 全 ref の一覧(`GET /v1/refs`)、どんなオブジェクトでも(`GET /v1/objects/{id}`)、全コレクションの検索(`peers` も素通し) |
| MCP | stdio | 起動した利用者 | `--serve-url` があれば主の口への HTTP、無ければストアを直接開く(main.rs の 1818 行付近) |

vega には hikalium(uid 1000。serve・viewer・MCP を走らせる)の他に、別の利用者 lamalium
(uid 1001)がいて常駐を走らせている。docker もある(host network のコンテナは 127.0.0.1 に届く)。

viewer の転送には method と道の許可表がある(viewer.rs の 75 行付近。shutdown・gc・fetch・sync・
objects の POST・pins の POST は転送しない)。無いのはデータの範囲(コレクション)の絞りである(Codex 低 11)。

`POST /v1/peer/query` は、HTTP の接続そのものは認証しないが、中身の QUERY の署名を検証し、その
署名者が登録済みのピアで trust_level が正で share が許すことを確かめる(api.rs の 1297 行付近、
distributed_search.rs の 185 行付近)。署名された要求者に対する認証と認可である。ピア口へ移しても
この判定を保つ(Codex 中 10)。

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

### 1. 主の口: 常駐の利用者と root のプロセスだけ、ブラウザは通さない

- 束縛はループバックの IP リテラルだけ(`127.0.0.0/8` と `[::1]`)。`localhost` のような名前・
  unspecified(`0.0.0.0`・`[::]`)・IPv4 射影(`[::ffff:127.0.0.1]`)は断る(Claude 低 7)。serve 自身が
  起動時に断る。例外のフラグは置かない: 認証の無い主の口を網へ出す道を作らない。網越しに要るもの
  (ピアの要求)は 4 のピア口で受ける(crystal の Codex H1)。install は同じ判定を据え付けの前に
  早く当てる(Claude 中 5)。
- uid の制限は serve の中で行う(crystal の Codex H2、vega の Codex 1)。主の口が接続を受けたら、
  要求を 1 バイトも読む前に、/proc/net/tcp と /proc/net/tcp6 の両方から相手のソケットの行を引き、
  その持ち主の uid を読む。許す uid の集合(既定は serve 自身の euid と 0)に無ければ 403 を返して閉じる。
  行の選び方は厳しくする(第 3 版への Codex の再レビューの高):
  - 相手側から見た 4 つ組が完全に一致する行だけを見る: その行の local が相手のアドレスとポート、
    remote が自分の束縛先のアドレスとポート。IPv4 の接続でも、相手が AF_INET6 のソケットから
    `::ffff:127.0.0.1` で繋いだなら行は tcp6 にあるので、両方を引き、IPv4 射影は IPv4 に直して比べる
    (Codex 中 3)。
  - その行の状態が ESTABLISHED(01)で、inode が 0 でないこと。切断の途中のソケット(FIN_WAIT2 から
    TIME_WAIT の構造へ移ったもの)は /proc で uid と inode が 0 と表示されるので、状態と inode を見ないと
    root と取り違える(crystal の Codex が、uid 1001 のクライアントが 20 バイト送って閉じ、200 ms 後に
    accept した接続で state 05・uid 0・inode 0 を実際に見た)。送ってすぐ閉じた接続は、この規則で
    必ず 403 になる(閉じる側に倒れる)。半分だけ閉じた(shutdown(SHUT_WR))正規のクライアントも断る
    ことになるが、http.rs のクライアント・MCP・curl はそうしないので受け入れる。
  - 一致する行がちょうど 1 つであること。0 行・2 行以上・読めない(/proc が無い)は全部 403。
  - 判定の結果を接続をまたいで覚えない。
  - この規則で接続と uid を安全に結べないと実装の段で分かったら、/proc を捨て、主の口を Unix ドメイン
    ソケット(SO_PEERCRED で相手の uid を確実に得る)へ移す案に切り替え、この文書を改めてレビューに出す。
  - install・user 単位・手で起こした serve・テストの、どの起こし方でも同じ判定が効く。nft も root も
    要らず、ufw と nft のどちらが入っているかにも依らない。第 2 版の nft の output の規則(A2)は捨てる。
  - root を許すのは、root はストアのファイルを直接読めるので守る意味が無いからである。host network の
    docker のコンテナは、中の uid がそのまま見える(root で走るものは届き、それ以外は届かない)。
  - 許す uid を足す口として `--main-allow-uid <uid,...>` を置く(viewer を別の利用者で走らせる形など)。
    これは既定の集合を置き換える。テストはこれで自分の uid を外し、実際の接続が 403 になることを見る。
  - 対象は Linux だけである(/proc/net/tcp が要る)。他の OS では serve が起動時に理由を言って断る。
  - uid は接続ごとに 1 度判定し、HTTP のヘッダ(下の門)は要求ごとに見る。serve は keep-alive を
    受けるので、同じ接続の 2 つ目以降の要求は uid の判定を繰り返さない(Codex 中 4)。
  - 代価は接続ごとに 2 つの表を 1 度ずつ読むこと。行数は同じ network namespace の全ソケット
    (TIME_WAIT を含む)で、上限は無い(2026-10-01 の vega では計 75 行)。判定を同時に走らせる数に上限
    (16)を置き、超えた接続は 503 で閉じる。読む行数にも上限(例 100,000 行)を置き、超えたら 403 に
    する。今の http.rs は接続ごとにスレッドを作るので、この上限が主の口への無制限の負荷を抑える。
    TIME_WAIT を大量に作った状態での判定の時間を測る試験を置く(Codex 中 4)。
  - namespace: /proc/net の表は serve 自身の network namespace のものである。ループバックに届くのは
    同じ network namespace のプロセスだけなので、表に無い相手は無い。uid は serve の user namespace へ
    写した値として表示され、写せない uid は overflowuid(65534)になるので許されない。別の user namespace
    で uid を写したコンテナは、写した先の uid が許す集合に入るとき(serve の利用者か root に写したとき)
    だけ届く(第 3 版への Codex の再レビューの低)。
- ブラウザの遮断: http.rs に門を置き、主の口に効かせる(Claude 高 2)。viewer に効かせるかは 3(A3)と
  一緒に決める(viewer に同じ門を当てると、0.0.0.0 に束縛した viewer を LAN のアドレスで開く正規の
  要求まで 421 になる。crystal の Codex M1、vega の Codex 5)。
  - 許す authority の一覧を束縛先と分けて持つ。主の口の既定は、束縛したループバックの字面
    (`127.0.0.1:<port>` など)と `localhost:<port>` である。`Host` がこの一覧に無ければ 421。DNS
    rebinding はここで止まる(攻撃者の名前は `localhost` になれない)。ポート 0 で束縛したときは、
    実際に割り当てられたポートで照らす。
  - `Origin` が付いているか、`Sec-Fetch-Site` が `same-origin` と `none` 以外なら 403。ブラウザは
    GET と HEAD 以外の要求には `no-cors` でも `Origin` を付けるので、書き込みの CSRF はここで止まる。
  - Content-Type を道ごとに決める(vega の Codex 中 6)。JSON を読む道は `application/json` を要る。
    生のバイト列を読む道(`POST /v1/objects`、`PUT /v1/collections/{c}/documents/{name}`)は
    `application/octet-stream` か、文書の種別を言う型を受ける。本文を持たずに動く道(shutdown、本文を
    省いた gc)は Content-Type を問わない。どの道でも、ブラウザが単純な要求で送れる 3 つの型
    (`text/plain`・`multipart/form-data`・`application/x-www-form-urlencoded`)は 415。Origin の検査が
    主の守りで、これは重ねの守りである。
  - node/tests の共通の手書きの HTTP 要求(common/mod.rs の 278 行付近)は `Host: x` で Content-Type を
    送らない。停止の要求も同じで、断られると終了待ちのまま止まる。A1 はこの共通の口と手書きの要求を
    直し、停止では応答の番号と待つ期限も見る(vega の Codex 中 9)。http.rs の JSON のクライアントは
    既に Content-Type を送る(crystal の Codex の確認)。
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

### 3a. 読み口: 出所を示せないチャンクを断る(vega の Codex 高 3)

読み口の `screen`(agent_door.rs の 317 行付近)は、出所のコレクションが引けたときだけ許可表と
照らし、引けないとき(`None`)でもチャンクなら通す。見えに無いチャンク(旧版など。api.rs の 1221
行付近で出所が `None` になる)は、ID を知る相手なら許可の外のコレクションのものでも読める。旧版は
previous から辿れるので gc の後も残る(RAG 項目 18)。読めるコレクションが絞られているとき
(`--agent-collections` を指定したとき)は、出所を示せないチャンクを 403 にする。絞りが全部のときは
今のままでよい(全部読めるので漏れは無い)。FEED の「見える ref の判定」(FEED.md)で隠れた c の
チャンクは、出所の引き当てが「隠れた c にだけ属する」と返すので、c が読めるコレクションの外なら 403、
内なら 503 になる(出所が無いものとは区別する。FEED.md の同じ節)。

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
> ループバックにだけ束縛し、接続の相手のソケットの uid が常駐の利用者か root であるものだけを
> 受け(それらのプロセスは操作者とみなす)、全ての口を通す。ブラウザからの要求は Host・Origin・
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
  テストがある。
- `--main-allow-uid` で自分の uid を外した serve への実際の接続が 403 になり、既定の serve には通る
  テストがある(別の uid からの接続を、root 無しで実際に断らせる形)。/proc/net/tcp と tcp6 の行の
  読み方は、固定の行を与えるテストでも固める: 完全な 4 つ組、ESTABLISHED 以外の状態、inode 0 の行
  (FIN_WAIT2・TIME_WAIT の形)、同じ 4 つ組の 2 行、IPv4 射影の行、行数の上限。
- AF_INET6 のソケットから `[::ffff:127.0.0.1]` で IPv4 の主の口へ繋ぐ実際の接続が通る。
- 送ってすぐ閉じる接続(IPv4 と IPv6 の両方。accept を遅らせて相手の行を FIN_WAIT2・TIME_WAIT の形に
  してから判定させる)と、判定の同時数の上限を超える接続が、どちらも要求を実行されずに終わる。
- 主の口が、一覧に無い Host に 421、Origin 付きの要求に 403、単純な要求の 3 つの型に 415、JSON の道で
  Content-Type の無い要求に 415 を返すテストがある。http.rs のクライアント・MCP・viewer の転送・install
  の確認・node/tests の共通の口は通る。
- 読み口で、読めるコレクションを絞ったとき、見えに無いチャンクの `GET /v1/objects/{id}` と出典が 403
  になるテストがある。
- SPEC §6.2・§6.3・§7・§10・§12 と DISTRIBUTED_SEARCH.md が上の形になっている。
- FEED.md の「口」の節が、この境界を引いている。
- viewer の完了条件は A3 と一緒に決める。

## 段取り

| 段 | 中身 | 大きさ |
|---|---|---|
| A1 | serve と install の束縛の検査、http.rs のブラウザの門(Host・Origin・Content-Type)、node/tests の共通の口、SPEC と design の書き直し | M |
| A2 | serve の中の uid の判定(/proc/net/tcp、`--main-allow-uid`) | S |
| A4 | 読み口の、出所を示せないチャンクの拒否 | S |
| A3 | viewer の許可表(agent_door.rs の判定の共用、`--viewer-collections`)。本番の値は操作者の答え 1 で決める | M |

A1・A2・A4 はレビューで高の指摘が無いと確かめてから入る。A3 は操作者の確認の後に設計を詰める。
その際は vega の Codex の 2(`collection` を省いた検索は admit を素通りし、主の口は全部を検索する)、
4(`--agent-collections` の「空は全部」を写すと既定で全公開になる)、7(viewer は rendition を使うが
読み口の許可表には無い)、8(VIEWER.md の暗号化の要求との関係)を満たす形にする。FEED の F2 は A1 と
A2 の後にする。本番への反映は serve の据え付け直しで、操作者への sudo の依頼として渡す。

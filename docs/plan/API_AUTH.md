# API_AUTH: ノードローカル API の信頼の境界を、SPEC §10 と実物で揃える(設計草案)

<a id="abde9b3c-75f8-453b-988e-bfb1e178c771"></a>

版: 第 14 版(2026-10-01)。操作者の裁定(2026-10-01、利用者から直接)「viewer (7450) はこのままでよい」を「操作者に聞くこと」の 1 の答えとして記録し、3 を「viewer は今のまま置く」に改め、A3 を取り下げた(番号は残す)。viewer の網への露出を裁定の下で受け入れた危険として書き、A5 の「自分の機械のアドレスを断る」を A3 から切り離して A5 自身の項にした。第 13 版への Codex と Claude のレビュー(Claude 低 15: 名前解決の子の `getent ahosts --` と IPv6 のリテラルの角括弧)を取り込んだ。第 13 版(2026-10-01)。第 12 版への Codex と Claude のレビュー(Codex 中 6: fetch の全体の期限が名前解決を含まない、Claude 中 5: install の断りの完了条件、低 12: `--noproxy` の引数の字面)を取り込んだ。第 12 版(2026-10-01)。第 11 版への Codex と Claude のレビュー(Codex 中 5: fetch の curl が curlrc と proxy の環境を継ぐ、Claude 中 5: vega で共有するバイナリと旧い名の unit、低 4: 転送の段をまたぐ予算と自分の LAN のアドレス、低 9: 読み捨ての枠の数)を取り込んだ。第 11 版(2026-10-01)。第 10 版(b4f026c)への Claude のレビュー(中 1: 起動時の断りをストアを開く前に、中 5: 旧い名の
unit からの移行を A2 の本番への反映の前提に、中 6: 判定の枠を 403 の読み捨てから切り離す、低 5: fetch の転送先、低 9: ストアの
ロックの横取り)を取り込んだ。第 10 版(2026-10-01)。第 9 版(98e4b0c)への vega の Codex の再レビュー(中: overflowuid)を取り込んだ。第 8 版(601ae81)への vega の Codex の再レビュー(中: unit の
RestrictAddressFamilies に AF_NETLINK が無い、低: 節の見出し)を取り込んだ。第 7 版(b398d42)への vega の Codex の再レビュー(中: install の起動の確認、
低 2)を取り込んだ。第 6 版(e34d81d)への vega の Codex の再レビュー(中: 同じソケットの重複、
低: 枠の受け渡し)と、crystal の Claude の第 3 版へのレビュー(中 2・4・5・6、低)を取り込んだ。第 5 版(3152f68)への Claude のレビュー(中 3: 行数の上限が網越しの DoS に
なる → sock_diag の 1 件の照会へ、低 1〜3)と vega の Codex の再レビュー(低: 枠の超過の応答と資源の
完了条件)を取り込んだ。第 4 版(8d55129)への vega の Codex の再レビュー(中 3: 判定の枠をスレッドを
作る前に取る)と、共有チャンクの出所の順(FEED 第 7 版)を取り込んだ。第 3 版(c1067dc)への Codex の再レビュー(uid の判定の高、中 3・4、低の
namespace)と、crystal の Codex の再レビュー(同じ高を実際に再現)を取り込んだ。第 2 版(e7c6b61)への Codex のレビュー 2 本(vega の Codex の 1〜12、crystal の
Codex の H1・H2・M1)を取り込んだ。viewer の扱い(3 と A3)は操作者の確認待ちで、第 2 版のまま置く(第 14 版で、
2026-10-01 の操作者の裁定「viewer (7450) はこのままでよい」により決着した。3 は viewer を今のまま置く形に改め、A3 は取り下げた)。
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
| viewer(`--viewer-listen`) | 0.0.0.0:7450(本番の命令が渡す。wg1 の 10.10.128.1:7450 も含む) | 無い。firewall も無い | 読むだけの転送だが、許可表は無い: 全 ref の一覧(`GET /v1/refs`)、どんなオブジェクトでも(`GET /v1/objects/{id}`)、全コレクションの検索(`peers` も素通し)。2026-10-01 の操作者の裁定で、この形のまま置く(3) |
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
   (lamalium)が、LAN と wg1 から読める(Claude 高 3・低 9)。第 14 版: 操作者の裁定で viewer を今のまま
   置くので、これは直さずに受け入れた危険とする(3)。

## 決めること(案)

トークンは入れない。信頼の境界を「誰の uid か」「どの Host・生成元か」「どの許可表か」で置く。

### 1. 主の口: 許す uid の集合に入るプロセスだけ、ブラウザは通さない

- 束縛はループバックの IP リテラルだけ(`127.0.0.0/8` と `[::1]`)。`localhost` のような名前・
  unspecified(`0.0.0.0`・`[::]`)・IPv4 射影(`[::ffff:127.0.0.1]`)は断る(Claude 低 7)。serve 自身が
  起動時に断る。例外のフラグは置かない: 認証の無い主の口を網へ出す道を作らない。網越しに要るもの
  (ピアの要求)は 4 のピア口で受ける(crystal の Codex H1)。install は同じ判定を据え付けの前に
  早く当てる(Claude 中 5)。
- uid の制限は serve の中で行う(crystal の Codex H2、vega の Codex 1)。主の口が接続を受けたら、
  要求を 1 バイトも読む前に、相手のソケットを NETLINK_SOCK_DIAG(inet_diag)で 1 件照会し、その
  持ち主の uid を読む。照会は dump ではなく、下の逆向きの 4 つ組をそのまま指定する形にする(root は
  要らない。state・uid・inode がまとめて返る)。AF_INET と AF_INET6 の両方に問う。/proc/net/tcp の
  全表の走査は使わない: 表の行数は網越しに増やせる(LAN から viewer や読み口へ `Connection: close` の
  要求を送り続けると、serve が先に閉じた TIME_WAIT の行が vega 側に積もる)ので、走査の費用と上限が
  主の口の全面の DoS になる(第 5 版への Claude のレビューの中 3)。以下の「行」は照会の答えを言う。許す uid の集合(既定は serve 自身の euid だけ)に無ければ 403 を返して閉じる。
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
  - 一致するソケットがちょうど 1 つであること。数えるのは答えの件数ではなく、異なるソケットの数で
    ある。カーネルの 1 件の照会(inet_diag_find_one_icsk)は、AF_INET と、アドレスを IPv4 射影にした
    AF_INET6 の両方で同じソケットを返しうるので、cookie と inode と正規化した 4 つ組が同じ答えは 1 つに
    まとめる(まとめた答えの間で uid か状態が違えば 403)。0 個・2 個以上・照会できないは全部 403
    (第 6 版への Codex の再レビューの中)。
  - 403 は最善努力で届ける: 要求の頭を上限つき(例 8 KiB、100 ms)で読み捨ててから 403 を書き、
    `shutdown(Write)` し、その後も総時間と総バイトの上限つき(例 1 秒、64 KiB)で読み捨ててから閉じる。
    未読のデータを残して閉じると Linux は RST を送るので、大きな本文や遅い相手には ECONNRESET に
    なりうる。守るのは「要求は実行されず、期限の内に閉じ、判定の枠が戻る」ことで、403 が届くのは
    小さな通常の要求についてである(crystal の Claude 低、第 7 版への Codex の再レビューの低)。
    この読み捨ては判定の枠の外で行う(第 10 版への Claude のレビューの中 6)。判定の枠は、照会の答えで
    許すか断るかが決まった時点で返す。断る接続は、上限つきの別の枠(8)を待たずに取れたときだけ
    読み捨てと 403 を行い、取れなければ 403 を書かずにその場で閉じる。第 10 版までの形では、403 の
    読み捨てが最大 100 ms と 1 秒のあいだ判定の枠を持ち続けるので、同じ機械の別の uid が毎秒 15 本
    ほど繋ぐだけで 16 の枠が全部塞がり、操作者の MCP・CLI も ExecStop の shutdown も判定に入れなく
    なった(ExecStop が失敗すると SIGKILL で保留に落ちる。APPEND_FAILURE の方針 5)。判定そのものは
    netlink の照会だけで相手の送信を待たないので、枠を持つ時間は相手に延ばされない。
  - 判定の結果を接続をまたいで覚えない。
  - この規則で接続と uid を安全に結べないと実装の段で分かったら、/proc を捨て、主の口を Unix ドメイン
    ソケット(SO_PEERCRED で相手の uid を確実に得る)へ移す案に切り替え、この文書を改めてレビューに出す。
  - install・user 単位・手で起こした serve・テストの、どの起こし方でも同じ判定が効く。nft も root も
    要らず、ufw と nft のどちらが入っているかにも依らない。第 2 版の nft の output の規則(A2)は捨てる。
  - root は既定では許さない(crystal の Claude 中 2)。host network の docker のコンテナの root や、他の
    利用者の代わりに localhost へ繋ぐ root のデーモンは、ストアのファイルを読めるとは限らない。
    root を許す必要があれば `--main-allow-uid` で集合に入れる。
  - 許す uid の集合を変える口として `--main-allow-uid <uid,...>` を置く(viewer を別の利用者で走らせる形など)。
    これは既定の集合を置き換える(root も許すなら、常駐の利用者を含めて `--main-allow-uid 1000,0`
    のように書く)。
  - system の install は root で走り、今は起動の確認(install.rs の 1348 行付近)で主の口へ直接 GET
    する。root は既定で許されないので、確認は常駐の利用者へ権限を落とした子プロセス(uid・gid・
    補助グループを常駐の利用者のものにしてから繋ぐ)から行う(第 7 版への Codex の再レビューの中)。テストはこれで自分の uid を外し、実際の接続が 403 になることを見る。
  - 配る unit(system と user の uniqnode-serve@.service)の `RestrictAddressFamilies=AF_UNIX AF_INET
    AF_INET6` に `AF_NETLINK` を足す。無いと照会のソケットを作れず、据え付けた serve が正規の接続まで
    403 にする(第 8 版への Codex の再レビューの中)。A2 はこの unit の変更を含む。
  - 対象は Linux だけである(sock_diag が要る)。他の OS では serve が起動時に理由を言って断る。
  - 起動時の断り(束縛先の字面、Linux 以外、許す集合に overflowuid、`--main-allow-uid` の字面の誤り)と
    主の口の束縛そのものは、`Store::open` より前に済ませる(第 10 版への Claude のレビューの中 1)。
    APPEND_FAILURE の S1 は、ストアを開いた後に無事な終わり方を通らずに抜けると印に `Running` を残し、
    同じブートの次の起動を保留にする。今の serve はストアを開いてから束縛し(main.rs の 1908〜1949 行
    付近)、束縛の失敗は `process::exit(1)` で抜けるので、そのままではポートの塞がりや引数の誤りの
    たびにホストの再起動が要る。順は、引数の検査 → A1・A2 の起動時の断り → 束縛 → ストアを開く →
    「listening on」の 1 行とし、1 行を開き終えた後に出す取り決めは保つ(APPEND_FAILURE の方針 5 の
    (a))。
  - uid は接続ごとに 1 度判定し、HTTP のヘッダ(下の門)は要求ごとに見る。serve は keep-alive を
    受けるので、同じ接続の 2 つ目以降の要求は uid の判定を繰り返さない(Codex 中 4)。
  - 代価は接続ごとに netlink の照会を 2 回まで(AF_INET と AF_INET6)。費用は表の大きさに依らない。
    判定を同時に走らせる数に上限(16)を置く。今の http.rs は accept の直後に無条件でスレッドを作るので、判定の枠はスレッドを作る前に、
    accept するスレッドの中で待たずに取る。枠が取れない接続は、新しいスレッドを作らずにその場で
    閉じる(応答は書かない。書くと遅い相手に accept が止められる)。判定を通った後の接続を持つ
    スレッドの数にも上限(64)を置き、超えたら同じく閉じる。こうして、判定の走査の数だけでなく、
    未認証の接続が作るスレッドの数も抑える(第 4 版への Codex の再レビューの中 3)。判定の枠は
    判定が決まった時点で、403 の読み捨ての枠はその読み捨てが終わって閉じたときに、接続の枠は接続の
    スレッドが終わるとき(閉じたとき)に返す(第 10 版への Claude のレビューの中 6)。受け渡しの順は、判定が通ったら接続の
    枠を待たずに取り、取れたら判定の枠を返して HTTP の処理へ移る(取れなければ閉じて判定の枠を返す)。
    判定の枠を接続の終わりまで持ち続けないので、認証済みの 16 本を持ったまま 17 本目も判定に入れる
    (第 6 版への Codex の再レビューの低)。
    TIME_WAIT を大量に作った状態での判定の時間を測る試験を置く(Codex 中 4)。
  - uid はソケットを作った者の uid である。許された uid のプロセスが中継すれば(viewer、操作者の
    socat や ssh -L)そのまま通る: 中継は操作者の権限を貸す。docker グループの利用者は root の
    コンテナで届くので、root と同じに扱う。F4 の前に vega で `getent group docker` を確かめ、
    lamalium の利用者が入っていないことを見る(Claude 低 3)。
  - namespace: sock_diag が答えるのは serve 自身の network namespace のソケットである。ループバックに
    届くのは同じ network namespace のプロセスだけなので、答えに無い相手は無い。uid は serve の user namespace へ
    写した値として表示され、写せない uid は overflowuid(`/proc/sys/kernel/overflowuid`、ふつう 65534)に
    置き換わる。答えの uid だけでは、本当にその uid のソケットと区別できないので、overflowuid は認可に
    使わない: serve は起動時に `/proc/sys/kernel/overflowuid` を読み、許す集合(既定でも明示でも)に
    その値が入っていれば理由を言って起動を断る。答えの uid が overflowuid なら 403(第 9 版への Codex の
    再レビューの中。user の unit は `PrivateUsers=yes` なので、この形は実際の配備にも関わる)。別の user namespace
    で uid を写したコンテナは、写した先の uid が許す集合に入るとき(serve の利用者か root に写したとき)
    だけ届く(第 3 版への Codex の再レビューの低)。
- ブラウザの遮断: http.rs に門を置き、主の口にだけ効かせる(Claude 高 2)。viewer には効かせない(3 の操作者の
  裁定。viewer に同じ門を当てると、0.0.0.0 に束縛した viewer を LAN のアドレスで開く正規の要求まで 421 に
  なる。crystal の Codex M1、vega の Codex 5)。viewer から主の口への転送は、http.rs のクライアントで新しい
  要求を作り(ブラウザの Origin などのヘッダを写さない。viewer.rs の `forward_get`・`forward_post`)、主の口の
  門を通る(完了条件の「viewer の転送」)。
  - 許す authority の一覧を束縛先と分けて持つ。主の口の既定は、束縛したループバックの字面
    (`127.0.0.1:<port>` など)と `localhost:<port>` である。`Host` がこの一覧に無ければ 421。比べるときは
    大文字と小文字を区別しない。ssh -L で別のローカルのポートから転送すると `Host` のポートが違って
    421 になるので、同じポート番号で転送するか、一覧に足す `--main-allow-host` を使う(crystal の
    Claude 低)。DNS
    rebinding はここで止まる(攻撃者の名前は `localhost` になれない)。ポート 0 で束縛したときは、
    実際に割り当てられたポートで照らす。
  - `Origin` が付いているか、`Sec-Fetch-Site` が `same-origin` と `none` 以外なら 403。ブラウザは
    GET と HEAD 以外の要求には `no-cors` でも `Origin` を付けるので、書き込みの CSRF はここで止まる。
  - Content-Type を道ごとに決める(vega の Codex 中 6)。JSON を読む道は `application/json` を要る。
    生のバイト列を読む道(`POST /v1/objects`、`PUT /v1/collections/{c}/documents/{name}`)は
    `application/octet-stream` か、文書の種別を言う型を受ける。本文を持たずに動く道(shutdown、本文を
    省いた gc、グラフの辺の本文の無い PUT と DELETE)は Content-Type を問わない。主の口を curl で叩く
    README の例(37・45・59 行付近。`-d` や `--data-binary` は既定で form-urlencoded を送る)には
    `-H 'Content-Type: application/json'` を足す。vega の `uniqnode-graph-pull.service` は既に
    `Content-Type: application/json` を送っている(2026-10-01 に systemctl cat で確かめた)。この規則は
    主の口だけのもので、読み口と feed の口には広げない(lamalium の uniqnode_put は読み口へ
    text/plain を送る。crystal の Claude 中 4)。どの道でも、ブラウザが単純な要求で送れる 3 つの型
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

読み口(`--listen-agent`)と FEED の書き口(`--listen-feed`。FEED.md で設計中)。
許可表の外は 403。束縛は wg1 のような限られた網のアドレスにし、install の firewall で送信元を
絞る。viewer も網越しに届くが、操作者の裁定で許可表も送信元の制限も置かない(3)。

### 3. viewer は今のまま置く(2026-10-01 の操作者の裁定。Claude 高 3 は受け入れた危険とする)

操作者の裁定(2026-10-01、利用者から直接): 「viewer (7450) はこのままでよい」。下の「操作者に聞くこと」の 1 の
答えである。viewer は次の形のまま置き、この文書では変えない:

- 束縛は本番の命令が渡す 0.0.0.0:7450 のまま(LAN と wg1 の両方から届く)。
- 許可表(読めるコレクションの集合)も、送信元の制限(`--firewall-allow` と同じ形)も足さない。
- ブラウザの門(1 の Host・Origin・Content-Type の検査)を viewer には当てない。

第 13 版までの案(viewer の転送に読み口と同じ許可の判定を通す `--viewer-collections`、objects は chunk だけ、
`peers` は 400、`GET /v1/refs` の絞り、agent_door.rs の `admit` と `screen` の共用、送信元の制限)は取り下げた。
段取りの A3 は取り下げとして番号を残す。

受け入れた危険(裁定の下で、そのまま残るもの): viewer は読む API を主の口へ転送するので、LAN か wg1 から
7450 に届く者は誰でも、全 ref の一覧(`GET /v1/refs`)、どんなオブジェクトでも(`GET /v1/objects/{id}`。
rendition と出典も)、全コレクションの検索(`POST /v1/search`。`peers` も素通しで、ピアへの分散検索も
起こせる)を読める。そこには lamalium のノート、グラフ層、今後の feeds/ の ref とそのコレクション(FEED の F2
の後)も入る。DNS rebinding で viewer を同じ生成元にしたブラウザのページも、同じ範囲を読める(脅威 2 の
viewer の側)。viewer の転送先は主の口で、viewer は hikalium の uid で走るので、A2 の uid の判定は通る(1 の
「中継は操作者の権限を貸す」)。残る守りは viewer の method と道の許可表(viewer.rs の `handle`)だけで、書く道
(shutdown・gc・fetch・sync・objects の POST・pins の POST・ref の PUT)は転送しないので、露出は読み出しに限る。

VIEWER.md の「ネットワークへ出すなら暗号化・認証済みチャネルの上に置く(SPEC §6.2)」との食い違いは残る
(LAN の側は暗号化されない)。A1 の design の書き直しで、§6.2 の適用範囲をピアのプロトコルに限ると明記し、
VIEWER.md の文を「viewer は認証も許可表も持たず、網へ出すと読み出しの全部がその網へ出る。vega で 0.0.0.0 に
置くのは 2026-10-01 の操作者の裁定による」に直す(Claude 低 8 の残り)。
後で viewer を絞ることにしたら、git の履歴にある第 13 版の 3 と、vega の Codex の 2・4・7・8(段取りの下)を
出発点にして改めて設計し、レビューに出す。

### 3a. 読み口: 出所を示せないチャンクを断る(vega の Codex 高 3)

読み口の `screen`(agent_door.rs の 317 行付近)は、出所のコレクションが引けたときだけ許可表と
照らし、引けないとき(`None`)でもチャンクなら通す。見えに無いチャンク(旧版など。api.rs の 1221
行付近で出所が `None` になる)は、ID を知る相手なら許可の外のコレクションのものでも読める。旧版は
previous から辿れるので gc の後も残る(RAG 項目 18)。読めるコレクションが絞られているとき
(`--agent-collections` を指定したとき)は、出所を示せないチャンクを 403 にする。絞りが全部のときは
今のままでよい(全部読めるので漏れは無い)。FEED の「見える ref の判定」(FEED.md)で隠れた c の
チャンクは、出所の候補を読めるコレクションに照らしてから選ぶ(FEED.md の同じ節の順):
読める範囲に見える出所があれば 200、無くて読める範囲に隠れた c の出所があれば 503、出所が読めない
範囲にしか無ければ 403、出所が無ければ上の規則(出所が無いものとは区別する)。

### 4. ピアの口を SPEC の上で主の口から切り離す(Claude 中 4)

§6.3 の例(`10.0.0.2:7440`)、§7.1 と §7.3 の具体化、DISTRIBUTED_SEARCH.md の例は、ピアが
相手の主の口を叩く前提で書かれている。主の口をループバックに限るなら、ピアの要求は別の
待ち受け(ピア口。未実装で、§12 の未決事項に足す)で受けると書く。ピア口が通す最小の集合を
今決めておく: `GET /v1/status`、`GET /v1/replication/signers`、`GET /v1/replication/refs`、
`GET /v1/objects/{id}`、`GET /v1/refs/{name}`、`POST /v1/peer/query`。DISTRIBUTED_SEARCH.md が
既知の穴として書く「objects の GET は share を迂回する」は、ピア口の許可表の課題として書き残す。
試験の 2 ノード(node/tests)は 127.0.0.1 同士なので、今のままの主の口で動き続ける。

### 5. fetch は転送を自分で辿り、ループバックとリンクローカルへ行かない(第 10 版への Claude のレビューの低 5)

`POST /v1/collections/{c}/fetch`・CLI の fetch・MCP の fetch_url が使う取得(fetch.rs の 173〜183 行
付近)は、curl に `--location` を渡して転送を辿らせ、`validate_url`(105 行付近)は字面の形しか見ない。
取りに行く相手のページが `http://127.0.0.1:7440/v1/refs` や `/v1/objects/{id}` へ転送すると、curl は
serve と同じ uid で主の口に繋ぐので、A2 の uid の判定もブラウザの門(Origin の無い GET)も通り、その
答えが文書としてコレクションに入る。そのコレクションを読み口や viewer が読めれば、主の口にしか無い
中身(全 ref の一覧、許可の外のコレクションのオブジェクト)が読み口の許可表の外へ出る。レビューは
「読み口の許可表を迂回する」と書いたが、読み口そのものは fetch を通さない(agent_door.rs の 586 行
付近の試験が 403 を固定している)。道は、主の口・MCP から fetch を打つ操作者やエージェントが、外の
ページの転送に乗せられる形である(最初の URL にループバックを直に書かせる形も同じ)。
直し方: curl の `--location` をやめ、転送は fetch.rs が 1 段ずつ辿る(上限は今の `MAX_REDIRECTS`)。
各段で、URL のホストを std の名前解決で引き、答えのどれかがループバック(`127.0.0.0/8`・`::1`)・
リンクローカル(`169.254.0.0/16`・`fe80::/10`)・unspecified・IPv4 射影のそれらなら理由を言って断る。
確かめたアドレスを curl の `--resolve <host>:<port>:<addr>` で固定して繋がせ、確かめてから繋ぐまでの
間の名前の差し替え(DNS rebinding)で別の先へ行かないようにする。最初の URL にも同じ判定を当てる。
curl の設定の継承を断つ(第 11 版への Codex のレビューの中 5): 今の起動(fetch.rs の 173 行付近)は環境と
既定の curlrc を継ぐので、引数から `--location` を除いても curlrc の `location` で転送を辿りうる。また
proxy を通すと名前解決は proxy がするので、`--resolve` で接続先を固定できない。そこで curl の先頭の引数を
`-q`(curlrc を読まない。先頭に置かないと効かない)にし、`--noproxy` とその引数の 1 文字 `*` を渡し
(`Command::arg` で渡す argv の 1 要素で、シェルを通さないので引用符は付けない。第 12 版への Claude のレビューの
低 12)、子の環境から
`http_proxy`・`https_proxy`・`HTTPS_PROXY`・`all_proxy`・`ALL_PROXY`・`no_proxy`・`NO_PROXY` を消す
(`HTTP_PROXY` の大文字は curl が読まないが、同じく消す)。
段をまたぐ予算(第 11 版への Claude のレビューの低 4): 1 段ずつ curl を起こすと、段ごとの `--max-time` と
`--max-filesize` が転送の数だけ掛け算になる。取得の全体の期限と全体の受け取りの上限を最初に決め、各段には
残りの時間(期限までの秒)と残りのバイト数を渡し、使い切ったら理由を言って断る。
名前解決も同じ期限に入れる(第 12 版への Codex のレビューの中 6): std の同期の名前解決(`ToSocketAddrs`)には
期限を掛ける手段が無く、DNS が止まれば curl を起こす前に全体の期限を超える。そこで各段の名前解決は、
std を呼ばず、子プロセス `getent ahosts -- <host>`(std と同じ NSS の解決。libc-bin にあり、この機械群では
/usr/bin/getent)を起こし(`--` は `-` で始まる名を選択肢と取り違えさせないため。`<host>` は URL のホストの
字面で、IPv6 のリテラル `[::1]` は角括弧を外して `::1` として渡す。curl の `--resolve` の `<host>` には URL の
字面のまま角括弧を付けて渡す。第 13 版への Claude のレビューの低 15)、残りの時間を期限にして待ち、過ぎたら子を kill して wait し、理由を言って断る。
答えは各行の先頭のアドレスを読み、上の判定に掛ける(`getent` が 2 を返す「名前が無い」は、その旨を言って
断る)。スレッドで std の解決を走らせて見捨てる形は採らない: 止まった解決のスレッドが要求ごとに残り続ける。
子プロセスなら期限で確実に片付く。
自分の機械のアドレスも断る(第 11 版への Claude のレビューの低 4。第 14 版で A3 から切り離した): ループバックと
リンクローカルだけを断ると、serve の機械自身の LAN・wg1 のアドレス(例えば vega の viewer の 10.10.128.1:7450、
読み口の 10.10.128.1:7441)への転送は通る。viewer は許可表の無い全読みの転送なので、外のページがそこへ転送
すれば、主の口と同じ中身(全 ref の一覧、どのコレクションのオブジェクトでも)が文書としてコレクションに入り、
読み口の許可表の外へ洗い出される(lamalium が読めるコレクションに入れば、lamalium に読めないものが読める)。
3 の裁定で viewer は今のまま置くので、viewer が LAN と wg1 に全部を見せること自体は既知の限界(受け入れた
危険)である。一方、fetch がそれをコレクションへ運ぶ道は、網に届かない相手(読み口しか使えない
エージェント)にまで露出を広げるので、安く塞げるものとして A5 で塞ぐ: 各段の名前解決の答えのどれかが、自分の
インタフェースのアドレス(`extern "C"` の `getifaddrs` で各段の判定のたびに読む。依存を足さない)か、serve が
束縛している待ち受けのアドレスなら、ループバックと同じく理由を言って断る。0.0.0.0 に束縛した viewer の
アドレスは、インタフェースのアドレスの全部で覆われる。LAN の他の機械の口への取得は、この判定の外に残る
(既知の限界)。

採らない案 B(主の口に固定トークン): uniqnode の利用者の 0600 のファイルに置けば別の uid と
ブラウザからは守れるが、同じことは uid の制限とブラウザの門で、MCP・CLI・viewer・install の
確認・テストにトークンを通す配管なしに得られる。第 1 版の「トークンでは守れる相手が増えない」は
誤りだった(Claude 高 1)。

## 既知の限界

- ストアのロックは、データのディレクトリの正規化した道の SHA-256 から名付けた抽象名前空間の unix
  socket である(store.rs の 416〜427 行付近)。抽象名前空間の socket には権限が無いので、同じ機械の
  別の uid(lamalium)が道を知れば、serve より先に同じ名前で束縛できる。そうされると serve は「別の
  プロセスがストアを開いている」で起動を断り、install も同じ探りで止まる。ストアの中身は読めないので
  漏れは無く、起動の妨げ(DoS)だけである。この文書の範囲では直さず、既知の限界として書き残す。
  直すなら、データのディレクトリの中(serve の利用者しか入れない)のファイルへの `flock` にロックを
  移す(第 10 版への Claude のレビューの低 9)。
- viewer(0.0.0.0:7450)は、LAN と wg1 から届く者の誰にでも、全 ref・どのオブジェクト・全コレクションの検索を
  読ませる。2026-10-01 の操作者の裁定で受け入れた危険である(3)。fetch がそれをコレクションへ運ぶ道は A5 が
  自分の機械のアドレスを断って塞ぐ(5)。

## SPEC の書き直し(案)

§10 の冒頭の 1 文を、次のように差し替える:

> 管理と利用のための HTTP API。認証は持たず、信頼の境界は待ち受けごとに置く。主の口は
> ループバックにだけ束縛し、接続の相手のソケットの uid が許す集合(既定は serve 自身の euid だけ。
> 変えるのは `--main-allow-uid`)に入るものだけを受け(それらのプロセスは操作者とみなす)、全ての口を通す。ブラウザからの要求は Host・Origin・
> Content-Type の検査で断る。網越しに届く口(読み口、設計中の書き口とピア口)は用途ごとの
> 別の待ち受けで、各々の許可表の外を 403 で断る。viewer は読む API を主の口へ転送するだけの口で、
> 許可表を持たない(網へ出すと読み出しの全部がその網へ出る。出すかは配備の判断)。詳細スキーマは
> 実装マイルストーンで確定する。

表に次の行を足す: `POST /v1/admin/gc`、`GET /v1/objects/{id}/citation`、
`GET /v1/objects/{id}/rendition` と `/rendition/{alias}`、`GET /v1/closure/{id}`、
`POST /v1/collections/{c}/fetch`。表の下に待ち受けごとの許可の短い表を置き、読み口の
許可表の正典は agent_door.rs と docs/design/ の読み口の節、viewer の method と道の許可表の正典は viewer.rs で
ある、と書く。§6.2・§6.3・§7・§12 を
上の 3 と 4 のとおり直す。

## 操作者に聞くこと

1. (答え済み)vega の viewer は 0.0.0.0:7450 に束縛していて、LAN と wg1 のどの機械からも全コレクション
   (lamalium のノートを含む)と全 ref が読める。これは意図どおりか。どこから開く必要があるか。
   答え(2026-10-01、利用者から直接): 「viewer (7450) はこのままでよい」。`--viewer-listen` は 0.0.0.0:7450 の
   まま、送信元の制限も許可表もブラウザの門も足さない(3)。第 13 版までの「3 の許可表は、答えに関わらず
   入れる」は、この裁定で取り下げた(A3)。
2. 主の口に届く hikalium のプロセス(Claude Code のセッションを含む)を、FEED の縮みの承認・
   edit・retire・rollback について「操作者」とみなしてよいか。よくなければ、sudo でしか届かない
   管理用の口を別に設計する。

答えを待つ間も、2 に依らない部分(主の口の束縛の検査・uid の制限・ブラウザの門・SPEC の
表と §7 の切り離し)は進められる。

## 完了条件

- serve と install が、`0.0.0.0:7440`・`[::]:7440`・`[::ffff:127.0.0.1]:7440`・`10.10.128.1:7440`・
  `localhost:7440` を理由を言って断り、`127.0.0.1:7440`・`127.0.0.2:7440`・`[::1]:7440` を通す
  テストがある。
- `--main-allow-uid` で自分の uid を外した serve への実際の接続が 403 になり、既定の serve には通る
  テストがある(別の uid からの接続を、root 無しで実際に断らせる形)。照会の答えの読み方は、固定の
  答えを与えるテストでも固める: 完全な 4 つ組、ESTABLISHED 以外の状態、inode 0 の答え(FIN_WAIT2・
  TIME_WAIT の形)、2 件の答え、IPv4 射影。
- 許される側の本物の接続が、`127.0.0.1` と `[::1]` に束縛した serve の両方に通る(照合が壊れて答えが
  0 件になっても 403 で通ってしまう、断る側の試験だけでは足りない。Claude 低 2)。
- AF_INET6 のソケットから `[::ffff:127.0.0.1]` で IPv4 の主の口へ繋ぐ実際の接続が通る。
- 送ってすぐ閉じる接続(IPv4 と IPv6 の両方。accept を遅らせて相手の行を FIN_WAIT2・TIME_WAIT の形に
  してから判定させる)と、判定の同時数の上限を超える接続が、どちらも要求を実行されずに終わる。
  上限を超える接続には HTTP の応答を書かずに閉じる。
- 大量の接続(例 1,000 本)を同時に開いたとき、主の口の判定中の接続とそのスレッドが 16、認証後の
  接続とそのスレッドが 64 を超えない(health や索引を温める常駐のスレッドは数えない。役割ごとの数を
  数える口で見る)。認証済みの 16 本を持ったまま 17 本目が通り、切断と失敗の後に枠が戻る。
- 同じソケットが AF_INET と AF_INET6 の両方から返る固定の答えは通り、異なるソケットの 2 つは 403。
- 小さな通常の要求で断られた相手が 403 の応答を受け取る(ECONNRESET にならない)。大きな本文や遅い
  相手の要求は、実行されず、期限の内に閉じられ、判定の枠が戻る。
- 許さない uid から、断られた後も 1 バイトずつ遅く送り続ける接続を毎秒 50 本ほど開き続けている間も、
  許す uid の接続が判定を通って答えを受け取り、shutdown が通る。判定の枠は判定が決まった時点で戻り、
  403 の読み捨てが 8 を超えない。読み捨ての枠が塞がっているときの断りは 403 を書かずに閉じる(第 10 版
  への Claude のレビューの中 6)。
- 束縛できないアドレスと、A1・A2 の起動時の断り(束縛先の字面、overflowuid)で serve が終わるとき、
  ストアを開いていない(印も node_key も作られず、同じ boot_id の次の起動が保留にならない。第 10 版への
  Claude のレビューの中 1)。
- fetch が、ループバックとリンクローカルへの転送(`http://127.0.0.1:<port>/…`、`http://[::1]/…`、
  `169.254.169.254`、ループバックへ解決する名前)と、それらを直に指す最初の URL を理由を言って断り、
  外への普通の転送は辿る(第 10 版への Claude のレビューの低 5)。
- fetch が、`location` と `proxy` を書いた curlrc を置いた HOME と、`http_proxy`・`https_proxy`・
  `ALL_PROXY` を立てた環境の下でも、ループバックへの転送を断り、proxy を通らずに `--resolve` で固定した
  先へ繋ぐ(第 11 版への Codex のレビューの中 5)。転送を重ねても、全体の期限と受け取りの上限を超えない
  (第 11 版への Claude のレビューの低 4)。名前解決が止まる形(debug ビルドだけが読む口で、解決の子を答えずに
  眠り続けるものへ差し替える)でも、fetch は全体の期限の内に理由を言って断り、解決の子が残らない(第 12 版への
  Codex のレビューの中 6)。curl の argv に `--noproxy` と `*` の 2 要素がそのまま並ぶことを、起動の引数を写す
  debug の口で見る(第 12 版への Claude のレビューの低 12)。`-` で始まる名と IPv6 のリテラル(`http://[::1]:<port>/`
  への転送)で、名前解決の子の argv が `getent`・`ahosts`・`--`・角括弧を外した字面になり、`--resolve` の字面が
  角括弧つきになることを同じ口で見る(第 13 版への Claude のレビューの低 15)。
- fetch が、serve の機械自身のインタフェースのアドレスへの転送(試験では、試験の機械の LAN 側のアドレスか、
  debug ビルドだけが読む口で差し替えたインタフェースの一覧のアドレスで viewer か serve を待たせる)と、それを直に
  指す最初の URL を理由を言って断る(第 14 版。5 の「自分の機械のアドレスも断る」)。
- install の断り(第 12 版への Claude のレビューの中 5): 据え先の unit_dir に旧い名の unit(LEGACY_UNITS の
  どれか)を置いた試験の据え先で `install --instance graph_a` を打つと、理由と docs/mop/SYSTEMD.md の「旧い名の
  unit から移る」を言って断り、据え先のバイナリのバイト列と mtime が前と同じである。旧い名が無ければ据わる。
  A1 と APPEND_FAILURE の S1a(S1 の最初の段)のうち先に入る方の完了条件にする(APPEND_FAILURE の完了条件と同じ試験)。
- root で走る system の install の起動の確認が、root の直接の接続は断る serve に対して通る。
- `--main-allow-uid` に overflowuid を入れた serve が理由を言って起動を断り、写せない uid の接続が 403 になる。
- 実際の unit の制限の下(install で据え付けた serve)で、許す uid の接続が通り、許さない uid は断られ、
  install の起動の確認が通る。
- 主の口が、一覧に無い Host に 421、Origin 付きの要求に 403、単純な要求の 3 つの型に 415、JSON の道で
  Content-Type の無い要求に 415 を返すテストがある。http.rs のクライアント・MCP・viewer の転送・install
  の確認・node/tests の共通の口は通る。
- 読み口で、読めるコレクションを絞ったとき、見えに無いチャンクの `GET /v1/objects/{id}` と出典が 403
  になるテストがある。
- SPEC §6.2・§6.3・§7・§10・§12 と DISTRIBUTED_SEARCH.md が上の形になっている。
- FEED.md の「口」の節が、この境界を引いている。
- viewer は 3 の裁定のとおり今の形を保つ: 既存の viewer の試験がそのまま通り、viewer の待ち受けには主の口の
  ブラウザの門が掛からず(LAN のアドレスの `Host` で開く要求が 421 にならない)、viewer の転送は主の口の門と uid の
  判定を通る(第 14 版。A3 の取り下げ)。

## 段取り

| 段 | 中身 | 大きさ |
|---|---|---|
| A1 | serve と install の束縛の検査、http.rs のブラウザの門(Host・Origin・Content-Type)、node/tests の共通の口、SPEC と design の書き直し。APPEND_FAILURE の S1a より先に入るなら、install が旧い名の unit の残る間は共有のバイナリの差し替えを断る直しも含める(下の「A2 の本番への反映の前提」) | M |
| A2 | serve の中の uid の判定(sock_diag の 1 件の照会、`--main-allow-uid`) | S |
| A4 | 読み口の、出所を示せないチャンクの拒否 | S |
| A5 | fetch の転送を自分で辿り、ループバックとリンクローカルと自分の機械のアドレス(インタフェースと束縛先)を断る。curl の `-q`・`--noproxy` と `*`・proxy の環境の消去、段をまたぐ予算、期限つきの名前解決(`getent ahosts --` の子)(5) | S |
| A3 | 取り下げ(2026-10-01 の操作者の裁定「viewer (7450) はこのままでよい」。3)。番号は再使用しない | — |

A1・A2・A4・A5 はレビューで高の指摘が無いと確かめてから入る。A3 は取り下げたので入れない。後で viewer を
絞ることにしたら、vega の Codex の 2(`collection` を省いた検索は admit を素通りし、主の口は全部を検索する)、
4(`--agent-collections` の「空は全部」を写すと既定で全公開になる)、7(viewer は rendition を使うが
読み口の許可表には無い)、8(VIEWER.md の暗号化の要求との関係)を満たす形で改めて設計する。FEED の F2 は A1 と
A2 の後にする。本番への反映は serve の据え付け直しで、操作者への sudo の依頼として渡す。

A2 の本番への反映の前提(第 10 版への Claude のレビューの中 5): vega の主のストアの serve は、今も
旧い名の unit `uniqnode-serve.service` で動いている(2026-10-01 に `systemctl cat` と
/etc/systemd/system/ の一覧で確かめた。その `RestrictAddressFamilies=` は `AF_UNIX AF_INET AF_INET6`
のまま)。`AF_NETLINK` を足すのは `@` のテンプレートだけで、install は旧い名が据え先に残っていると何も
置かずに断る(install.rs の 43〜50 行付近の LEGACY_UNITS)。バイナリだけを差し替えると、sock_diag の
照会のソケットを作れず、主の口の全ての接続が 403 になる(MCP の Forward・viewer・`--serve-url` の CLI が全部
止まる)。A2 を本番へ入れる前に、docs/mop/SYSTEMD.md の「旧い名の unit から移る」で
`uniqnode-serve@default` ほかへ移す。APPEND_FAILURE の S1 も同じ移行を前提にするので、1 回の移行で
両方の前提を満たす。

vega のバイナリの共有(第 11 版への Claude のレビューの中 5): 旧い `uniqnode-serve.service` と graph_a・
graph_b の serve は同じ /home/hikalium/.local/bin/uniqnode を走らせる(2026-10-01 に /etc/systemd/system/ の
drop-in の ExecStart= で確かめた)。install は既定でないインスタンスを据えるとき、旧い名を「取り合わない」
として残したまま進み(install.rs の 2087〜2096 行付近)、共有のバイナリを差し替える。A2 のバイナリで
graph_* を据え直すと、旧い serve の次の起動(restart・再起動)が AF_NETLINK の無い unit でそれを走らせ、
主の口の全ての接続が 403 になる。vega では、どのインスタンスの install も `@default` への移行の後に打つ
(docs/mop/SYSTEMD.md の「旧い名の unit から移る」の注意)。加えて、install は据え先に旧い名の unit が
残っている間は、インスタンスに依らず共有のバイナリの差し替えを断るように直す(A1 か S1a のうち先に入る
方に含め、完了条件の「install の断り」の試験で閉じる。第 12 版への Claude のレビューの中 5)。

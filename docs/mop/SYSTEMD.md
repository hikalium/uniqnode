# SYSTEMD — serve と viewer の常駐と、毎日の backup を systemd に任せる手順

<a id="7de68e4a-e6a6-4930-8cc7-a56f90f522e2"></a>

読み手は、1 台の機械で uniqnode の serve(HTTP API)と viewer(ブラウザ用の 1 枚の頁)を
起こしたままにし、ストアの写しを毎日取りたい運用者。unit ファイルの例は docs/mop/systemd/
にあり、この文書はその読み方と、起動・停止・更新・ログ・二重起動の手順を書く。各命令の
引数と既定の意味は
[docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004)(ログ)、
[docs/design/VIEWER.md](#4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847)(viewer)、
[docs/design/MCP.md](#dacd474d-424a-45d5-a278-766fc2465dd9)(LLM エージェントからの利用)、
[docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2)(backup が何を写し、どう戻すか)
にあり、ここでは繰り返さない。

## 構成

常駐するのは serve と viewer の 2 つである。mcp は LLM クライアントが会話ごとに起こす
ので unit にしない(走っている serve へ転送する形
`uniqnode mcp <dir> --serve-url http://127.0.0.1:7440` で登録する)。backup は常駐せず、
timer が毎日 1 回起こす一回きりの命令である。

unit はテンプレートである。`@` の後がインスタンス名で、同じ機械に何組でも置ける。名前を
決めるのは `uniqnode install <dir> --instance <名>` で、省くと `default` になる。以下は
既定のインスタンスで書いてあるので、別の名で据えたなら `default` をその名に読み替える。

| unit | 中身 | 待ち受け(既定) | ストアのロック |
|---|---|---|---|
| uniqnode-serve@default.service | `uniqnode serve <dir> 127.0.0.1:7440` | 7440 | 取る |
| uniqnode-viewer@default.service | `uniqnode viewer <dir> 127.0.0.1:7450 --serve-url http://127.0.0.1:7440` | 7450 | 取らない |
| uniqnode-backup@default.service | `uniqnode backup <dir> <backup_dir>`(Type=oneshot) | — | 取らない(写し先のロックだけを検証の間) |
| uniqnode-backup@default.timer | uniqnode-backup@default.service を毎日 0 時に起こす。Persistent=true なので逃した刻みは次の起動時に走る | — | — |

viewer は serve の後に起こす(After= と Wants=)。Requires= にしないのは、serve が居なくても
頁は出て、届かないことを 502 の本文で言うためである。serve を起こし直せばそのまま繋がる。
backup は serve に順序を持たない。ロックを取らないので serve が走っていても止まっていても同じ
写しが取れる。

unit は system 単位と user 単位で別のファイルにしてある。

| 置き場 | ファイル | 動かす者 | データディレクトリ(既定) | 写し先(既定) |
|---|---|---|---|---|
| /etc/systemd/system/ | docs/mop/systemd/system/*@.service と *@.timer | 専用ユーザー uniqnode(`install --system` は drop-in の User=/Group= で sudo を打った利用者に替える) | /var/lib/uniqnode/<インスタンス> | /var/backups/uniqnode/<インスタンス> |
| ~/.config/systemd/user/ | docs/mop/systemd/user/*@.service と *@.timer | 自分 | この機械の systemd 249 では ~/.config/uniqnode/<インスタンス>(新しい systemd では ~/.local/state/uniqnode/<インスタンス>)| ~/uniqnode-backup/<インスタンス> |

同じファイルを両方で使えないのは実測による。user 単位に User= を書くと exit 217 で
起動できず、閉じ込め(ProtectSystem= など)は user 単位では PrivateUsers=yes を添えないと
黙って効かない(/ が読み書き可のまま走る)。違いはその 3 点(User=/Group= の有無、
PrivateUsers=yes の有無、WantedBy=)と backup の写し先の既定だけで、各ファイルの先頭に
書いてある。timer は両方で同じ中身である。

## unit の読み方

運用者が変えるものはすべて環境変数にしてあり、ExecStart= はそれを並べるだけである。
変えるときは unit ファイルを編集せず、drop-in(`systemctl edit uniqnode-serve@default`。user 単位
なら `systemctl --user edit uniqnode-serve@default`)に `[Service]` と `Environment=` を書く。
unit ファイルは差し替えても drop-in は残る。

| 変数 | unit | 既定 | 意味 |
|---|---|---|---|
| UNIQNODE_DATA_DIR | 両方 | %S/uniqnode/%i | ストア。ロック・logs/・derived/ はこの下(%i はインスタンス名) |
| UNIQNODE_LISTEN | serve | 127.0.0.1:7440 | HTTP API の待ち受け |
| UNIQNODE_SERVE_OPTIONS | serve | 空 | 追加の引数(`--embed` など。空白で分ける) |
| UNIQNODE_VIEWER_LISTEN | viewer | 127.0.0.1:7450 | ブラウザが開く待ち受け |
| UNIQNODE_SERVE_URL | viewer | http://127.0.0.1:7440 | 転送先。UNIQNODE_LISTEN を変えたらここも |
| UNIQNODE_VIEWER_OPTIONS | viewer | 空 | 追加の引数(ログの指定など) |
| UNIQNODE_BACKUP_DIR | backup | system: /var/backups/uniqnode/%i、user: %h/uniqnode-backup/%i | 写し先。変えるときは ReadWritePaths= も(下) |
| UNIQNODE_AGENT_LISTEN | serve(install の `--listen-agent` だけが書く) | 無し | 読み口の待ち受け。unit の ExecStart= には無く、install が drop-in の ExecStart= の末尾に `--listen-agent ${UNIQNODE_AGENT_LISTEN}` を足す |
| UNIQNODE_AGENT_WRITABLE | serve(install の `--agent-writable` だけが書く) | 無し | 読み口から書けるコレクションの集合(空白区切り。読み手のための写し)。ExecStart= には展開せず、install が drop-in の ExecStart= の末尾に `--agent-writable <c>` を集合の数だけ値のまま並べる |
| UNIQNODE_AGENT_COLLECTIONS | serve(install の `--agent-collections` だけが書く) | 無し(全コレクションが読める) | 読み口から読めるコレクションの集合(空白区切り。読み手のための写し)。ExecStart= には展開せず、install が drop-in の ExecStart= の末尾に `--agent-collections <c>` を集合の数だけ値のまま並べる |
| UNIQNODE_AGENT_GRAPH | serve(install の `--agent-graph` だけが書く) | 無し(グラフ層は読み口に現れない) | 読み口から読めるグラフの集合(空白区切り。読み手のための写し)。ExecStart= には展開せず、install が drop-in の ExecStart= の末尾に `--agent-graph <g>` を集合の数だけ値のまま並べる |
| UNIQNODE_AGENT_GRAPH_WRITABLE | serve(install の `--agent-graph-writable` だけが書く) | 無し | 読み口から読み書きできるグラフの集合(空白区切り。読み手のための写し)。ExecStart= には展開せず、install が drop-in の ExecStart= の末尾に `--agent-graph-writable <g>` を集合の数だけ値のまま並べる |
| PATH | serve(install は 3 つに同じ値) | systemd の既定 /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin | pdftotext・pdftohtml・pdftoppm・curl を探す道。unit には書かず、install が自分の PATH で見つけた場所を前に足して drop-in に書く(下) |

- 埋め込みは既定で装備しない。serve は `--embed` を明示したときだけ埋め込みサーバに繋ぎ、
  何も指定しなければ他のプロセスに依存せず BM25 だけで答える。装備するなら drop-in に

  ```
  [Service]
  Environment="UNIQNODE_SERVE_OPTIONS=--embed http://127.0.0.1:8083/v1/embeddings"
  ```

  と書く(`--embedder <id>`、`--rerank <url>` も同じ変数に足す)。起動時に相手の生存は
  確かめないので、埋め込みサーバが後から起きても構わない。届かなければ検索は BM25 に
  劣化し、応答の `degraded` がそう言う。
- 追加の引数の変数だけ `${}` で囲んでいないのは、空のときに引数 0 個に消えるためである。
  `${UNIQNODE_SERVE_OPTIONS}` と書くと空文字列 1 個の引数になり、serve は usage で落ちる。
- データディレクトリを別の場所にするときは、UNIQNODE_DATA_DIR と一緒に
  `ReadWritePaths=<その道>` を drop-in に書く。閉じ込めで書けるのは StateDirectory= の下
  だけなので、それが無いとストアも logs/ も書けない(serve はログを倒せる先へ倒すか、
  それも無ければ書けないことを標準エラーで言って走り続けるが、ストアが書けないので起動に
  失敗する)。ディレクトリは先に作っておく(ReadWritePaths= は無い道を作らない)。
- backup の写し先(UNIQNODE_BACKUP_DIR)を変えるときは、環境変数と一緒に ReadWritePaths= も
  drop-in で差し替える。環境変数は ReadWritePaths= に展開されないので、道は 2 箇所に書く。
  ReadWritePaths= は ExecStart= と同じく追記なので、空の行で unit 既定の道を一度消す:

  ```
  [Service]
  Environment=UNIQNODE_BACKUP_DIR=/mnt/backup/uniqnode
  ReadWritePaths=
  ReadWritePaths=/mnt/backup/uniqnode
  ```

  消し忘れて既定の道が無いままだと、新しい道を足しても起動が 226/NAMESPACE で失敗する
  (実測: `Failed to set up mount namespacing: …/uniqnode-backup: No such file or directory`)。
  写し先のディレクトリは先に作る(system 単位では
  `sudo install -d -o uniqnode -g uniqnode -m 0750 /var/backups/uniqnode`。写しにも node_key が
  入るので 0750)。
- StateDirectory=uniqnode は起動のたびに %S/uniqnode を作り(system 単位では User= の
  所有にし)、3 つの service で共有する。viewer が書くのは logs/viewer.log だけで、backup は
  読むだけである。
  StateDirectoryMode=0750 なのは、node_key(DBノードの秘密鍵。自身は 0600)の入った
  ディレクトリを他の利用者に見せないため。
- Restart=on-failure と RestartSec=2s。落ちたら 2 秒後に起こし直す。ストアのロックは
  データディレクトリの道から名付けた抽象名前空間の unix socket で(node/src/store.rs の
  acquire_lock)、プロセスが死ねば kill -9 でもカーネルが解放するので、ロックが残って再起動を
  阻むことはない。書き込み途中で裂かれたストアは次の open が fsck なしで回復する
  (node/tests/crash.rs がそれを実プロセスで確かめている)。
- RestartPreventExitStatus=1 2。起動時に分かる誤りは再起動で直らないので、1 回で止めて
  理由を journal に残す。1 はアドレスが塞がっている・別プロセスがストアを開いている、
  2 は引数の誤りと、主の口の起動時の自己試験の設定による失敗(照会の netlink ソケットを
  作れない。AF_NETLINK を塞いだ unit など。自分の uid が overflowuid に見える user namespace)
  である。自己試験のそれ以外の失敗(照会の期限切れのような一時の失敗でありうるもの)は 3 で終わり、
  on-failure が起こし直す。POST /v1/admin/shutdown による終了(0)は意図した停止なので、
  on-failure は起こし直さない(unit は inactive のまま。起こすなら `systemctl start`)。
- 閉じ込めは書ける場所を StateDirectory= の下だけにし、/ と home を読むだけにする。
  RestrictAddressFamilies= に AF_UNIX を残しているのはロックが unix socket だからで、
  PrivateNetwork= を使わないのは、抽象名前空間の socket がネットワーク名前空間ごとに別に
  なり、外の CLI と unit が互いのロックを見られなくなる(二重起動を検出できなくなる)からで
  ある。viewer と mcp が loopback で serve に届く必要もある。serve の unit に AF_NETLINK を
  足しているのは、主の口が接続の相手のソケットの uid を NETLINK_SOCK_DIAG に照会するからで、
  無いと照会のソケットを作れない。serve は束縛の後の自己試験(自分から主の口へ 1 本繋いで
  判定する)でこれを見つけ、listening on を出さずに理由を言って 2 で終わる(全部の接続を 403 で
  断るまま active で居座らない。[docs/plan/API_AUTH.md](#abde9b3c-75f8-453b-988e-bfb1e178c771)
  の 1)。
- 主の口は serve と同じ uid(`--main-allow-uid` で集合を置き換えられる)の接続だけを受ける。
  system 単位の serve は unit の User=(既定は uniqnode。`install --system` は drop-in で sudo を
  打った利用者に替える)で走るので、それ以外の利用者(root を含む)の curl は 403 になる。
  install の起動の確認(`GET /v1/status` の node_id の照合)は、system 単位では
  サービスの利用者に権限を落とした子プロセス(置いたバイナリの隠しコマンド
  `install-status-probe`)から主の口を探る。手で確かめるなら、unit の実際の User= で
  `sudo -u "$(systemctl show -p User --value uniqnode-serve@default)" curl …` の形で叩く。user 単位の unit は PrivateUsers=yes なので、serve からは自分と root 以外の uid が
  overflowuid に見え、断られる。
- pdftotext・pdftohtml・pdftoppm(PDF の取り込み・見出し・写し)と curl(URL の取り込み)は
  PATH から引く。閉じ込めは /usr の実行を妨げず、ProtectHome=read-only は home の下の
  実行も妨げない。ただし unit の PATH はログインシェルのものではなく systemd の既定
  `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin` で、~/.local/bin を含まない。
  そこに symlink した pdftotext を手で起こした serve は見つけ、unit の serve は見つけず、
  `POST /v1/collections/web/fetch` の PDF が 503「PDF の取り込みには pdftotext コマンドが
  必要」で失敗した(実測 2026-09-05。HTML は通る)。install は据える時点の自分の PATH で
  4 つの道具を探し、見つかったディレクトリを既定の前に置いた `Environment=PATH=…` を
  drop-in に書く(下の表の PATH)。手で置いた unit や、後から別の場所に道具を置いたときは、
  同じ 1 行を drop-in に書く:

  ```
  [Service]
  Environment=PATH=/home/op/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
  ```

  poppler の一式(pdftohtml・pdftoppm)は PATH に無くても pdftotext の実体の隣にあれば
  serve が見つける(node/src/rendition.rs の candidate_commands)ので、PATH に要るのは
  pdftotext と curl の場所である。

## 準備

```
cargo build --release -p uniqnode
sudo install -m 0755 target/release/uniqnode /usr/local/bin/uniqnode
```

unit の ExecStart= は /usr/local/bin/uniqnode を指す。別の道に置くなら drop-in で
`ExecStart=` を空にしてから書き直す(ExecStart= は上書きでなく追記なので、空の行で一度
消す。user 単位の `uniqnode install` はこれを自分で書く):

```
[Service]
ExecStart=
ExecStart=/opt/uniqnode/bin/uniqnode serve ${UNIQNODE_DATA_DIR} ${UNIQNODE_LISTEN} $UNIQNODE_SERVE_OPTIONS
```

## system 単位で起こす

第一の道は `uniqnode install --system` である。root で走らせ(sudo)、unit は
/etc/systemd/system/ に置かれ、常駐は sudo を打った利用者(SUDO_USER。`--user <name>` で
別の利用者を指せる。root は断る)で走る。既定の置き場(バイナリ ~/.local/bin/uniqnode、
写し先 ~/uniqnode-backup/<インスタンス>)はその利用者の home の下で、root の home ではない。出力は
ファイルに残す形で渡す(CLAUDE.md):

```
sudo /home/hikalium/.local/bin/uniqnode install /work2/llm_playground_host_dir/uniqnode-store --system --user hikalium --serve-options "--embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank" --viewer-listen 0.0.0.0:7450 --listen-agent 10.10.128.1:7441 --agent-writable lamalium-notes --agent-collections articles --agent-collections papers --agent-collections seccamp --agent-collections specs --agent-collections trial --agent-collections web --agent-collections lamalium-notes --after wg-quick@wg1.service --firewall-allow 10.10.128.4,10.10.128.2 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee /tmp/uniqnode-install-system.log
```

(本番の実物。ストアは /work2 の下、読み口は wg1 のアドレス 10.10.128.1:7441、読み口から
書けるのはコレクション lamalium-notes の 1 つ(lamalium 側の書く先は共有 1 つで、名は
lamalium-notes。操作者の裁定 2026-09-05 と 2026-09-06)、読めるのは現在の 7 コレクション
全部(articles・papers・seccamp・specs・trial・web・lamalium-notes。機構として絞れる形に
しておき、値は全部という操作者の裁定 2026-09-06。コレクションが増えたらこの行に足す)、
wg1 の unit は wg-quick@wg1.service。既に user 単位で常駐しているなら、先に下の
「user 単位から移る」。)

user 単位と違うのは、unit の中身(docs/mop/systemd/system/。User=/Group= を持ち、閉じ込めは
PrivateUsers= なしで効く)と、drop-in に足す行、確認の走らせ方である:

- drop-in の `[Service]` の先頭に `User=<name>` と `Group=<主グループ>`(unit の専用ユーザー
  uniqnode を実行ユーザに替える)、`StateDirectory=`(空で打ち消す。ストアは明示なので
  %S は要らず、残すと /var/lib/uniqnode を無駄に作る)。バイナリ・ストア・写し先のどれかが
  /home・/root・/run/user の下なら `ProtectHome=read-only`(unit の yes は home を空に隠す
  ので、その下の ReadWritePaths= も ExecStart= も届かない。read-only なら user 単位の unit と
  同じで、ReadWritePaths= が効く)。
- `--after <unit>`(複数可)は drop-in の `[Unit]` に `After=` と `Wants=` で書く(3 つの
  service とも)。読み口を wg1 のアドレスに束縛するときは wg-quick@wg1.service を待つ
  (10.10.128.1 は wg1 が上がって初めて存在する)。user 単位では断る: user 単位のマネージャは
  機械の unit を知らないので、user unit は system unit を待てない。
- 外部の道具(pdftotext・curl)は install 自身の PATH に加えて、実行ユーザの ~/.local/bin と
  ~/bin でも探す(sudo は PATH を secure_path に替えるので、利用者が ~/.local/bin に置いた
  pdftotext は root の PATH に無い)。報告の PATH の行が「探したのは …」で探した並びを言う。
- linger は取らない(機械と共に起きる)。
- 確認の backup は `systemctl start uniqnode-backup@default.service`(unit は User= で走る)、fsck は
  `runuser -u <name> -- <bin> fsck <写し先>` で実行ユーザとして走らせる(root のプロセスで
  写し先を開くと、未封印の尻尾の切り詰めで root 所有のファイルができる)。install が root で
  作ったもの(バイナリの写し、無ければ作るストアと写し先のディレクトリ)は実行ユーザの所有に
  する。最後にストアと写し先の木を歩き、実行ユーザ以外の所有のものが 1 つでもあれば赤で
  止まる: 据え付けの後にストアと写し先に root 所有のものは残らない。

root で走らせないと、sudo で走らせる形と tee の例を言って断る。ストアを別のプロセスが開いて
いれば(user 単位の serve が典型)、止めて外す命令を添えて断り、黙って止めはしない。

### 旧い名の unit から移る

vega の主のストアは、テンプレートになる前(2026-09-07)に据えた旧い名の unit で動いている:
`uniqnode-serve.service`・`uniqnode-viewer.service`・`uniqnode-backup.service` と
`uniqnode-backup.timer`(2026-10-01 に `systemctl cat` と /etc/systemd/system/ の一覧で確かめた。
graph_a・graph_b は既に `@` の名)。install は既定のインスタンスを据えるとき、旧い名が据え先に残って
いれば何も置かずに断る。旧い unit と `uniqnode-serve@default.service` が同じ主の口(127.0.0.1:7440)と
同じストアを取り合い、後から起きた方が束縛かストアのロックで exit 1 になって、どちらが動くかが起動の
順で決まってしまうためである。黙って外しはしない(操作者のものを止めるのは操作者の判断。must/0022)。
下の「テンプレートになる前の名の unit から移る」が同じ規則の一般形で、ここは vega の実物に当てた手順。

移らないと困るのは、これからの変更がテンプレートにだけ入るからである。API_AUTH の A2 は
`uniqnode-serve@.service` の `RestrictAddressFamilies=` に `AF_NETLINK` を足し、APPEND_FAILURE の S1 は
`ExecStop=` を足す。旧い名のままバイナリだけを差し替えると、A2 では照会のソケットを作れず主の口の全ての
接続が 403 になり、S1 では restart のたびに印に `Running` が残りうる。install の打ち直し(下の「更新」)も
旧い名が残る限り断られる。したがってこの移行は、A2 と S1 を本番へ入れる前提である
([docs/plan/API_AUTH.md](#abde9b3c-75f8-453b-988e-bfb1e178c771)・
[docs/plan/APPEND_FAILURE.md](#d973833f-4e2b-4fc8-8a49-42f6821b6a7a) の段取り)。

旧い unit が持っているもの(外すと消えるので、据え直す install の引数で全部作り直されることを確かめて
から外す):

- `uniqnode-serve.service.d/override.conf`: 本番の引数(ストアの道、`--embed`・`--rerank`、読み口
  10.10.128.1:7441、`--agent-writable lamalium-notes`、`--agent-collections` の 7 つ、
  `After=wg-quick@wg1.service`)。下の「user 単位から移る」の install の行が同じ値を持つ。
- `uniqnode-serve.service.d/agent-door.nft`: 表 `inet uniqnode` で、読み口へ届いてよいのは
  10.10.128.4 と 10.10.128.2(crystal。2026-09-29 に手で足した)。install の `--firewall-allow
  10.10.128.4,10.10.128.2` が表 `inet uniqnode_default` として作り直す。同じ場所の
  agent-door.nft.bak-20260929 は手で足す前の写しで、一緒に消える。
- `uniqnode-viewer.service.d/override.conf`: viewer の 0.0.0.0:7450。install の `--viewer-listen` が持つ。
- `uniqnode-backup.service.d/override.conf`: 写し先 /home/hikalium/uniqnode-backup(名の付かない道)。
  install の写し先の既定は /home/hikalium/uniqnode-backup/default で、そのままでは初回が全件の写しに
  なる(下の「2 つ目のストアを同じ機械で」)。増分を続けるため、下の「user 単位から移る」の install の行は
  `--backup-dir /home/hikalium/uniqnode-backup` を持つ。

外す命令は vega で打つ。sudo を使えるどの利用者がどこから貼っても同じに動く。止めて外す行は install の
断りが添える命令と同じ字句(install.rs の `legacy_units_removal_command`。must/0023。node/tests/repo_hygiene.rs
がこの文書に同じ字句があることを確かめる)で、その前に次を行う(第 17 版の APPEND_FAILURE への Codex と
Claude のレビュー):

- 旧い timer を止め、走っている旧い `uniqnode-backup.service` の終わりを待つ。timer を止めても実行中の
  service は止まらず、install が起こす新しい backup と同じ写し先へ並んで書きうる(backup はコピー全体を
  ロックせず、pack の削除と MANIFEST の更新もする)。30 分たっても終わらなければ、何も外さずに止まる
  (timer は止まったままなので、後でこの命令を打ち直すか、`systemctl start uniqnode-backup.timer` で戻す)。
- 外す行が消す 8 つの名のうち、在るものを全部、毎回別の名の退避のディレクトリ
  (/var/backups/uniqnode-legacy-<時刻>/units.tar.gz)へ写し、`tar -tzf` で全部が入っていることを確かめてから
  外す。退避の道は記録に 1 行残す。退避は /var/backups に置く(/var/tmp は systemd-tmpfiles が 30 日で掃除
  するので、後で戻すときに消えている。第 18 版の APPEND_FAILURE への Claude のレビューの低 7)。
  /var/backups は tmpfiles の掃除の対象ではなく、退避は操作者が消すまで残る。
- tar か確かめが失敗したら、その回の退避のディレクトリを消してから止まる(不完全な退避を残さない。同じ
  レビューの低 6)。一覧との照らし合わせは正規表現でなく字句で行う(`grep -qxF`。同じレビューの低 8)。
  旧い timer を止めた後のどの止まり方でも(backup の待ちの期限、退避の名の重なり、tar・確かめ・バイナリの
  写しの失敗、固定の壊れ、`set -e` で止まる予期しない失敗)、止まる文の後に `sudo systemctl start
  uniqnode-backup.timer` で戻す命令を添える。timer を止めた直後に `EXIT` の trap を掛けて、0 でない終わりの
  ときに案内を出す(APPEND_FAILURE 第 21 版への Codex と Claude のレビューの Claude 低)。案内は外す行
  (`disable --now`・削除・daemon-reload)が通った後に空にし、その行の間は、disable が一部だけ済んだ形を考えた
  案内に替える: 退避したものが全部残っていれば `start` でなく `enable --now` で戻し(`start` では disable 済みの
  ものが次の起動から戻らない)、一部でも消えていれば打ち直して外し終える(APPEND_FAILURE 第 22 版への Codex と Claude の
  レビューの Codex 低 7。第 22 版は外す行の前に案内を空にしていて、その行の失敗を案内しなかった)。
- 共有のバイナリ /home/hikalium/.local/bin/uniqnode も同じ退避のディレクトリへ写す(同じレビューの中 4)。
  install の据え付け(install.rs の 1216〜1255 行付近の `install_binary`)はバイナリを rename で置き換え、
  前のものを残さないので、unit だけを戻すと旧い unit が新しいバイナリを走らせる(API_AUTH の A2 のバイナリ
  なら AF_NETLINK が無く、主の口が全部 403 になる)。戻す命令はこのバイナリも据え直す。
- 最初に確かめ終えた退避を /var/backups/uniqnode-legacy-pinned(退避のディレクトリへの symlink)で固定し、
  戻す命令はこれだけを使う(最新の退避ではない)。`rm -rf` の途中で止まった後にこの命令を打ち直すと、残った
  一部だけの退避が新しく作られるが、固定は既に在るので上書きしない(第 18 版の APPEND_FAILURE への Codex の
  レビューの中 7)。固定が既に在るときは、何かを外す前に、戻す命令が使える形かを検める: 解決先が在る
  ディレクトリであること(`[ -L ]` はリンク切れでも真なので、`readlink -e` で解決する)、その units.tar.gz を
  読めて uniqnode-serve.service が入っていること、その uniqnode が実行できるファイルであること。どれかが
  欠ければ、今回の退避(確かめ済み)を残したまま何も外さずに止まる(第 19 版の APPEND_FAILURE への Codex と
  Claude のレビューの Codex 中 6)。固定をどう直すか(壊れた固定を消して今回の退避を固定し直すか)は、
  操作者が記録の 2 つの道を見て決める。この止まり方では旧い timer が止まったままなので、止まる文は
  `sudo systemctl start uniqnode-backup.timer` で戻す命令を添える(固定を直すまでの間も毎日の backup を
  続けるため。第 20 版の APPEND_FAILURE への Codex と Claude のレビューの Claude 低 7)。添えるのは上の
  trap で、他の止まり方と同じ 1 箇所から出る。
- nft の表は在るときだけ消す。在るかは `nft list tables` の答えで決め、その一覧の取得が失敗したら(nft が
  無い、照会の誤り)止まる。第 18 版の `if nft list table …` は失敗も「表が無い」と扱い、完了の文と終了
  コード 0 を出していた(同じレビューの中 8)。外した後に nft や install が失敗して打ち直しても、前の退避を
  上書きせず、旧い unit が既に無ければ退避と外しを飛ばして先へ進む。
- 外側のパイプラインにも `pipefail` を掛け、最後に終了コードを記録へ足す。内側の `set -euo pipefail` は
  外側の `sudo … | ts | tee` には効かず、tee が成功すれば失敗が隠れるため。外側の全体は `( … )` の
  サブシェルで囲み、`pipefail` が操作者の対話のシェルに残らないようにする(同じレビューの低 9)。`echo` の
  `$?` は、同じサブシェルの中の直前のパイプラインの終了コード(`pipefail` の下なので、sudo の側の失敗を含む)
  である。

```
( set -o pipefail; sudo bash -s <<'EOF' 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee -a /tmp/uniqnode-legacy-units.log; echo "exit status: $?" | /usr/bin/tee -a /tmp/uniqnode-legacy-units.log )
set -euo pipefail
cd /etc/systemd/system
pin=/var/backups/uniqnode-legacy-pinned
binary=/home/hikalium/.local/bin/uniqnode
targets=(uniqnode-serve.service uniqnode-serve.service.d uniqnode-viewer.service uniqnode-viewer.service.d uniqnode-backup.service uniqnode-backup.service.d uniqnode-backup.timer uniqnode-backup.timer.d)
present=()
for t in "${targets[@]}"; do if [ -e "$t" ]; then present+=("$t"); fi; done
if [ "${#present[@]}" -eq 0 ]; then
  echo "旧い名の unit は既に無い。退避と外しを飛ばす"
else
  systemctl stop uniqnode-backup.timer || true
  timer_hint="旧い timer は止めたままである。直すまでの間も毎日の backup を続けるなら sudo systemctl start uniqnode-backup.timer で戻す"
  trap 'rc=$?; if [ "$rc" -ne 0 ] && [ -n "${timer_hint:-}" ]; then echo "$timer_hint"; fi; exit "$rc"' EXIT
  waited=0
  while :; do
    state=$(systemctl is-active uniqnode-backup.service || true)
    case "$state" in inactive|failed) break ;; esac
    if [ "$waited" -ge 1800 ]; then echo "旧い backup が 30 分たっても終わらない(状態: $state)。何も外さずに止める"; exit 1; fi
    echo "旧い backup の終わりを待つ(状態: $state)"
    sleep 10; waited=$((waited + 10))
  done
  saved=/var/backups/uniqnode-legacy-$(/usr/bin/date +%Y%m%dT%H%M%S%z)
  if [ -e "$saved" ]; then echo "$saved が既に在る。上書きしない"; exit 1; fi
  /usr/bin/mkdir -m 0700 "$saved"
  if ! /usr/bin/tar -czf "$saved/units.tar.gz" "${present[@]}"; then /usr/bin/rm -rf "$saved"; echo "退避の tar が失敗した。退避を消し、外さずに止める"; exit 1; fi
  if ! listing=$(/usr/bin/tar -tzf "$saved/units.tar.gz"); then /usr/bin/rm -rf "$saved"; echo "$saved/units.tar.gz を読めない。退避を消し、外さずに止める"; exit 1; fi
  for t in "${present[@]}"; do /usr/bin/grep -qxF -e "$t" -e "$t/" <<<"$listing" || { /usr/bin/rm -rf "$saved"; echo "退避に $t が無い。退避を消し、外さずに止める"; exit 1; }; done
  if ! { /usr/bin/cp -p "$binary" "$saved/uniqnode" && /usr/bin/cmp -s "$binary" "$saved/uniqnode"; }; then /usr/bin/rm -rf "$saved"; echo "$binary を退避へ写せない。退避を消し、外さずに止める"; exit 1; fi
  echo "退避: $saved (${present[*]} と $binary)"
  if [ -e "$pin" ] || [ -L "$pin" ]; then
    if ! pinned=$(/usr/bin/readlink -e "$pin") || [ ! -d "$pinned" ]; then echo "固定 $pin が壊れている(解決先が無いか、ディレクトリでない)。何も外さずに止める。今回の退避 $saved は残す"; exit 1; fi
    if ! pinned_listing=$(/usr/bin/tar -tzf "$pinned/units.tar.gz"); then echo "固定した退避 $pinned/units.tar.gz を読めない。何も外さずに止める。今回の退避 $saved は残す"; exit 1; fi
    /usr/bin/grep -qxF uniqnode-serve.service <<<"$pinned_listing" || { echo "固定した退避 $pinned/units.tar.gz に uniqnode-serve.service が無い。何も外さずに止める。今回の退避 $saved は残す"; exit 1; }
    [ -f "$pinned/uniqnode" ] && [ -x "$pinned/uniqnode" ] || { echo "固定した退避 $pinned/uniqnode が無いか実行できない。何も外さずに止める。今回の退避 $saved は残す"; exit 1; }
    echo "固定した退避は既に在り、戻す命令が使える形である: $pin -> $pinned。今回の退避は固定しない"
  else
    /usr/bin/ln -sT "$saved" "$pin"
    echo "固定した退避: $pin -> $saved"
  fi
  timer_hint="旧い unit を外す行(disable --now・unit のファイルの削除・daemon-reload)の途中で止まった。disable が一部だけ済んでいることがある。記録の「退避:」の行に並ぶものが /etc/systemd/system に全部残っていれば、start でなく sudo systemctl enable --now uniqnode-serve.service uniqnode-viewer.service uniqnode-backup.timer で戻す(start では、disable 済みのものが次の起動から戻らない)。一部でも消えていれば、この命令を打ち直して外し終え、下の install へ進む(戻すなら下の戻す命令で)"
  systemctl disable --now uniqnode-serve.service uniqnode-viewer.service uniqnode-backup.timer ; rm -rf /etc/systemd/system/uniqnode-serve.service /etc/systemd/system/uniqnode-serve.service.d /etc/systemd/system/uniqnode-viewer.service /etc/systemd/system/uniqnode-viewer.service.d /etc/systemd/system/uniqnode-backup.service /etc/systemd/system/uniqnode-backup.service.d /etc/systemd/system/uniqnode-backup.timer /etc/systemd/system/uniqnode-backup.timer.d ; systemctl daemon-reload
  timer_hint=""
fi
tables=$(/usr/sbin/nft list tables)
if /usr/bin/grep -qxF 'table inet uniqnode' <<<"$tables"; then /usr/sbin/nft delete table inet uniqnode; fi
echo "旧い名の unit を外し終えた"
EOF
```

注意:

- 外してから据え直すまでの間、主の口 7440・読み口 7441・viewer 7450 は止まり、毎日の backup の刻みも
  無い(graph_a・graph_b は別の unit なので動き続ける)。lamalium の読み口への書き込みはこの間
  失敗する。
- 順は、この外す命令 → 下の「user 単位から移る」の install の命令、でなければならない。逆に打つと、
  install は旧い名が残っているので何も置かずに断る(壊れはしないが、据わらない)。外した後は間を空けず
  に install の命令を打つ。ビルドを先に済ませておけば、install の命令の中の cargo build はすぐ終わり、
  止まる間が短くなる。
- vega では、graph_a・graph_b を含むどのインスタンスの install も、この移行の後に打つ(第 17 版の
  APPEND_FAILURE への Claude のレビューの中 5)。旧い `uniqnode-serve.service` と graph_a・graph_b の serve は
  同じ /home/hikalium/.local/bin/uniqnode を走らせ、install は既定でないインスタンスでは旧い名を残した
  まま共有のバイナリを差し替える(install.rs の 2087〜2096 行付近)。先に graph_* を据え直すと、旧い
  serve の次の起動が新しいバイナリを旧い unit で走らせる(API_AUTH の A2 なら AF_NETLINK が無く主の口が
  全部 403、APPEND_FAILURE の S1 なら ExecStop が無い)。install がこの形を断るようにする直しは、
  API_AUTH の A1 か APPEND_FAILURE の S1a(S1 の最初の段)のうち先に入る方に含める。
- 済んだかは、/tmp/uniqnode-legacy-units.log の最後が「旧い名の unit を外し終えた」と
  `exit status: 0` であることで確かめてから、install の命令へ進む(記録は追記なので、打ち直した分も
  時刻つきで残る)。続く /tmp/uniqnode-install-system.log も、最後の `exit status: 0` を確かめる。
- 戻すときは、固定した退避(/var/backups/uniqnode-legacy-pinned。最新の退避ではない)を使う。退避に
  uniqnode-serve.service とバイナリが入っていることを、何かを変える前に確かめる(第 18 版の APPEND_FAILURE への
  Codex のレビューの中 7、Claude のレビューの低 6)。共有のバイナリを退避のものへ据え直し(同じレビューの中 4)、
  unit の退避を /etc/systemd/system に展開して daemon-reload し、旧い 3 つを enable --now する(同じ主の口を
  取り合うので、`@default` と同時には置かない)。バイナリは graph_a・graph_b の serve と共有なので、それらも次の
  起動から移行の前のバイナリで走る(移行の前と同じ形)。
  S1b より前のバイナリの `Store::open` は `open-marker` を見ずに recover して書くので、新しいバイナリが残した
  `Io` や同じ boot_id の `Running` の上でそれを起こすと、保留を素通りして、page cache にだけ在る完全なレコードを
  採ってその後ろへ書く道が戻る。そのため差し替えの前に、共有のバイナリを走らせるものを全部止め、それらが使う
  ストアの保留を検める。第 22 版の APPEND_FAILURE までは、この検めを命令が知っている unit とストアの一覧に
  寄せていて、レビューのたびに一覧の漏れと打ち直しの穴が見つかった。その第 22 版への Codex と Claude のレビューを
  受けて、命令を次の原則で組み直した(指摘ごとに直すのでなく、一覧に頼らない形にする)。第 23 版への Codex と
  Claude のレビューで、門を読めないプロセスに対して閉じる形にし、止める前にも門を通し、一回走る service を
  起こし直す一覧から外し、止まったときの案内を段ごとに元へ戻せる形にした。
  - 止める範囲: /etc/systemd/system の `uniqnode-*` の unit と drop-in のディレクトリと、systemd が読み込んで
    いる `uniqnode-*` の unit の和(テンプレートそのものは除く)を全部止める。2 つの一覧は別々に取り、どちらかの
    取得が失敗したら何も変えずに止まってから和を作る(第 23 版は `{ find …; systemctl …; } | …` を 1 つの
    コマンド置換で取り、前半の失敗が後半の成功に隠れた。Codex 中 4)。service は `systemctl show -p
    Type,RemainAfterExit` で分ける。`Type=oneshot` で `RemainAfterExit=no` のもの(backup の旧い名と `@` の全
    インスタンス、graph-pull、ingest-git などの一回走るもの)は終わりを待ち(合わせて 30 分で止まる)、起こし直す
    一覧に入れない(timer が次に起こす)。残りの service(serve と viewer。前の回に戻した旧い名の
    `uniqnode-serve.service`・`uniqnode-viewer.service` も含む)は止める。timer を先に止める。`@default` の 3 つは
    起こし直さず disable する。どれが据わっているかは、/etc/systemd/system/<unit>.d(timer は
    `uniqnode-backup@default.service.d`)が在るか、`systemctl is-enabled` が真かで決める。テンプレートの有無では
    決めない: vega には graph の据え付けで既にテンプレートが在り、`@default` が据わっていなくても真になる
    (Claude 低 3)。
  - 止める前の門: 何かを止める前に、下のプロセスでの門と同じ規則で /proc の全プロセスを調べる。/proc/<pid>/cgroup
    が `uniqnode-*.service`(system.slice の直下か、テンプレートのインスタンスの `system-uniqnode…slice` の下)に
    在るものは、この命令が止めるので除く。残りに、共有のバイナリ・退避のバイナリ・検め手(候補を含む)のどれかを
    走らせるものか、実行ファイルを確かめられない生きたプロセスがあれば、pid と理由を出し、unit を何も止めず、
    バイナリも差し替えずに止まる。vega でいちばんありそうな断りは Claude Code のセッションが起こした mcp の
    Local(--serve-url 付き)で、2026-10-01 に利用者の権限で走らせた読むだけの写しでも、一致したのはこれ 1 つ
    だけだった。第 23 版ではこれが、
    全部を止めて `@default` を disable した後の門で初めて当たり、その段には元へ戻す案内が無かった(Claude 中 2)。
  - プロセスでの門: /proc/*/exe を readlink で解き、その道(末尾の「 (deleted)」は外す。rename で置き換えられた前の
    inode を走らせているもの)か、`stat -L -c %d:%i` で読んだ inode が、4 つのバイナリのどれかと一致するものを
    数える。inode でも比べるのは、別のマウントの名前空間(コンテナなど)で同じファイルを別の道として走らせるものを
    拾うため。readlink が失敗したときは、消えた(`[ -d /proc/<pid> ]` が偽)・ゾンビ(/proc/<pid>/stat の状態が
    Z)・カーネルのスレッド(同じ stat の flags に PF_KTHREAD の 0x00200000、か cmdline が空で親が 2)のどれかと
    確かめられたときだけ除き、それ以外は「実行ファイルを確かめられない生きたプロセス」として pid と理由を出して
    断る。inode を読めない生きたプロセスも同じ。第 23 版の `readlink … || continue` は、読めない生きたプロセスを
    該当なしと扱った(Codex 高 1、Claude 低 1)。命令は root で走るので、ふつうはどのプロセスの exe も読める。
    unit の外の持ち主の典型は、mcp の Local(Claude Code などのクライアントが起こす。共有のバイナリが変わると
    自分を exec で差し替えるので、旧いバイナリへ替わって保留を素通りしうる)と、手で打った CLI(取り込み・gc・
    fsck など)で、命令はそれらを止めない(操作者のもの。must/0022)。vega では uniqnode の作業ツリーで開いた
    Claude Code のセッションのたびに Local が 1 つ起きるので、戻す前に vega のそのセッションを全部閉じる。止めた
    後にも門を通し(止める間に起きたもの、unit の cgroup の外へ逃れたものを拾う)、差し替えの直前にもう一度通す
    (検めの間に起きたものを拾う。それより後の短い窓は残る)。一覧に無いストアを開く Local・CLI(Codex 高 2)も、
    前の回に戻して起きている旧い serve(Codex 中 5)も、これで一覧に頼らずに止まる。
  - 共有のバイナリでない書き手: 別のプロジェクトの MCP の設定が、`--serve-url` の無い target/debug/uniqnode mcp
    を本番のストアへ起こすことがある。共有のバイナリでないので門を素通りするが、ストアのロックを持つので、検めで
    hold-status が 1 を返すか、差し替えの後に旧い serve が起きない。どちらの案内もこの形を言う。戻す前にそれも
    閉じる。
  - 検めるストア: 止めた範囲の service のうち `UNIQNODE_DATA_DIR` を持つもの(serve・viewer・backup の各
    インスタンスと旧い名)の全部と、既定のストア。serve・viewer・backup の unit で読めなければ止まる。門で
    見つけたプロセスのストアは、`uniqnode <command> <data_dir>` の第 2 引数(相対なら /proc/<pid>/cwd から)が
    ディレクトリに解ければ、状態のディレクトリの stores(NUL 区切り)に残して、打ち直したときに検める(止める前の
    門でも残す)。決められないときは門で断ったことが守りで、止めた後にそのストアを自分で hold-status で検めるよう
    言う。
  - `UNIQNODE_DATA_DIR` の読み方: install は drop-in に変数ごとに 1 行の `Environment=` を書き、
    値が英数字と `/._-:=+,@~` だけなら裸で、それ以外は全体を `"` で囲んで `\` と `"` を `\` で逃がす
    (`%` は `%%`。install.rs の `unit_word` と `environment_line`)。systemd 249 の `systemctl show -p Environment
    --value` は要素を空白で繋ぎ、空白などを含む要素だけを `"…"` で囲んで `\`・`"`・`$`・`` ` `` を `\` で
    逃がす。第 22 版の `tr ' '` は空白を含む道を壊し(Codex 中 6)、`xargs` は `"…"` の中の `\` を外さないので
    `$` や `"` を含む道を誤る。そこで `busctl --json=short` で unit の `Environment` を D-Bus の文字列の配列の
    まま JSON で受け、`jq` で `UNIQNODE_DATA_DIR=` で始まる要素の最後(後の代入が勝つ)の値を取る。名は要素の
    頭との字句の一致で決め、他の変数の値の中の `UNIQNODE_DATA_DIR=` は拾わず、eval もしない。値は systemd が
    `%` の指定子を解いた後のもの。
  - 道の検め: 各ストアを `readlink -e` で解き、`[ -d ]` でディレクトリと確かめ、`find -H` で `open-marker` を
    探す。解けない・ディレクトリでない・find の失敗のどれでも、「印が無い」と扱わずに止まる。第 22 版は
    symlink の道で find が開始点を辿らず、在る印を見落として検めを飛ばしえた(Codex 高 1・Claude 中 2)。
    同じ実体は 1 度だけ検める。
  - 検めの走らせ方: hold-status はストアの持ち主として走らせる(第 22 版は常に hikalium。Claude 低 4)。
    持ち主は unit の User=(空なら root)で、ストアのディレクトリの持ち主と食い違えば止まる(install は
    据え付けの後にストアを実行ユーザの所有にする)。プロセスから得たストアはディレクトリの持ち主。どのストアにも
    印が無ければ(S1b 以後のバイナリが開いたことが無い)保留は無いので検め手なしで進み、1 つでも印があれば
    全部のストアに検め手の `uniqnode hold-status <ストア>`(APPEND_FAILURE の方針 5。印を読むだけで書かない。
    main.rs の `uniqnode <command> <data_dir> [args]` の形)を打ち、全部が 0 のときだけ差し替える。
  - 検め手: 戻す前のバイナリは差し替えの後には消え、退避の旧いバイナリは `hold-status` も `release-hold` も
    知らないので、退避の外に検め手を持つ。操作者が /var/backups/uniqnode-legacy-checker に置いたもの(か前の回に
    採ったもの)を先に使う。今のバイナリは、`[ -x ]` で在ると確かめ、退避のものと違う(`cmp -s`)ときだけ候補
    /var/backups/uniqnode-legacy-checker.candidate へ写す(止める前の門を通った後)。同じなら「前の回で差し替え済みか、
    移行の後に一度も差し替えていない」と記録に 1 行残して写さない(Claude 低 5)。検め手が hold-status を知ると
    見なすのは、少なくとも 1 つのストアに 0 か 3 で答えたときだけである。1 は hold-status の誤りと sudo 自身の
    失敗を区別できず、2・126・127 は知らない版か走らない。検め手が知らなければ候補を試し、候補が知ると確かめられたら、
    検め手の道が空いているときだけそこへ移して採る。操作者が置いた検め手は、知らない版でも上書きしない
    (第 22 版は打ち直しのたびに今のバイナリで上書きしていた。Codex 中 3)。どちらも知らなければ、S1b 以後の
    ビルドを検め手の道へ置くよう言って止まる。
  - 0 でないときは何も差し替えずに止まり、終了コードごとに戻し方を言う。3(保留): 同じ boot_id の `Running`
    と `Io` も検めを通らない印も、ホストを再起動するか、ストアを置いたファイルシステムを umount・fsck・mount
    し直して page cache を捨てた後でなければ release-hold しない(方針 5 の前提)。同じ boot_id の `Running` と
    `Io` は再起動だけで解け、検めを通らない印は再起動の後も release-hold か検め済みの backup からの復元が要る。
    ホストの再起動は手で起こした llama-server(8082〜8084)も止める(APPEND_FAILURE の「最終目標とのつながり」の
    代価)。再起動では enable された graph の serve と timer が今のバイナリで起きるが、差し替える前なので
    構わない(打ち直した命令がまた止める)。1(検められない): 記録の直前の stderr の行を読む。2 ほか: 同じく
    直前の行を読んで直す。
  - 起こし直す unit の状態: 止める前に、今動いている timer と常駐の service(`@default` を除く)を、状態の
    ディレクトリ /var/backups/uniqnode-legacy-rollback-state の restart へ、前の回の分との和で書き(tmp から
    rename)、enable されている `@default` を同じく default-enabled へ和で書いてから、止めて disable する
    (Claude 中 2)。restart を読むときに、読み込まれない unit と一回走る service を外して書き直す(第 23 版は
    activating の `uniqnode-ingest-git@lamalium.service` のような oneshot を restart に入れ、最後の確かめで
    active を求めたので、正しく終わっても完了できず、打ち直しても同じだった。Codex 中 2)。打ち直しても、前の回に
    止めて今は止まっているもの(graph の serve と viewer、graph-pull と graph の backup の timer)を落とさない
    (Codex 中 4・Claude 中 1)。最後に旧い名の 3 つを enable --now し、restart の全部を start し、全部が active に
    なったこと(1 つにつき 60 秒まで待つ)を確かめてから状態のディレクトリを消し、完了を言う。起きないものが
    あれば状態を残して止まる。
  - 古い記録の外し方(Claude 低 2): stores には、もう無いプロセスが開いていた一時のストア(/tmp の下など)や
    ストアでない道が残りうる。前の回の門が残した道だけから来たストアで断るとき(解けない・ディレクトリでない・
    照会の誤り・hold-status の 1)は、その道を stores から外す命令(`grep -zvxF` で書き直して rename する
    `sudo /usr/bin/bash -c '…' _ <道> <stores>`。道は `printf %q` で引用する)を添える。restart の unit が
    起きないときも、その unit を restart から外す同じ形の命令を添える。どちらも、もう要らないと操作者が
    確かめてから打つ。
  - 止まったときの案内: `EXIT` の trap が段に応じた案内を出す。案内の命令は全部、記録へ追記する形
    (`… 2>&1 | tee -a /tmp/uniqnode-legacy-rollback.log` と `exit status:`)である。
    - 止める前の門で断ったとき(何も止めていない)と、止めた後・差し替えの前: 据わっているバイナリが退避のもの
      と違い(`cmp -s`)、この命令が始まったときと同じ(`stat` の装置・inode・大きさ・時刻)なら、それは止めた
      unit が走らせていたものなので、元の形へ戻す命令を添える: default-enabled の `@default` を enable --now し、
      restart を start し、`systemctl is-active` で全部が active と確かめてから状態のディレクトリを消す。同じ
      バイナリで起こし直すので systemctl restart と同じ形で、保留を素通りさせない(S1b 以後のバイナリなら保留で
      開き、S1b より前のバイナリなら印を残していない)。第 22 版で「今のバイナリで起こす」案内を検め手と同じ版に
      限った(Codex 高 2)のは、始まったときと違うバイナリを起こしうる形だったからで、ここでは始まったときと同じ
      ことを確かめて添える。退避のものと同じなら(前の回に差し替え済み)、次の 2 つの案内を出す。
    - 差し替えの後で、旧い名の unit がまだ /etc/systemd/system に無いとき: 先に `sudo install -m 0755 -o hikalium
      -g hikalium <検め手> /home/hikalium/.local/bin/uniqnode` で S1b 以後へ戻してから、元の形へ戻す命令と同じ
      ものを打つ。
    - 旧い名の unit を展開した後(前の回に展開した後の打ち直しで、どの段で止まったときも含む): S1b 以後の
      バイナリへ戻し、上の外す命令で旧い名を外し、下の「user 単位から移る」の install の命令を打ち、その後に
      restart(旧い名の 3 つを除く)を restart して active と確かめてから状態を消す。第 23 版は状態を先に消すよう
      言い、install が起こさない graph の serve・viewer と timer の記録を失った(Codex 中 3・Claude 中 1)。
    - S1b 以後のバイナリとして名を出すのは、この回に少なくとも 1 つのストアへ 0 か 3 で答えた検め手だけで、
      無ければ `<確認済みの S1b 以後のビルド>` と書き、検め手か候補を使うなら先に hold-status で確かめるよう
      添える(第 23 版は確かめていない候補を名指した。Codex 低 5)。
  - 打ち直し: どこで止まっても同じ命令を打ち直せばよい。止める・待つ・disable は止まっているものには何もせず、
    restart と default-enabled は和で残り、検め手は上書きされず、差し替えと展開は同じものを置き直すだけである。
    前の回で旧い名の unit を戻した後なら、その旧い serve・viewer・backup の timer も止める範囲に入り、走っている
    旧い backup の終わりを待つ。止まっている間は、主の口 7440・読み口 7441・viewer 7450、graph の口 7442〜7445
    (lamalium-plan)と backup の timer が止まり、断りの後に打ち直しが通るまで続く(APPEND_FAILURE の「最終目標との
    つながり」の代価)。
  vega で打つ命令:

```
( set -o pipefail; sudo bash -s <<'EOF' 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee -a /tmp/uniqnode-legacy-rollback.log; echo "exit status: $?" | /usr/bin/tee -a /tmp/uniqnode-legacy-rollback.log )
set -euo pipefail
binary=/home/hikalium/.local/bin/uniqnode
checker=/var/backups/uniqnode-legacy-checker
candidate=/var/backups/uniqnode-legacy-checker.candidate
state=/var/backups/uniqnode-legacy-rollback-state
default_store=/work2/llm_playground_host_dir/uniqnode-store
log=/tmp/uniqnode-legacy-rollback.log
old_units=(uniqnode-serve.service uniqnode-viewer.service uniqnode-backup.timer)
if ! saved=$(/usr/bin/readlink -e /var/backups/uniqnode-legacy-pinned); then echo "固定した退避 /var/backups/uniqnode-legacy-pinned が無い。何も変えずに止める"; exit 1; fi
echo "戻す退避: $saved"
listing=$(/usr/bin/tar -tzf "$saved/units.tar.gz")
/usr/bin/grep -qxF uniqnode-serve.service <<<"$listing" || { echo "$saved/units.tar.gz に uniqnode-serve.service が無い。何も変えずに止める"; exit 1; }
if [ ! -f "$saved/uniqnode" ] || [ ! -x "$saved/uniqnode" ]; then echo "$saved/uniqnode が無いか実行できない。何も変えずに止める"; exit 1; fi
binary_id=$(/usr/bin/stat -L -c %d:%i:%s:%Y -- "$binary" 2>/dev/null || true)
unit_env() {
  local path
  path=$(/usr/bin/busctl --json=short call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager LoadUnit s "$1" | /usr/bin/jq -r '.data[0]') || return 1
  /usr/bin/busctl --json=short get-property org.freedesktop.systemd1 "$path" org.freedesktop.systemd1.Service Environment | /usr/bin/jq -r --arg k "$2=" '[.data[] | select(startswith($k))] | last // empty | .[($k | length):]' || return 1
}
unit_kind() {
  local type rae
  case "$1" in *.timer) kind=timer; return 0 ;; esac
  type=$(systemctl show -p Type --value "$1") || return 1
  rae=$(systemctl show -p RemainAfterExit --value "$1") || return 1
  if [ "$type" = oneshot ] && [ "$rae" = no ]; then kind=oneshot; else kind=daemon; fi
}
if ! fs_units=$(/usr/bin/find /etc/systemd/system -mindepth 1 -maxdepth 1 \( -name 'uniqnode-*.service' -o -name 'uniqnode-*.timer' -o -name 'uniqnode-*.service.d' -o -name 'uniqnode-*.timer.d' \) -printf '%f\n'); then echo "/etc/systemd/system の uniqnode-* を列挙できない。何も変えずに止める"; exit 1; fi
if ! sd_units=$(systemctl list-units --all --plain --no-legend --type=service,timer 'uniqnode-*'); then echo "systemd が読み込んでいる uniqnode-* の unit を列挙できない。何も変えずに止める"; exit 1; fi
fs_names=$(/usr/bin/sed 's/\.d$//' <<<"$fs_units")
sd_names=$(/usr/bin/awk '$2 == "loaded" {print $1}' <<<"$sd_units")
units_text=$(printf '%s\n%s\n' "$fs_names" "$sd_names" | /usr/bin/sed '/@\.service$/d; /@\.timer$/d; /^$/d' | /usr/bin/sort -u)
mapfile -t units <<<"$units_text"
timers=(); waits=(); daemons=()
for u in "${units[@]}"; do
  [ -n "$u" ] || continue
  load=$(systemctl show -p LoadState --value "$u")
  if [ "$load" != loaded ]; then echo "$u は読み込まれない(LoadState: $load)。走りえないので飛ばす"; continue; fi
  unit_kind "$u" || { echo "$u の Type か RemainAfterExit を読めない。何も変えずに止める"; exit 1; }
  case "$kind" in
    timer) timers+=("$u") ;;
    oneshot) waits+=("$u") ;;
    daemon) daemons+=("$u") ;;
  esac
done
echo "止める timer: ${timers[*]:-なし} / 終わりを待つ一回走る service(Type=oneshot で RemainAfterExit=no。起こし直さず、timer が次に起こす): ${waits[*]:-なし} / 止める常駐の service: ${daemons[*]:-なし}"
default_units=()
for u in uniqnode-serve@default.service uniqnode-viewer@default.service uniqnode-backup@default.timer; do
  case "$u" in *.timer) svc=${u%.timer}.service ;; *) svc=$u ;; esac
  if [ -d "/etc/systemd/system/$svc.d" ] || systemctl is-enabled --quiet "$u"; then default_units+=("$u"); else echo "$u は据わっていない(/etc/systemd/system/$svc.d が無く、enable もされていない)。disable するものが無い"; fi
done
now=(); dnow=()
for u in "${timers[@]}" "${daemons[@]}"; do
  case "$u" in *@default.*) continue ;; esac
  case "$(systemctl is-active "$u" || true)" in active|activating|reloading|deactivating) now+=("$u") ;; esac
done
for u in "${default_units[@]}"; do if systemctl is-enabled --quiet "$u"; then dnow+=("$u"); fi; done
union_into() {
  local f=$1; shift
  { if [ -f "$f" ]; then /usr/bin/cat "$f"; fi; if [ "$#" -gt 0 ]; then printf '%s\n' "$@"; fi; } | /usr/bin/sort -u > "$f.new"
  /usr/bin/mv -f "$f.new" "$f"
}
[ -d "$state" ] || /usr/bin/mkdir -m 0700 "$state"
union_into "$state/restart" "${now[@]}"
union_into "$state/default-enabled" "${dnow[@]}"
restart=()
while IFS= read -r u; do
  [ -n "$u" ] || continue
  load=$(systemctl show -p LoadState --value "$u")
  if [ "$load" != loaded ]; then echo "$state/restart の $u は読み込まれない(LoadState: $load)。起こせないので一覧から外す"; continue; fi
  unit_kind "$u" || { echo "$u の Type か RemainAfterExit を読めない。何も変えずに止める"; exit 1; }
  if [ "$kind" = oneshot ]; then echo "$state/restart の $u は一回走る service なので起こし直さない(timer が起こす)。一覧から外す"; continue; fi
  restart+=("$u")
done < "$state/restart"
if [ "${#restart[@]}" -gt 0 ]; then printf '%s\n' "${restart[@]}" > "$state/restart.new"; else : > "$state/restart.new"; fi
/usr/bin/mv -f "$state/restart.new" "$state/restart"
mapfile -t default_on < "$state/default-enabled"
echo "最後に起こし直す unit(前の回の分との和。$state/restart): ${restart[*]:-なし} / 止める前に enable されていた @default($state/default-enabled): ${default_on[*]:-なし}"
# shellcheck disable=SC2016
drop_nul='{ /usr/bin/grep -zvxF -e "$1" -- "$2" || [ $? -eq 1 ]; } > "$2.new" && /usr/bin/mv -f "$2.new" "$2"'
# shellcheck disable=SC2016
drop_line='{ /usr/bin/grep -vxF -e "$1" -- "$2" || [ $? -eq 1 ]; } > "$2.new" && /usr/bin/mv -f "$2.new" "$2"'
drop_store_hint() {
  printf "  このストアは前の回の門が %s に残したものだけから来ている。そのプロセスはもう無く、ストアももう要らない(一時のストア、ストアでない道)なら、一覧から外して打ち直す: sudo /usr/bin/bash -c '%s' _ %q %q\n" "$state/stores" "$drop_nul" "$1" "$state/stores"
}
logged() {
  printf '( set -o pipefail; { %s; } 2>&1 | /usr/bin/tee -a %s; echo "exit status: $?" | /usr/bin/tee -a %s )' "$1" "$log" "$log"
}
stage=prescan; checker_used=""
on_failure() {
  local restore="" check="" put_back keep="" u o note=""
  if [ "${#default_on[@]}" -gt 0 ]; then restore="sudo systemctl enable --now ${default_on[*]}"; check="${default_on[*]}"; fi
  if [ "${#restart[@]}" -gt 0 ]; then restore="${restore:+$restore && }sudo systemctl start ${restart[*]}"; check="${check:+$check }${restart[*]}"; fi
  if [ -n "$check" ]; then restore="$restore && sleep 10 && systemctl is-active $check"; fi
  restore="${restore:+$restore && }sudo rm -rf $state"
  for u in "${restart[@]}"; do
    for o in "${old_units[@]}"; do if [ "$u" = "$o" ]; then continue 2; fi; done
    keep="${keep:+$keep }$u"
  done
  if [ -n "$checker_used" ]; then
    put_back=$checker_used
  else
    put_back="<確認済みの S1b 以後のビルド>"
    note="(この回は検め手を確かめていない。$checker か $candidate(この命令を初めて打ったときの $binary の写し)が在っても、sudo -u hikalium -H <それ> hold-status $default_store が 0 か 3 を返すと確かめてから使う)"
  fi
  guide_swapped() {
    echo "戻すのをやめるなら、先に sudo install -m 0755 -o hikalium -g hikalium $put_back $binary で S1b 以後のバイナリへ戻し${note}、それから止めた unit を起こし、起きたと確かめてから状態を消す: $(logged "$restore")"
  }
  guide_extracted() {
    local again="sudo rm -rf $state"
    if [ -n "$keep" ]; then again="sudo systemctl restart $keep && sleep 10 && systemctl is-active $keep && $again"; fi
    echo "戻すのをやめるなら、先に sudo install -m 0755 -o hikalium -g hikalium $put_back $binary で S1b 以後のバイナリへ戻し${note}、上の「旧い名の unit から移る」の外す命令で旧い名の unit を外し、下の「user 単位から移る」の install の命令を打ち(@default が据わり直る)、その後に止めた unit を S1b 以後のバイナリで起こし直し、起きたと確かめてから状態を消す: $(logged "$again")"
  }
  guide_put_back() {
    if [ -e /etc/systemd/system/uniqnode-serve.service ]; then guide_extracted; else guide_swapped; fi
  }
  case "$stage" in
    prescan|stopped)
      if [ "$stage" = prescan ]; then echo "この回は unit を何も止めず、バイナリも差し替えていない"; else echo "止めた unit は止まったまま。起こし直す一覧は $state/restart に残した。直してからこの命令を打ち直せば、最後に一覧の全部を起こす"; fi
      if /usr/bin/cmp -s "$binary" "$saved/uniqnode"; then
        echo "据わっているのは前の回に戻した S1b より前のバイナリである。この命令を打ち直せば先へ進む"
        guide_put_back
      elif [ -n "$binary_id" ] && [ "$(/usr/bin/stat -L -c %d:%i:%s:%Y -- "$binary" 2>/dev/null || true)" = "$binary_id" ]; then
        echo "元の形へ戻すなら(バイナリは差し替えておらず、この命令が始まったときと同じものが据わっている。止めた unit はこれを走らせていたので、systemctl restart と同じ形になる)、止める前に enable されていた @default と止めた unit を起こし、起きたと確かめてから状態を消す: $(logged "$restore")"
      else
        echo "$binary がこの命令の間に替わったか読めない。元の形へ戻すなら、$binary が S1b 以後のビルドと確かめてから: $(logged "$restore")"
      fi ;;
    swapped)
      echo "検めは通り、バイナリを退避の S1b より前のものへ差し替えるところで止まった(この回は旧い名の unit をまだ展開していない)。この命令を打ち直せば最後まで進む"
      guide_put_back ;;
    extracted)
      echo "バイナリを戻し、旧い名の unit の展開か起こすところで止まった。起こし直す一覧は $state/restart に残した。journalctl -u <unit> で理由を見て直し、この命令を打ち直せば、検めをもう一度通して最後まで進む"
      guide_extracted ;;
    started)
      echo "全部起きたが、状態のディレクトリ $state を消せなかった。sudo rm -rf $state で消す" ;;
  esac
}
trap 'rc=$?; if [ "$rc" -ne 0 ]; then on_failure; fi; exit "$rc"' EXIT
scan_processes() {
  local mode=$1 p f exe ino pid cwd dir real line rest st ppid flags cmd match
  local -a argv paths inodes
  found=0; opaque=0; paths=(); inodes=()
  for f in "$binary" "$saved/uniqnode" "$checker" "$candidate"; do
    paths+=("$(/usr/bin/readlink -m -- "$f")")
    if [ -e "$f" ]; then inodes+=("$(/usr/bin/stat -L -c %d:%i -- "$f")"); fi
  done
  for p in /proc/[0-9]*; do
    pid=${p#/proc/}
    if [ "$mode" = pre ] && /usr/bin/grep -qE '^[0-9]+:[^:]*:/system\.slice/(system-uniqnode[^/]*\.slice/)?uniqnode-[^/]+\.service(/|$)' "$p/cgroup" 2>/dev/null; then continue; fi
    if exe=$(/usr/bin/readlink "$p/exe" 2>/dev/null); then
      exe=${exe% (deleted)}
      match=0
      for f in "${paths[@]}"; do if [ "$exe" = "$f" ]; then match=1; fi; done
      if [ "$match" -eq 0 ]; then
        if ino=$(/usr/bin/stat -L -c %d:%i -- "$p/exe" 2>/dev/null); then
          for f in "${inodes[@]}"; do if [ "$ino" = "$f" ]; then match=1; fi; done
        elif [ -d "$p" ]; then
          opaque=$((opaque + 1)); echo "実行ファイルの inode を読めない生きたプロセス: pid $pid($exe)。共有のバイナリを走らせていないと確かめられない"; continue
        fi
      fi
      [ "$match" -eq 1 ] || continue
      found=$((found + 1))
      echo "共有のバイナリか検め手を走らせているプロセス: pid $pid(利用者 $(/usr/bin/stat -c %U "$p" 2>/dev/null))、$exe: $(/usr/bin/tr '\0' ' ' < "$p/cmdline" 2>/dev/null)"
      mapfile -d '' -t argv < "$p/cmdline" 2>/dev/null || argv=()
      dir=""
      if [ "${#argv[@]}" -ge 3 ]; then
        case "${argv[2]}" in
          /*) dir=${argv[2]} ;;
          *) if cwd=$(/usr/bin/readlink "$p/cwd" 2>/dev/null); then dir="$cwd/${argv[2]}"; fi ;;
        esac
      fi
      if [ -n "$dir" ] && real=$(/usr/bin/readlink -e -- "$dir") && [ -d "$real" ]; then
        printf '%s\0' "$real" >> "$state/stores"
        echo "  このプロセスのストア(<command> <data_dir> の第 2 引数)として $real を $state/stores に残す。打ち直したときに検める"
      else
        echo "  このプロセスのストアは決められない。止めた後に、それが開いていたストアを sudo -u <ストアの持ち主> -H <S1b 以後のバイナリ> hold-status <ストア> で自分で検め、0 を確かめてから打ち直す"
      fi
    else
      [ -d "$p" ] || continue
      if ! line=$(/usr/bin/cat "$p/stat" 2>/dev/null); then
        [ -d "$p" ] || continue
        opaque=$((opaque + 1)); echo "実行ファイルも /proc/$pid/stat も読めない生きたプロセス: pid $pid。共有のバイナリを走らせていないと確かめられない"; continue
      fi
      rest=${line##*) }
      read -r st ppid _ _ _ _ flags _ <<<"$rest"
      case "$ppid$flags" in ''|*[!0-9]*) opaque=$((opaque + 1)); echo "/proc/$pid/stat を解けない: $line"; continue ;; esac
      if [ "$st" = Z ] || [ $((flags & 0x00200000)) -ne 0 ]; then continue; fi
      cmd=$(/usr/bin/tr '\0' ' ' < "$p/cmdline" 2>/dev/null || true)
      if [ -z "$cmd" ] && [ "$ppid" = 2 ]; then continue; fi
      [ -d "$p" ] || continue
      opaque=$((opaque + 1))
      echo "実行ファイルを読めない生きたプロセス: pid $pid(状態 $st、親 $ppid、利用者 $(/usr/bin/stat -c %U "$p" 2>/dev/null)): $cmd。ゾンビでもカーネルのスレッドでもないので、共有のバイナリを走らせていないと確かめられない"
    fi
  done
}
scan_processes pre
if [ $((found + opaque)) -gt 0 ]; then
  echo "止める前の門: uniqnode-*.service の外で共有のバイナリ・退避のバイナリ・検め手を走らせるプロセスが $found 個、実行ファイルを確かめられない生きたプロセスが $opaque 個ある。vega では Claude Code のセッションが起こした mcp の Local がふつうこれに当たる(--serve-url の有無に依らない。uniqnode の作業ツリーで開いたセッションのたびに 1 つ起きる)。取り込み・gc・fsck などの手で打った CLI もこれに当たる。上の pid を止めて(mcp ならそのセッションを閉じて)から打ち直す"
  exit 1
fi
if [ ! -f "$binary" ] || [ ! -x "$binary" ]; then
  echo "$binary が無いか実行できない。検め手の候補は作らない"
elif /usr/bin/cmp -s "$binary" "$saved/uniqnode"; then
  echo "$binary は退避のバイナリと同じ(前の回で差し替え済みか、移行の後に一度も差し替えていない)。検め手の候補は作らない"
else
  /usr/bin/install -m 0755 -o root -g root "$binary" "$candidate.new"
  /usr/bin/mv -f "$candidate.new" "$candidate"
  echo "今のバイナリを検め手の候補として写した: $candidate <- $binary(hold-status を知ると確かめてから使う)"
fi
stage=stopped
if [ "${#timers[@]}" -gt 0 ]; then systemctl stop "${timers[@]}"; echo "止めた: ${timers[*]}"; fi
if [ "${#default_units[@]}" -gt 0 ]; then systemctl disable "${default_units[@]}"; echo "disable した: ${default_units[*]}"; fi
waited=0
for u in "${waits[@]}"; do
  while :; do
    st=$(systemctl is-active "$u" || true)
    case "$st" in inactive|failed) break ;; esac
    if [ "$waited" -ge 1800 ]; then echo "$u が 30 分たっても終わらない(状態: $st)。何も差し替えずに止める"; exit 1; fi
    echo "$u の終わりを待つ(状態: $st)"
    sleep 10; waited=$((waited + 10))
  done
done
if [ "${#daemons[@]}" -gt 0 ]; then systemctl stop "${daemons[@]}"; echo "止めた: ${daemons[*]}"; fi
for u in "${timers[@]}" "${waits[@]}" "${daemons[@]}"; do
  st=$(systemctl is-active "$u" || true)
  case "$st" in inactive|failed) ;; *) echo "$u がまだ止まっていない(状態: $st)。何も差し替えずに止める"; exit 1 ;; esac
done
scan_processes post
if [ $((found + opaque)) -gt 0 ]; then
  echo "止めた後の門: 共有のバイナリ(か検め手)を走らせるプロセスが $found 個、実行ファイルを確かめられない生きたプロセスが $opaque 個ある(止める前の門の後に起きたものか、uniqnode-*.service の中に残ったもの)。差し替えると mcp の Local は自分を exec で旧いバイナリへ替え、保留を素通りしうる。上の pid を止めて(mcp ならそのセッションを閉じて)から打ち直す。何も差し替えずに止める"
  exit 1
fi
store_paths=("$default_store"); store_users=(hikalium); store_from=("既定のストア"); store_proc=(0)
for u in "${daemons[@]}" "${waits[@]}"; do
  if ! dir=$(unit_env "$u" UNIQNODE_DATA_DIR); then echo "$u の Environment を D-Bus から読めない。何も差し替えずに止める"; exit 1; fi
  if [ -z "$dir" ]; then
    case "$u" in uniqnode-serve*|uniqnode-viewer*|uniqnode-backup*) echo "$u に UNIQNODE_DATA_DIR が無い。何も差し替えずに止める"; exit 1 ;; *) continue ;; esac
  fi
  user=$(systemctl show -p User --value "$u")
  store_paths+=("$dir"); store_users+=("${user:-root}"); store_from+=("$u"); store_proc+=(0)
done
if [ -s "$state/stores" ]; then
  while IFS= read -r -d '' dir; do store_paths+=("$dir"); store_users+=(""); store_from+=("前の回に止めるよう言ったプロセス"); store_proc+=(1); done < "$state/stores"
fi
stores=(); owners=(); stores_proc=(); stores_orig=()
for i in "${!store_paths[@]}"; do
  s=${store_paths[$i]}
  if ! real=$(/usr/bin/readlink -e -- "$s"); then
    echo "${store_from[$i]} のストア $s を解決できない(無いか、途中のリンクが切れている)。何も差し替えずに止める"
    if [ "${store_proc[$i]}" -eq 1 ]; then drop_store_hint "$s"; fi
    exit 1
  fi
  if [ ! -d "$real" ]; then
    echo "${store_from[$i]} のストア $s($real)はディレクトリでない。何も差し替えずに止める"
    if [ "${store_proc[$i]}" -eq 1 ]; then drop_store_hint "$s"; fi
    exit 1
  fi
  owner=$(/usr/bin/stat -c %U -- "$real")
  if [ -n "${store_users[$i]}" ] && [ "${store_users[$i]}" != "$owner" ]; then echo "${store_from[$i]} の User=${store_users[$i]} と、ストア $real の持ち主 $owner が食い違う。何も差し替えずに止める"; exit 1; fi
  dup=-1
  for j in "${!stores[@]}"; do if [ "${stores[$j]}" = "$real" ]; then dup=$j; fi; done
  if [ "$dup" -lt 0 ]; then
    stores+=("$real"); owners+=("$owner"); stores_proc+=("${store_proc[$i]}"); stores_orig+=("$s")
    echo "検めるストア: $real(${store_from[$i]}、持ち主 $owner)"
  elif [ "${store_proc[$i]}" -eq 0 ]; then
    stores_proc[dup]=0
  fi
done
marked=()
for i in "${!stores[@]}"; do
  s=${stores[$i]}
  if ! found_marker=$(/usr/bin/find -H "$s" -mindepth 1 -maxdepth 1 -name open-marker -print); then
    echo "$s の open-marker を照会できない(読めないか I/O の誤り。不在とは扱わない)。何も差し替えずに止める"
    if [ "${stores_proc[$i]}" -eq 1 ]; then drop_store_hint "${stores_orig[$i]}"; fi
    exit 1
  fi
  if [ -n "$found_marker" ]; then marked+=("$s"); else echo "$s に open-marker は無い"; fi
done
try_checker() {
  local i rc
  ok=(); held=(); unchecked=(); unsupported=()
  for i in "${!stores[@]}"; do
    if /usr/bin/sudo -u "${owners[$i]}" -H "$1" hold-status "${stores[$i]}"; then
      ok+=("$i"); echo "${stores[$i]}: 書ける道でそのまま開いてよい形(hold-status が 0)"
    else
      rc=$?
      case "$rc" in
        3) held+=("$i"); echo "${stores[$i]}: 保留(hold-status が 3)。理由は直前の行" ;;
        1) unchecked+=("$i"); echo "${stores[$i]}: 検められない(終了コード 1。hold-status の誤りか、sudo 自身の失敗)。直前の stderr の行を読む" ;;
        *) unsupported+=("$i"); echo "${stores[$i]}: 検め手が hold-status を知らないか走らない(終了コード $rc。2 は使い方の誤りで、S1b より前の版)" ;;
      esac
    fi
  done
}
if [ "${#marked[@]}" -eq 0 ]; then
  echo "どのストアにも open-marker が無い(S1b 以後のバイナリが開いたことが無い)。保留は無い"
else
  for c in "$checker" "$candidate"; do
    if [ ! -f "$c" ] || [ ! -x "$c" ]; then echo "検め手 $c は無いか実行できない"; continue; fi
    echo "検め手 $c で全部のストアを検める"
    try_checker "$c"
    if [ $(( ${#ok[@]} + ${#held[@]} )) -gt 0 ]; then checker_used=$c; break; fi
    echo "$c はどのストアにも 0 も 3 も答えなかった(hold-status を知らない S1b より前の版か、全部のストアで検められない)。この検め手は使わない"
  done
  if [ -z "$checker_used" ]; then
    echo "印のあるストア(${marked[*]})を検められる検め手が無い。S1b 以後のビルドを sudo install -m 0755 -o root -g root <ビルド> $checker で置いてから打ち直す。何も差し替えずに止める"
    exit 1
  fi
  if [ "$checker_used" = "$candidate" ]; then
    if [ -e "$checker" ]; then
      echo "$checker は hold-status を知らないので使わず、候補 $candidate を使った。$checker は操作者が置いたものなので上書きしない"
    else
      /usr/bin/mv -f "$candidate" "$checker"; checker_used=$checker
      echo "候補が hold-status を知ると確かめたので、検め手として採った: $checker"
    fi
  fi
  if [ "${#held[@]}" -gt 0 ] || [ "${#unchecked[@]}" -gt 0 ] || [ "${#unsupported[@]}" -gt 0 ]; then
    echo "旧いバイナリでは起こさず、何も差し替えずに止める"
    for i in "${held[@]}"; do
      echo "保留の ${stores[$i]}: ホストを再起動するか、ストアを置いたファイルシステムを umount・fsck・mount し直して page cache を捨ててから、この命令を打ち直す。同じ boot_id の Running と Io は再起動だけで解ける。検めを通らない印は、再起動か umount・fsck・mount し直しの後に sudo -u ${owners[$i]} -H $checker_used release-hold ${stores[$i]} を打つか、検め済みの backup から戻す。再起動できずに umount・fsck・mount し直したときも、その後に同じ release-hold を打つ。ホストの再起動は手で起こした llama-server(8082〜8084)も止める(docs/plan/APPEND_FAILURE.md の方針 5 と「S1b より前のバイナリへ戻す」)"
    done
    for i in "${unchecked[@]}"; do
      echo "検められない ${stores[$i]}: 直前の stderr の行を読む。ロックの持ち主がいれば止めてから打ち直す(共有のバイナリでないもの、例えば別のプロジェクトの MCP の設定が起こす --serve-url の無い target/debug/uniqnode mcp は、プロセスの門を素通りしてここで当たる)。ストアでない道なら、その unit の UNIQNODE_DATA_DIR を見直す"
      if [ "${stores_proc[$i]}" -eq 1 ]; then drop_store_hint "${stores_orig[$i]}"; fi
    done
    for i in "${unsupported[@]}"; do
      echo "検め手で検められない ${stores[$i]}: 直前の stderr の行を読み、直してから打ち直す"
    done
    exit 1
  fi
fi
scan_processes post
if [ $((found + opaque)) -gt 0 ]; then echo "検めの後に、共有のバイナリ(か検め手)を走らせるプロセスか、実行ファイルを確かめられない生きたプロセスが現れた。上の pid を止めてから打ち直す。何も差し替えずに止める"; exit 1; fi
tables=$(/usr/sbin/nft list tables)
if /usr/bin/grep -qxF 'table inet uniqnode_default' <<<"$tables"; then /usr/sbin/nft delete table inet uniqnode_default; fi
stage=swapped
/usr/bin/install -m 0755 -o hikalium -g hikalium "$saved/uniqnode" /home/hikalium/.local/bin/.uniqnode.rollback
/usr/bin/mv -f /home/hikalium/.local/bin/.uniqnode.rollback "$binary"
echo "バイナリを戻した: $binary <- $saved/uniqnode"
stage=extracted
/usr/bin/tar -C /etc/systemd/system -xzf "$saved/units.tar.gz"
systemctl daemon-reload
systemctl enable --now "${old_units[@]}"
if [ "${#restart[@]}" -gt 0 ]; then systemctl start "${restart[@]}"; fi
down=(); down_restart=()
for u in "${old_units[@]}" "${restart[@]}"; do
  waited=0
  while :; do
    st=$(systemctl is-active "$u" || true)
    if [ "$st" = active ]; then break; fi
    if [ "$st" = failed ] || [ "$waited" -ge 60 ]; then
      down+=("$u($st)")
      for r in "${restart[@]}"; do if [ "$r" = "$u" ]; then down_restart+=("$u"); fi; done
      break
    fi
    sleep 2; waited=$((waited + 2))
  done
done
if [ "${#down[@]}" -gt 0 ]; then
  echo "起きていない unit がある: ${down[*]}。serve が起きないなら、検めの後に別のバイナリのプロセス(--serve-url の無い target/debug/uniqnode mcp など)がストアのロックを取っていないかも見る"
  for u in "${down_restart[@]}"; do
    printf "  %s は %s から来ている。もう起こさなくてよい unit なら、一覧から外して打ち直す: sudo /usr/bin/bash -c '%s' _ %q %q\n" "$u" "$state/restart" "$drop_line" "$u" "$state/restart"
  done
  exit 1
fi
echo "起こし直した: ${old_units[*]} ${restart[*]}"
stage=started
/usr/bin/rm -rf "$state"
echo "旧い名の unit に戻し終えた"
EOF
```

  `@default` の unit の現物(テンプレート)と drop-in は残るが、disable したので動かない。据え直すときは上の
  外す命令から打ち直す。検め手 /var/backups/uniqnode-legacy-checker(と、採らなかった候補
  /var/backups/uniqnode-legacy-checker.candidate)も残る(差し替えた後に止まった回を打ち直すときの検めに使う。
  要らなくなれば操作者が消す)。状態のディレクトリ /var/backups/uniqnode-legacy-rollback-state(restart・
  default-enabled・stores)は、全部が起きたと確かめたときに消える。戻すのをやめたときは、止まったときの案内の
  命令が、止めたものを起こして active と確かめてから消す。済んだかは、/tmp/uniqnode-legacy-rollback.log の最後が「旧い名の unit に
  戻し終えた」と `exit status: 0` であることで確かめる。

### user 単位から移る

user 単位で動いているものを system 単位に載せ替える。ストアも写し先もバイナリも同じ場所の
まま、unit の置き場と走らせ方だけが変わる。1 命令で通す(移行の間、serve と viewer は止まる):

```
( set -o pipefail; sudo bash -s <<'EOF' 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee /tmp/uniqnode-install-system.log; echo "exit status: $?" | /usr/bin/tee -a /tmp/uniqnode-install-system.log )
set -euo pipefail
sudo -u hikalium -H /home/hikalium/.cargo/bin/cargo build --release --manifest-path /work2/llm_playground_host_dir/uniqnode/Cargo.toml -p uniqnode
/work2/llm_playground_host_dir/uniqnode/target/release/uniqnode install /work2/llm_playground_host_dir/uniqnode-store --system --user hikalium --take-over-user-units --serve-options "--embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank" --viewer-listen 0.0.0.0:7450 --backup-dir /home/hikalium/uniqnode-backup --listen-agent 10.10.128.1:7441 --agent-writable lamalium-notes --agent-collections articles --agent-collections papers --agent-collections seccamp --agent-collections specs --agent-collections trial --agent-collections web --agent-collections lamalium-notes --after wg-quick@wg1.service --firewall-allow 10.10.128.4,10.10.128.2
EOF
```

vega の上なら、sudo を使えるどの利用者がどのディレクトリから貼っても同じに動く形にしてある
(2026-09-30 の操作者の規則): パスは全部絶対パスで、ビルドは hikalium に切り替えて行い、常駐の
利用者は SUDO_USER に頼らず `--user hikalium` で指す。

中で何をしているか(手で同じことをするなら、この順):

1. `--take-over-user-units`: 実行ユーザの user 単位の 3 つの unit を
   `systemctl --user -M <name>@ disable --now uniqnode-serve@default.service uniqnode-viewer@default.service uniqnode-backup@default.timer`
   で止めて外す。終了コードは見ず、効果で判定する: 3 つの `is-active` が走っていない答え
   (inactive・failed・unit が無い)であること、そしてストアのロックが 10 秒以内に外れること。
   まだ走っていれば出力を添えて赤で止まる。指定が無いのに user 単位が動いていれば、install
   は「別プロセスが開いている」で止まり、その文言がこの指定と手で打つ命令を言う。
2. `--firewall-allow 10.10.128.4` の実物を決める(drop-in を描く前): root で `ufw status` を
   読み、`Status: active` なら ufw、そうでなければ(inactive、または ufw が無い)nft。nft なら
   規則ファイル /etc/systemd/system/uniqnode-serve@default.service.d/agent-door.nft を書く。中身は
   自分の表 `inet uniqnode_<インスタンス>` を空で作り、消し、作り直す形(何度読んでも同じ 1 表):
   `ip daddr 10.10.128.1 tcp dport 7441 ip saddr != { 10.10.128.4, 10.10.128.1 } counter drop`
   (自分の IP も許すのは、install の確認が同じ機械から 10.10.128.1 を源に届くため)。iptables-nft や
   docker の表には触らない(base chain は表ごとに評価され、どれかの drop が勝つ)。serve の
   drop-in に `ExecStartPre=+/usr/sbin/nft -f <その道>` を足す(先頭の + は User= に関わらず
   root で走らせる印)ので、nftables.service が無効な機械でも、serve を起こすたびに規則が入る。
   ufw も nft も無ければその旨で赤。
3. 据え付けと確認(上の「system 単位で起こす」と同じ。「確認:」が 5 本。`--agent-writable`・
   `--agent-collections`・グラフの許し(`--agent-graph` か `--agent-graph-writable`)があれば
   それぞれさらに 1 本、下の「読み口(--listen-agent)の確認」)。
4. firewall の効果を見て「確認:」を 1 本足す。ufw なら `ufw allow from 10.10.128.4 to 10.10.128.1
   port 7441 proto tcp` を入れて、`ufw status` の表にその行(10.10.128.1 7441/tcp、ALLOW、
   10.10.128.4)が載ったこと。nft なら `nft list table inet uniqnode_<インスタンス>` にその規則が載っている
   こと(入れたのは serve の起動そのもので、install はここで入れ直さない: 再起動のたびに同じ
   道で入ることの証拠になる)。
5. /tmp/uniqnode-install-system.log を読む。「確認:」の行が 6 本(`--agent-writable` と
   `--agent-collections` があれば 8 本、グラフの許しもあれば 9 本)並び、最後に「次の刻み:」があれば据え付けは完了である。unit の状態は
   `systemctl status uniqnode-serve@default uniqnode-viewer@default uniqnode-backup@default.timer`。
6. user 単位の unit ファイル(~/.config/systemd/user/uniqnode-*)は disable しても残る。
   消すなら `rm ~/.config/systemd/user/uniqnode-*@.service ~/.config/systemd/user/uniqnode-*@.timer`
   と `rm -r ~/.config/systemd/user/uniqnode-*.d`、`systemctl --user daemon-reload`。
   残しておいても enable されていなければ起きない。linger は他に使うものが無ければ
   `loginctl disable-linger` で外せる。

### 読み口(--listen-agent)の確認

install が自分で見るのは 2 本で、報告の「確認: 読み口 …」の行がその結果である:

```
curl http://10.10.128.1:7441/v1/status                      # 主の口と同じ node_id
curl -X POST http://10.10.128.1:7441/v1/admin/gc -d '{"dry_run":true}'   # 403
```

読み口は wg1 が遅れて上がることがあるので、install は 90 秒を上限に答えるまで待ってから
断る(serve の unit が failed に落ちたら待たない)。gc に dry_run を添えるのは、許可表が
壊れていて通ってしまっても本番の pack を回収しないため。

`--agent-writable <c>` があれば、書く口の確認をもう 1 本(「確認: 読み口の書く口: …」)足す。
試し書きはしない: 許した各コレクションへ拡張子の無い文書名で空の PUT を打ち、門を越えて
api.rs の 400(文書名に拡張子が要る)で止まること(403 でなく、本文が門の断りでない。何も
書かれない)、そして許していない名前(uniqnode-install-probe。集合にあれば -x を足す)への
PUT が「は書けない(--agent-writable で許したのは …)」の 403 で断られることを見る:

```
curl -X PUT http://10.10.128.1:7441/v1/collections/lamalium-notes/documents/uniqnode-install-probe --data-binary ''   # 400(門は越えた。書かない)
curl -X PUT http://10.10.128.1:7441/v1/collections/uniqnode-install-probe/documents/uniqnode-install-probe --data-binary ''   # 403 「は書けない」
```

`--agent-collections <c>` があれば、読める集合の確認をもう 1 本(「確認: 読み口の読める集合:
…」)足す。読むだけの確認で、本番のデータには触らない: 集合に無い名前(uniqnode-install-probe。
集合にあれば -x を足す)を collection に指した検索が「は読めない(--agent-collections で許した
のは …)」の 403 で断られることを見る:

```
curl -X POST http://10.10.128.1:7441/v1/search -d '{"query":"probe","collection":"uniqnode-install-probe","top_k":1}'   # 403 「は読めない」
curl -X POST http://10.10.128.1:7441/v1/search -d '{"query":"xhci","collection":"specs","top_k":1}'                     # 200(集合の中)
```

`--agent-graph <g>` か `--agent-graph-writable <g>` があれば、グラフの確認をもう 1 本
(「確認: 読み口のグラフ: …」)足す。読める各グラフの GET が 200 であること、許していない名
(uniqnode_install_probe。集合にあれば -x を足す)の GET が「は読めない(--agent-graph …)」の
403 であることを見る。書けるグラフがあれば、コレクションの書く口と同じ形で試し書きをしない:
attrs がオブジェクトでない本文を PUT し、門を越えて api の 400 で止まること(403 でなく、本文が
門の断りでない。何も書かれない)と、許していないグラフへの PUT が 403 であることを見る:

```
curl http://10.10.128.1:7443/v1/graphs/lamalium-plan                              # 200(読める)
curl http://10.10.128.1:7443/v1/graphs/uniqnode_install_probe                     # 403 「は読めない」
curl -X PUT http://10.10.128.1:7443/v1/graphs/lamalium-plan/nodes/uniqnode-install-probe --data-binary '[]'   # 400(門は越えた。書かない)
curl -X PUT http://10.10.128.1:7443/v1/graphs/uniqnode_install_probe/nodes/uniqnode-install-probe --data-binary '[]'   # 403 「は書けない」
```

firewall は `--firewall-allow <addr>` で install が入れる。<addr> は IPv4 アドレスか CIDR で、
`--firewall-allow 10.10.128.4,10.10.128.2` や `10.10.128.0/24` のようにカンマで並べられる
(`/32` は素のアドレスとして扱う。ホスト部が 0 でない CIDR と `/0` は断る)。ufw が active なら
要素ごとに ufw allow を打ち、そうでなければ nft の自分の表 inet uniqnode_<インスタンス>に
1 つの集合として書く(上の「user 単位から移る」の 2 と 4)。nft は覆われた要素や隣り合う範囲を
まとめて書き戻すので、install の確認は字句ではなく「各要素が集合に覆われていること」で見る。この機械には
ufw と nft の両方が入っているが、ufw は inactive で規則は iptables-nft(docker・fail2ban)が
持っており、INPUT の policy は accept なので、nft の道で「10.10.128.4 以外から 7441 は落とす」
を自分の表に置く形になる(実測 2026-09-06)。読み口は wg1 のアドレスにしか束縛しないので、
127.0.0.1:7441 は接続拒否のままである。

vega の本番の許可は、install が書いたものから手で広げてある(lamalium を orion 10.10.128.4 から
crystal 10.10.128.2 へ移すため)。次の 3 つの規則ファイルの集合に 10.10.128.2 を足した。元の
ファイルは隣の `.bak-<日付>` に残っている。

| 規則ファイル | 口 | 足した日 |
|---|---|---|
| /etc/systemd/system/uniqnode-serve.service.d/agent-door.nft | 7441(主のストアの読み口) | 2026-09-29 |
| /etc/systemd/system/uniqnode-serve@graph_a.service.d/agent-door.nft | 7445(graph_a の読み口) | 2026-09-29 |
| /etc/systemd/system/uniqnode-serve@graph_b.service.d/agent-door.nft | 7443(graph_b の読み口) | 2026-09-30 |

どれも今の集合は `{ 10.10.128.4, 10.10.128.2, 10.10.128.1 }` である。install で据え付け直すと
規則ファイルは `--firewall-allow` の値で書き直されるので、移行の間は 3 つとも
`--firewall-allow 10.10.128.4,10.10.128.2` を渡す(上の「user 単位から移る」の命令はそうしてある)。
移行が済んで orion を外すときは `--firewall-allow 10.10.128.2` にする。

### 手で同じことをするなら(専用ユーザーで置く形)

install を使わず、専用ユーザー uniqnode と /var/lib/uniqnode/default で unit の既定のまま
置く形(インスタンス名は default):

```
sudo useradd --system --home-dir /var/lib/uniqnode --shell /usr/sbin/nologin uniqnode
sudo install -d -o uniqnode -g uniqnode -m 0750 /var/backups/uniqnode/default
sudo cp docs/mop/systemd/system/uniqnode-*@.service docs/mop/systemd/system/uniqnode-*@.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now uniqnode-serve@default.service uniqnode-viewer@default.service uniqnode-backup@default.timer
```

useradd で作るのはユーザー(とその主グループ)だけで、/var/lib/uniqnode は StateDirectory=
が初回の起動で作る。既にあるストアを使うなら `chown -R uniqnode:uniqnode` してから起こす
(所有者が違えば StateDirectory= が直すが、中身の数だけ時間がかかる)。写し先の
/var/backups/uniqnode/default は unit が作らないので install -d で先に作る(別のディスクに置くなら
上の drop-in)。enable するのは backup の timer であって service ではない(service は timer が
起こす)。

serve が走っているあいだ、CLI の ingest・embed・sync はストアのロックに阻まれる。取り込みは
REST(`PUT /v1/collections/{c}/documents/{name}`)か、それを 1 件ずつ打つ
`uniqnode ingest <dir> <c> <パス> --serve-url http://127.0.0.1:7440`(ストアを開かないので
serve を止めずに打てる。ただし主の口は serve の uid の接続だけを受けるので、serve の実行ユーザーで
打つ)で行い、CLI が要るそれ以外の作業は serve を止めて
`sudo -u uniqnode /usr/local/bin/uniqnode ingest /var/lib/uniqnode/default …` のように実行ユーザーで
行う(install --system で据えたなら、その利用者で普通に打つ)。root で走らせると root 所有の
ファイルがストアに残り、次の serve が書けなくなる。

LLM クライアントから使うときの mcp は、それを起こす人間の権限で走る。転送する形は
ストアを開かないので 0750 のディレクトリを指したまま起こせるが、ログの既定の道
`<dir>/logs/mcp.log` には書けない。このとき mcp は `$XDG_STATE_HOME/uniqnode/logs/mcp.log`
(無ければ `~/.local/state/uniqnode/logs/mcp.log`)へ倒し、最初の行で両方の道を言う
([docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004))。標準エラーは
クライアントが吸うので、mcp の記録を読むのはそのファイルである。登録に `--log` は要らない:

```
claude mcp add --transport stdio uniqnode -- /usr/local/bin/uniqnode mcp /var/lib/uniqnode --serve-url http://127.0.0.1:7440
```

## user 単位で起こす

第一の道は 1 命令である。ビルドした実行ファイルで、ストアにするディレクトリを指して打つ:

```
cargo build --release -p uniqnode
target/release/uniqnode install ~/uniqnode-store
```

これで serve(127.0.0.1:7440)・viewer(127.0.0.1:7450)・毎日 0 時の backup
(~/uniqnode-backup/default)が user 単位の systemd に載り、命令は効果を見てから戻る。待ち受け・
写し先・置き場は引数で変える:

```
uniqnode install <dir> [--listen <addr>] [--viewer-listen <addr>] [--serve-options "<引数列>"]
                       [--backup-dir <dir>] [--bin <path>] [--unit-dir <dir>] [--no-start]
                       [--listen-agent <addr>] [--agent-writable <c>]...
                       [--agent-collections <c>]... [--agent-graph <g>]...
                       [--agent-graph-writable <g>]...
                       [--system [--user <name>] [--after <unit>]...]
```

| 引数 | 既定 | 意味 |
|---|---|---|
| `<dir>` | (必須) | ストア。/tmp の下は断る(unit の PrivateTmp=yes から見えない) |
| `--listen` | 127.0.0.1:7440 | serve の待ち受け。viewer の転送先もここから導く |
| `--viewer-listen` | 127.0.0.1:7450 | viewer の待ち受け |
| `--serve-options` | 空 | serve の追加の引数を 1 つの文字列で(例 `"--embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank"`) |
| `--backup-dir` | ~/uniqnode-backup/<インスタンス> | 写し先。既定が名ごとに分かれるのは、2 つの実体が同じ写し先を取り合うと backup が毎日「別のノードの写し」で失敗するからである |
| `--bin` | ~/.local/bin/uniqnode | 実行ファイルの置き場。走っている自分自身をここへ写す |
| `--unit-dir` | ~/.config/systemd/user(`--system` なら /etc/systemd/system) | unit と drop-in の置き場(テスト用) |
| `--no-start` | — | daemon-reload までで止める(unit を置くだけ) |
| `--listen-agent` | 無し | serve の読み口(第 2 の待ち受け。許可表の外は 403)。drop-in に UNIQNODE_AGENT_LISTEN を書き、ExecStart= の末尾に `--listen-agent ${UNIQNODE_AGENT_LISTEN}` を足す。--listen と別のアドレスで、ポートは固定 |
| `--agent-writable` | 無し | 読み口から書けるコレクション(複数可。`--listen-agent` があるときだけ)。drop-in に `Environment="UNIQNODE_AGENT_WRITABLE=<c1> <c2>"` を書き、ExecStart= の末尾に `--agent-writable <c>` を集合の数だけ並べる。確認に「許したコレクションへの PUT が門を越え、許していないものは 403」を足す(試し書きはしない) |
| `--agent-collections` | 無し(全コレクションが読める) | 読み口から読めるコレクション(複数可。`--listen-agent` があるときだけ)。drop-in に `Environment="UNIQNODE_AGENT_COLLECTIONS=<c1> <c2>"` を書き、ExecStart= の末尾に `--agent-collections <c>` を集合の数だけ並べる。確認に「許していない名を指した検索が 403」を足す(読むだけ。本番のデータに触らない) |
| `--agent-graph` | 無し(グラフ層は読み口に現れない) | 読み口から読めるグラフ(複数可。`--listen-agent` があるときだけ)。drop-in に `Environment="UNIQNODE_AGENT_GRAPH=<g1> <g2>"` を書き、ExecStart= の末尾に `--agent-graph <g>` を集合の数だけ並べる。確認に「許したグラフが読め、外の名は 403」を足す |
| `--agent-graph-writable` | 無し | 読み口から読み書きできるグラフ(複数可。`--listen-agent` があるときだけ)。書ける名は読めもする。drop-in に `Environment="UNIQNODE_AGENT_GRAPH_WRITABLE=<g1> <g2>"` を書き、ExecStart= の末尾に `--agent-graph-writable <g>` を集合の数だけ並べる。確認に「許したグラフへの PUT が門を越え、許していないものは 403」を足す(試し書きはしない) |
| `--instance` | default | 同じ機械に何組も置くための名(下の「2 つ目のストアを同じ機械で」)。unit の名・drop-in の置き場・既定のストアと写し先・nft の表がこの名で分かれる。ASCII の小文字と数字と `_` の 32 字まで |
| `--system` | — | system 単位に据える(上の「system 単位で起こす」)。root で走らせる |
| `--user` | SUDO_USER | `--system` の実行ユーザ(unit の User=/Group=)。root は断る。`--system` のときだけ |
| `--after` | 無し | drop-in の `[Unit]` に After= と Wants= で書く unit(複数可。`--system` のときだけ) |
| `--take-over-user-units` | — | 据える前に実行ユーザの user 単位の常駐を止めて外す(`--system` のときだけ。上の「user 単位から移る」) |
| `--firewall-allow` | 無し | このアドレス(IPv4 アドレスか CIDR。カンマで並べられる)からだけ読み口へ届く規則を入れて効果を見る。ufw が active なら ufw allow、そうでなければ nft の表 inet uniqnode_<インスタンス>(規則ファイルを drop-in の隣に置き、serve の ExecStartPre=+nft -f が起動のたびに入れる)。`--system` で `--listen-agent` があるときだけ |

出力は手順ごとに 1 行で、最後に確認した観測(serve と viewer 経由の /v1/status が返した
node_id、backup の写し先の fsck の件数)と backup の次の刻みが出る。実測(2026-09-05、
`--listen 127.0.0.1:7443 --viewer-listen 127.0.0.1:7453`):

```
install: インスタンス default(unit は uniqnode-serve@default.service、nft の表は inet uniqnode_default)
install: バイナリ /home/op/uniqnode-install-probe-bin/uniqnode ← …/target/debug/uniqnode(32219056 bytes)
install: unit /home/op/.config/systemd/user/uniqnode-serve@.service
…
install: systemctl --user daemon-reload: 済み
install: uniqnode-serve@default.service: enable、起こした(直前は inactive)
…
install: loginctl enable-linger: 済み
install: 確認: http://127.0.0.1:7443/v1/status と viewer http://127.0.0.1:7453 経由が同じ node_id 2dce4e60… を返した(1 ms)
install: 確認: uniqnode-backup@default.service を 1 回走らせ、写し先 /home/op/uniqnode-install-probe-backup を開いて fsck: objects 0 refs 0 errors 0
install: 次の刻み: Sun 2026-09-06 00:00:00 JST 6h left … uniqnode-backup@default.timer uniqnode-backup@default.service
```

失敗は理由を標準エラーに出して 1 で終わり、途中まで置いたものはそのまま残る。直して同じ命令
を打てばよい: 再実行は更新である(バイナリを写し直し、unit と drop-in を書き直し、
daemon-reload して restart する)。ストアを別のプロセス(手で起こした serve や CLI)が
開いていると、unit を起こす前に「別プロセスが開いている」と言って止まる(unit は exit 1 で
起こし直さないので、起こしてから journal を読ませるより先に言う)。system 単位
(/etc/systemd/system)は同じ命令に `--system` を足す(上の「system 単位で起こす」)。

### 中で何をしているか(手で同じことをするなら)

install は次を 1 手順 1 命令で行う。unit は docs/mop/systemd/user/ の現物を実行ファイルに
埋め込んだもの(include_str!)なので、置かれるものはこのリポジトリのファイルと同じである。

1. 走っている自分自身を `--bin` へ写す。隣に書いてから rename するので、走行中の実行ファイル
   を上書きしない(cp の Text file busy を避ける)。
2. テンプレート unit 4 本(serve・viewer・backup の service と backup の timer)を
   `--unit-dir` に書く。名前にインスタンスは入らない(`uniqnode-serve@.service` のまま)。
   手でなら:

   ```
   mkdir -p ~/.config/systemd/user ~/uniqnode-backup/default
   cp docs/mop/systemd/user/uniqnode-*@.service docs/mop/systemd/user/uniqnode-*@.timer ~/.config/systemd/user/
   ```

3. 3 つの service に drop-in `<unit>.d/override.conf` を書く(`<unit>` はインスタンスの
   入った名。`uniqnode-serve@default.service.d/override.conf`)。中身は PATH(install 自身の
   PATH で pdftotext・pdftohtml・pdftoppm・curl を探し、見つかったディレクトリを systemd の
   既定の前に置く。見つからない道具は「install: <名> は PATH に無い(…)」と 1 行ずつ言うが
   止まらない: 道具が無くても serve は起き、その機能だけが 503 で答える)、「unit の読み方」の
   環境変数、ReadWritePaths=(ストアと写し先。空の行で unit の値を消してから)、
   ExecStart=(空の行で消してから、`--bin` の道で書き直す。引数の並びは unit の ExecStart=
   行を読んでバイナリの道だけ替える)。serve の drop-in はこの形になる:

   ```
   [Service]
   Environment=PATH=/home/op/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
   Environment=UNIQNODE_DATA_DIR=/home/op/uniqnode-store
   Environment=UNIQNODE_LISTEN=127.0.0.1:7440
   Environment=UNIQNODE_SERVE_OPTIONS=
   ReadWritePaths=
   ReadWritePaths=/home/op/uniqnode-store
   ExecStart=
   ExecStart=/home/op/.local/bin/uniqnode serve ${UNIQNODE_DATA_DIR} ${UNIQNODE_LISTEN} $UNIQNODE_SERVE_OPTIONS
   ```

   ストアと写し先のディレクトリを作る(ReadWritePaths= は無い道を作らない)。
4. `systemctl --user daemon-reload`。`--no-start` はここで止まる。
5. ストアのロックを探り、別のプロセスが開いていれば止まる。
6. `systemctl --user enable` と `systemctl --user restart` を uniqnode-serve@default.service、
   uniqnode-viewer@default.service、uniqnode-backup@default.timer に(restart は止まっている unit も起こす
   ので、初回と更新で同じ手順)。手でなら
   `systemctl --user enable --now uniqnode-serve@default.service uniqnode-viewer@default.service uniqnode-backup@default.timer`。
7. `loginctl enable-linger`。取れなければ警告して続ける。無ければ、最後のセッションが閉じた
   ときに user 単位のマネージャごと止まり、backup の timer の刻みも来ない。
8. 確認(下の「効いていることの確かめ方」と同じ観測): serve の /v1/status と viewer 経由の
   /v1/status が同じ node_id を返すまで短い間隔で待ち(上限 90 秒。serve の unit が failed
   に落ちたら待たずに言う)、`--listen-agent` があれば読み口の /v1/status が同じ node_id を
   返し POST /v1/admin/gc(dry_run)が 403 であることを同じ上限で待ち、`--agent-writable` が
   あれば書く口を書かずに確かめ、`--agent-collections` があれば集合の外を指した検索が 403 で
   断られることを確かめ、`--agent-graph`(か `--agent-graph-writable`)があれば許したグラフが
   読めて外の名が 403 であることを確かめ(いずれも上の「読み口(--listen-agent)の確認」)、
   `systemctl --user start uniqnode-backup@default.service` を 1 回走らせ、写し先をストアとして開いて
   fsck する(backup 命令の最後の検証と同じ関数。system 単位では実行ユーザで
   `<bin> fsck <写し先>` を起こし、最後にストアと写し先の所有者を見る)。
9. `systemctl --user list-timers uniqnode-backup@default.timer` の表を載せる。

unit だけを手で置いたときのデータディレクトリの既定は %S/uniqnode/<インスタンス> で、実際にどこへ
置かれたかは起動時のログの行「ログを <dir>/logs/serve.log に残す」が言う
(`journalctl --user -u uniqnode-serve@default`)。ProtectHome=read-only なので、home の下の別の場所
を使うなら ReadWritePaths= を添える(上の「unit の読み方」。install はこれを drop-in に書く)。

## 効いていることの確かめ方

設定は編集した時点ではなく、効果を見た時点で完了である(should/0116)。

```
systemctl --user status uniqnode-serve@default uniqnode-viewer@default   # system 単位なら --user を外す
curl http://127.0.0.1:7440/v1/status                      # serve が答える
curl http://127.0.0.1:7450/v1/status                      # viewer が serve へ転送して同じ答え
```

system 単位では、主の口は serve の User= の uid の接続だけを受けるので、1 行目は unit の
実際の User=(既定は uniqnode で、`install --system` の drop-in が替えていればその利用者)で
`sudo -u "$(systemctl show -p User --value uniqnode-serve@default)" curl http://127.0.0.1:7440/v1/status`
の形で叩く(User= と違う利用者のままだと
「main door: この接続の相手を受け付けない」の 403 になる。それ自体が uid の判定の効いている証である)。

埋め込みを足したなら、起動直後の journal に `uniqnode: embedding: bge-m3 (http://…)` の
行があること、検索の応答の `method` が hybrid になる(または `degraded` が理由を言う)
ことまで見る。

backup は刻みを待たずに 1 回起こして、写しができたことを写し先で見る:

```
systemctl --user list-timers uniqnode-backup@default.timer     # 次の発火時刻が出る
systemctl --user start uniqnode-backup@default.service          # 一回きりに走る(終わるまで戻らない)
journalctl --user -u uniqnode-backup@default.service -n 5       # copied … / backup: … / verify: … errors 0
uniqnode fsck <backup_dir>                              # unit の外からも写しが開けて緑
```

実測(user 単位、serve が同じストアのロックを持ったまま): 1 回目は未封印の pack と node_key を
写して `verify: objects 4 refs 0 errors 0`、serve 経由で 1 件足した後の 2 回目も同じ 2 つだけを
写し直して緑、その間 serve は走り続けた。

## 停止

```
systemctl --user stop uniqnode-viewer@default uniqnode-serve@default
```

systemd は SIGTERM を送り、uniqnode はそれを受け取る手続きを持たないので即座に落ちる。
それでよい理由は、ストアが書き込みのたびに fsync 済みで、途中で裂かれても次の open が
回復するからである(上の Restart= の項と同じ根拠)。処理中の要求は応答なしに切れる。
実測では stop の完了に 30 ms、ポートの解放は即時、次の起動でオブジェクト数はそのまま
だった。

viewer だけ止めても serve は走り続け、serve だけ止めても viewer は残って 502 で「先に
serve を起こす」と言う。

backup を止めるのは timer である(`systemctl --user stop uniqnode-backup@default.timer`。刻みを
止めるだけで、走行中の写しは最後まで走る)。走行中の backup を裂いても写し先は「前回の写し +
未封印のセグメント」として開ける([docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2))。

## 更新

ビルドしてから同じ引数で `uniqnode install` を打ち直す(バイナリを写し直し、unit と
drop-in を書き直し、restart し、確認まで通す)。system 単位なら同じく sudo で
`… install … --system … 2>&1 | tee /tmp/uniqnode-install-system.log`。install を使わず
専用ユーザーで置いた形なら手で:

```
cargo build --release -p uniqnode
sudo install -m 0755 target/release/uniqnode /usr/local/bin/uniqnode
sudo systemctl restart uniqnode-serve@default uniqnode-viewer@default
```

install(1) は走っている実行ファイルを一度 unlink してから置くので、走行中に差し替えられる
(cp は Text file busy で断られる。`uniqnode install` は隣に書いて rename する)。走っているプロセスは古いイメージのまま動き続けるので、
restart で新しいものに替わる。restart は停止と同じく SIGTERM で落として起こし直す。

## ログの見方

同じ行が 2 箇所に残る。

| 場所 | 見方 | 中身 |
|---|---|---|
| journal | `journalctl --user -u uniqnode-serve@default -f`(system 単位なら `sudo journalctl -u …`) | 標準エラーと標準出力 |
| `<dir>/logs/serve.log`(viewer は viewer.log) | `tail -f` | 標準エラーと同じ行 |

- 行頭の `2026-09-05T04:48:45Z [pid 176927]` は uniqnode 自身が付ける UTC の時刻と pid で、
  journal の時刻(ローカル)と別に付く。2 つの記録を突き合わせるのはこの部分で行う。
- `listening on <addr>` の 1 行だけは標準出力(起動スクリプトとの取り決め)で、journal に
  はあるがファイルには無い。ファイルには代わりに「<addr> で待ち受ける」がある。この行は
  ストアを開き、埋め込み・リランカーを装備し、アドレスを束縛した後に出るので、出た時点で
  要求を受け付けている(起動確認はこの行を待ってよい)。
- ファイルの回転(8 MiB × 5 世代)と `--log`・`--no-log` は
  [docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004)。journal の保持は
  journald の設定による。
- 起動に失敗した理由(ストアを開けない・アドレスが塞がっている)は journal に残る。ログの
  ファイルはストアを開くより先に開くので、そちらにも残る。
- backup の `copied …`・`backup: …`・`verify: …` は一回きりの命令の結果であってログではない
  ので、journal にだけある(`journalctl --user -u uniqnode-backup@default`)。写せなかった・検証が赤
  だったときは unit が failed になり、`systemctl --user list-units --failed` に並ぶ。

## 二重起動したときの症状

serve は起動時にまずストアを開き、次にアドレスを束縛し、最後に `listening on` を出す。
どちらで衝突したかで出る行が違うが、どちらの場合も `listening on` は出ず、何も束縛しないまま
終わる(node/tests/api.rs の
a_serve_that_cannot_take_the_store_lock_never_says_listening_on)。

| 状況 | 出る行 | 終了 |
|---|---|---|
| 同じストアに 2 本目(アドレスは別) | ロックの判別の 250 ms の後に `uniqnode: ストアを開けない: invalid: <dir> は別プロセスが開いている` | 1 |
| 同じアドレスに 2 本目(ストアは別) | ストアを開いた後に `uniqnode: serve: 127.0.0.1:7440 に束縛できない: Address already in use (os error 98)` | 1 |
| 同じストアかつ同じアドレスに 2 本目 | 先に見るのはストアなので、上の「別プロセスが開いている」 | 1 |

- ロックはデータディレクトリの正規化した道から名付けるので、unit と CLI、unit と別の unit の
  どの組でも互いに検出する。閉じ込めの中で走る unit と外の CLI の間でも同じである
  (実測: user 単位、PrivateUsers=yes、どちらが先でも)。
- unit の側が 2 本目だったときは、exit 1 なので RestartPreventExitStatus= により起こし直さ
  ず、`systemctl status` が failed (Result: exit-code) を示す。先に走っていた方を止めてから
  `systemctl reset-failed uniqnode-serve@default` と `systemctl start uniqnode-serve@default`。
- viewer はロックを取らないので、同じストアに何本でも起こせる。衝突するのはアドレスだけである。
- serve が走っている間に CLI の ingest・embed・sync を叩くと、同じ「別プロセスが開いて
  いる」で断られる。REST を使うか、serve を止めてから行う(上の「system 単位で起こす」)。

## 2 つ目のストアを同じ機械で

unit はテンプレートなので、増えるのは設定の 1 項目であって、ファイルの写しではない
(should/0118)。2 つ目を据えるのは `--instance` を変えた install 1 回である。

```
sudo /usr/local/bin/uniqnode install /srv/uniqnode-graph --system \
  --instance graph --listen 127.0.0.1:7442 --viewer-listen 127.0.0.1:7452 \
  2>&1 | ts '%Y-%m-%dT%H:%M:%S%z' | tee /tmp/uniqnode-install-graph.log
```

インスタンスごとに分かれるもの:

| もの | 分かれ方 |
|---|---|
| unit の名 | `uniqnode-serve@<名>.service`(テンプレートは 1 組しか置かれない) |
| drop-in | `<unit_dir>/uniqnode-serve@<名>.service.d/override.conf` |
| 既定のストア | `%S/uniqnode/<名>` |
| 既定の写し先 | `<home>/uniqnode-backup/<名>`(install が drop-in に書く。手で置いた unit の既定は system が `/var/backups/uniqnode/<名>`、user が `%h/uniqnode-backup/<名>`) |
| nft の表 | `inet uniqnode_<名>` |

分かれないもの: 待ち受けのアドレスと、実行ファイルの道。ポートは install の `--listen` と
`--viewer-listen`(と `--listen-agent`)で明示する。同じポートを 2 つのインスタンスに与えると、
2 本目は exit 1 で起きない(上の「二重起動したときの症状」)。

nft の表を名ごとに分けるのは、規則ファイルが表ごと消して作り直す形だからである。1 つの名を
共有すると、後から起きた serve が先の実体の規則を消し、その実体の読み口が誰からでも
届くようになる(2026-09-08 に、2 つ目を据える前に見つけた)。

写し先の既定も名ごとに分かれる(`<home>/uniqnode-backup/<名>`)。分けていないと、2 つ目の
実体の backup が毎日「別のノードの写し(node_key が写し元と違う)」で失敗する。写しは壊れない
(backup は取り合いに気づいて何も書かずに 1 で終わる)が、毎日 failed が積まれる。

2026-09-08 より前に据えた実体は、写し先が `<home>/uniqnode-backup`(名の付かない道)を
指している。install を打ち直すと既定が `<home>/uniqnode-backup/default` に移り、初回は全件の
写しになる(増分の起点が無いため)。古い写しはそのまま残るので、確かめてから消す。動かし
たくなければ `--backup-dir <home>/uniqnode-backup` を明示する(明示した道は名を変えても
動かない)。

インスタンス名は ASCII の小文字と数字と `_` の 32 字までである。この名が unit の名・nft の
表の名・drop-in の道の 3 つにそのまま入るので、3 つとも通る字種に限る(`-` は nft の識別子に
使えない)。

### テンプレートになる前の名の unit から移る

2026-09-08 より前に据えた機械には、`uniqnode-serve.service` のように `@` の無い名の unit が
残っている。既定のインスタンスを据えようとすると、同じ主の口と同じストアを 2 つの unit が
取り合うので、install は何も置かずに断り、外す命令を出力に添える。黙って止めはしない
(操作者のものを止めるのは操作者の判断。must/0022)。

```
sudo systemctl disable --now uniqnode-serve.service uniqnode-viewer.service uniqnode-backup.timer
sudo rm -rf /etc/systemd/system/uniqnode-serve.service /etc/systemd/system/uniqnode-serve.service.d \
  /etc/systemd/system/uniqnode-viewer.service /etc/systemd/system/uniqnode-viewer.service.d \
  /etc/systemd/system/uniqnode-backup.service /etc/systemd/system/uniqnode-backup.service.d \
  /etc/systemd/system/uniqnode-backup.timer /etc/systemd/system/uniqnode-backup.timer.d
sudo systemctl daemon-reload
sudo nft delete table inet uniqnode   # 読み口を firewall で限っていたときだけ
```

外した後に install を打ち直すと、テンプレートと `@default` の drop-in が置かれる。ストアは
drop-in の `UNIQNODE_DATA_DIR` が指したままなので、動かす必要はない。

別の名のインスタンスを足すだけなら、古い名の unit とは取り合わない。install はその旨を
1 行言って、そのまま進む。vega の主のストアに当てた手順(残る drop-in と nft の中身、止まる間、
install との順)は上の「旧い名の unit から移る」。

## git の木を定期に取り込む

手元の git の木の、ある ref に追跡されている文書を、走っている serve のコレクションへ
毎時入れる。命令は `uniqnode ingest-git`(意味は
[docs/design/INGEST.md](#47d69a3e-c39a-4e76-9814-e9c24240293b) の「git の木からの取り込み」)で、
unit は docs/mop/systemd/system/ の uniqnode-ingest-git@.service と uniqnode-ingest-git@.timer
である。`uniqnode install` はこの 2 つを置かない(上の表の `*@.service` のうち、install が置くのは
serve・viewer・backup の 3 つ)。system 単位だけを用意してあり、手で置く。

インスタンス名は取り込み先のコレクション名である(`uniqnode-ingest-git@lamalium` がコレクション
lamalium へ入れる)。ストアのインスタンス名ではない。既定は serve の主の口
`http://127.0.0.1:7440` へ送り、木は `/srv/uniqnode-trees/<インスタンス名>`、ref は `main`、道は
lamalium の木の一覧(DESIGN.md・docs/design・docs/plan・docs/mop・policy・project_policy・
memory)。変えるものはすべて unit の先頭に並べた環境変数で、drop-in の `Environment=` で替える。

この unit は木を取りに行かない。木を置き、最新にするのは別の段で、既定では何も無い。木が
無い・ref が無い・道の 1 つがその ref に無いときは、何も送らずに理由を journal に残して
failed になる(0 件で成功したことにはしない。must/0022)。木を最新にする段を足すなら、drop-in に
ExecStartPre= を書く。例えば fetch で最新にする clone なら:

```
[Service]
Environment=UNIQNODE_GIT_REF=origin/main
ReadWritePaths=/srv/uniqnode-trees/lamalium
ExecStartPre=/usr/bin/git -C ${UNIQNODE_GIT_TREE} fetch --quiet origin
```

(ProtectSystem=strict で木は読むだけになっているので、fetch が書く木を ReadWritePaths= で
開ける。ExecStartPre= が失敗すれば取り込みは走らない。資格情報を何で渡すかは取り方の
裁定による。) 木を他の機械から push で届ける形なら、ExecStartPre= は要らず、届く先の裸の木を
UNIQNODE_GIT_TREE に指す。ingest-git は HTTP で serve へ送るので、木のある別の機械で走らせて
この機械の serve へ送る形もとれる(その機械から届く口が要る)。

木の所有者は unit の User= と同じにする。違うと git が「dubious ownership」で断り、その文が
journal に載る。

置き方(本番 vega。木が置かれてから打つ。unit の User= を本番の実行ユーザに、ExecStart= を
本番のバイナリに替える drop-in を書き、1 回走らせて確かめてから timer を有効にする):

```
{ sudo install -m 0644 -t /etc/systemd/system /work2/llm_playground_host_dir/uniqnode/docs/mop/systemd/system/uniqnode-ingest-git@.service /work2/llm_playground_host_dir/uniqnode/docs/mop/systemd/system/uniqnode-ingest-git@.timer && sudo mkdir -p /etc/systemd/system/uniqnode-ingest-git@lamalium.service.d && printf '%s\n' '[Service]' 'User=hikalium' 'Group=hikalium' 'Environment=UNIQNODE_DATA_DIR=/work2/llm_playground_host_dir/uniqnode-store' 'ExecStart=' 'ExecStart=/home/hikalium/.local/bin/uniqnode ingest-git ${UNIQNODE_DATA_DIR} ${UNIQNODE_INGEST_COLLECTION} ${UNIQNODE_GIT_TREE} --ref ${UNIQNODE_GIT_REF} --paths ${UNIQNODE_GIT_PATHS} --serve-url ${UNIQNODE_SERVE_URL}' | sudo tee /etc/systemd/system/uniqnode-ingest-git@lamalium.service.d/override.conf && sudo systemctl daemon-reload && sudo systemctl start uniqnode-ingest-git@lamalium.service && sudo systemctl enable --now uniqnode-ingest-git@lamalium.timer && systemctl list-timers uniqnode-ingest-git@lamalium.timer --no-pager; sudo journalctl -u uniqnode-ingest-git@lamalium.service -n 40 --no-pager; } 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee /tmp/uniqnode-ingest-git-lamalium.log
```

(どこから打ってもよい。unit は Type=oneshot なので、取り込みが失敗すれば `start` が失敗を返し、timer は有効にしない。journal は成否によらず最後に出す。) 確かめる
のは journal の 3 種の行である: `木: … の main = <コミット>(書き出し N 件、…)`、1 件ごとの
`lamalium/<文書名>: updated|no-op …`、締めの `取り込み: N 件(updated N、no-op N)、対象外 N 件`。
2 回目以降は変わった文書だけが updated になる。

git から消えた文書は uniqnode から消えない(取り込みは足すか書き換えるだけで、ref を
tombstone しない)。消したいときは serve の主の口へ
`PUT /v1/refs/collections/lamalium/<文書名>`(本文 `{"target":null}`)で ref を tombstone する。
tombstone した文書と旧版のチャンクは gc が回収する([docs/design/GC.md](#9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。

止めるのは timer である(`sudo systemctl disable --now uniqnode-ingest-git@lamalium.timer`)。

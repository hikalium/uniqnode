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

| unit | 中身 | 待ち受け(既定) | ストアのロック |
|---|---|---|---|
| uniqnode-serve.service | `uniqnode serve <dir> 127.0.0.1:7440` | 7440 | 取る |
| uniqnode-viewer.service | `uniqnode viewer <dir> 127.0.0.1:7450 --serve-url http://127.0.0.1:7440` | 7450 | 取らない |
| uniqnode-backup.service | `uniqnode backup <dir> <backup_dir>`(Type=oneshot) | — | 取らない(写し先のロックだけを検証の間) |
| uniqnode-backup.timer | uniqnode-backup.service を毎日 0 時に起こす。Persistent=true なので逃した刻みは次の起動時に走る | — | — |

viewer は serve の後に起こす(After= と Wants=)。Requires= にしないのは、serve が居なくても
頁は出て、届かないことを 502 の本文で言うためである。serve を起こし直せばそのまま繋がる。
backup は serve に順序を持たない。ロックを取らないので serve が走っていても止まっていても同じ
写しが取れる。

unit は system 単位と user 単位で別のファイルにしてある。

| 置き場 | ファイル | 動かす者 | データディレクトリ(既定) | 写し先(既定) |
|---|---|---|---|---|
| /etc/systemd/system/ | docs/mop/systemd/system/*.service と *.timer | 専用ユーザー uniqnode(`install --system` は drop-in の User=/Group= で sudo を打った利用者に替える) | /var/lib/uniqnode | /var/backups/uniqnode |
| ~/.config/systemd/user/ | docs/mop/systemd/user/*.service と *.timer | 自分 | この機械の systemd 249 では ~/.config/uniqnode(新しい systemd では ~/.local/state/uniqnode)| ~/uniqnode-backup |

同じファイルを両方で使えないのは実測による。user 単位に User= を書くと exit 217 で
起動できず、閉じ込め(ProtectSystem= など)は user 単位では PrivateUsers=yes を添えないと
黙って効かない(/ が読み書き可のまま走る)。違いはその 3 点(User=/Group= の有無、
PrivateUsers=yes の有無、WantedBy=)と backup の写し先の既定だけで、各ファイルの先頭に
書いてある。timer は両方で同じ中身である。

## unit の読み方

運用者が変えるものはすべて環境変数にしてあり、ExecStart= はそれを並べるだけである。
変えるときは unit ファイルを編集せず、drop-in(`systemctl edit uniqnode-serve`。user 単位
なら `systemctl --user edit uniqnode-serve`)に `[Service]` と `Environment=` を書く。
unit ファイルは差し替えても drop-in は残る。

| 変数 | unit | 既定 | 意味 |
|---|---|---|---|
| UNIQNODE_DATA_DIR | 両方 | %S/uniqnode | ストア。ロック・logs/・derived/ はこの下 |
| UNIQNODE_LISTEN | serve | 127.0.0.1:7440 | HTTP API の待ち受け |
| UNIQNODE_SERVE_OPTIONS | serve | 空 | 追加の引数(`--embed` など。空白で分ける) |
| UNIQNODE_VIEWER_LISTEN | viewer | 127.0.0.1:7450 | ブラウザが開く待ち受け |
| UNIQNODE_SERVE_URL | viewer | http://127.0.0.1:7440 | 転送先。UNIQNODE_LISTEN を変えたらここも |
| UNIQNODE_VIEWER_OPTIONS | viewer | 空 | 追加の引数(ログの指定など) |
| UNIQNODE_BACKUP_DIR | backup | system: /var/backups/uniqnode、user: %h/uniqnode-backup | 写し先。変えるときは ReadWritePaths= も(下) |
| UNIQNODE_AGENT_LISTEN | serve(install の `--listen-agent` だけが書く) | 無し | 読み口の待ち受け。unit の ExecStart= には無く、install が drop-in の ExecStart= の末尾に `--listen-agent ${UNIQNODE_AGENT_LISTEN}` を足す |
| UNIQNODE_AGENT_WRITABLE | serve(install の `--agent-writable` だけが書く) | 無し | 読み口から書けるコレクションの集合(空白区切り。読み手のための写し)。ExecStart= には展開せず、install が drop-in の ExecStart= の末尾に `--agent-writable <c>` を集合の数だけ値のまま並べる |
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
  2 は引数の誤りである。POST /v1/admin/shutdown による終了(0)は意図した停止なので、
  on-failure は起こし直さない(unit は inactive のまま。起こすなら `systemctl start`)。
- 閉じ込めは書ける場所を StateDirectory= の下だけにし、/ と home を読むだけにする。
  RestrictAddressFamilies= に AF_UNIX を残しているのはロックが unix socket だからで、
  PrivateNetwork= を使わないのは、抽象名前空間の socket がネットワーク名前空間ごとに別に
  なり、外の CLI と unit が互いのロックを見られなくなる(二重起動を検出できなくなる)からで
  ある。viewer と mcp が loopback で serve に届く必要もある。
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
写し先 ~/uniqnode-backup)はその利用者の home の下で、root の home ではない。出力は
ファイルに残す形で渡す(CLAUDE.md):

```
sudo ~/.local/bin/uniqnode install /work2/llm_playground_host_dir/uniqnode-store --system --serve-options "--embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank" --viewer-listen 0.0.0.0:7450 --listen-agent 10.10.128.1:7441 --agent-writable lamalium-notes --after wg-quick@wg1.service 2>&1 | ts '%Y-%m-%dT%H:%M:%S%z' | tee /tmp/uniqnode-install-system.log
```

(本番の実物。ストアは /work2 の下、読み口は wg1 のアドレス 10.10.128.1:7441、読み口から
書けるのはコレクション lamalium-notes の 1 つ(lamalium 側の書く先は共有 1 つで、名は
lamalium-notes。操作者の裁定 2026-09-05 と 2026-09-06)、wg1 の unit は wg-quick@wg1.service。既に user 単位で常駐しているなら、先に下の
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
- 確認の backup は `systemctl start uniqnode-backup.service`(unit は User= で走る)、fsck は
  `runuser -u <name> -- <bin> fsck <写し先>` で実行ユーザとして走らせる(root のプロセスで
  写し先を開くと、未封印の尻尾の切り詰めで root 所有のファイルができる)。install が root で
  作ったもの(バイナリの写し、無ければ作るストアと写し先のディレクトリ)は実行ユーザの所有に
  する。最後にストアと写し先の木を歩き、実行ユーザ以外の所有のものが 1 つでもあれば赤で
  止まる: 据え付けの後にストアと写し先に root 所有のものは残らない。

root で走らせないと、sudo で走らせる形と tee の例を言って断る。ストアを別のプロセスが開いて
いれば(user 単位の serve が典型)、止めて外す命令を添えて断り、黙って止めはしない。

### user 単位から移る

user 単位で動いているものを system 単位に載せ替える。ストアも写し先もバイナリも同じ場所の
まま、unit の置き場と走らせ方だけが変わる。1 命令で通す(移行の間、serve と viewer は止まる):

```
cargo build --release -p uniqnode && sudo target/release/uniqnode install /work2/llm_playground_host_dir/uniqnode-store --system --take-over-user-units --serve-options "--embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank" --viewer-listen 0.0.0.0:7450 --listen-agent 10.10.128.1:7441 --agent-writable lamalium-notes --after wg-quick@wg1.service --firewall-allow 10.10.128.4 2>&1 | ts '%Y-%m-%dT%H:%M:%S%z' | tee /tmp/uniqnode-install-system.log
```

中で何をしているか(手で同じことをするなら、この順):

1. `--take-over-user-units`: 実行ユーザの user 単位の 3 つの unit を
   `systemctl --user -M <name>@ disable --now uniqnode-serve.service uniqnode-viewer.service uniqnode-backup.timer`
   で止めて外す。終了コードは見ず、効果で判定する: 3 つの `is-active` が走っていない答え
   (inactive・failed・unit が無い)であること、そしてストアのロックが 10 秒以内に外れること。
   まだ走っていれば出力を添えて赤で止まる。指定が無いのに user 単位が動いていれば、install
   は「別プロセスが開いている」で止まり、その文言がこの指定と手で打つ命令を言う。
2. `--firewall-allow 10.10.128.4` の実物を決める(drop-in を描く前): root で `ufw status` を
   読み、`Status: active` なら ufw、そうでなければ(inactive、または ufw が無い)nft。nft なら
   規則ファイル /etc/systemd/system/uniqnode-serve.service.d/agent-door.nft を書く。中身は
   自分の表 `inet uniqnode` を空で作り、消し、作り直す形(何度読んでも同じ 1 表):
   `ip daddr 10.10.128.1 tcp dport 7441 ip saddr != { 10.10.128.4, 10.10.128.1 } counter drop`
   (自分の IP も許すのは、install の確認が同じ機械から 10.10.128.1 を源に届くため)。iptables-nft や
   docker の表には触らない(base chain は表ごとに評価され、どれかの drop が勝つ)。serve の
   drop-in に `ExecStartPre=+/usr/sbin/nft -f <その道>` を足す(先頭の + は User= に関わらず
   root で走らせる印)ので、nftables.service が無効な機械でも、serve を起こすたびに規則が入る。
   ufw も nft も無ければその旨で赤。
3. 据え付けと確認(上の「system 単位で起こす」と同じ。「確認:」が 5 本。`--agent-writable` が
   あればさらに 1 本、下の「読み口(--listen-agent)の確認」)。
4. firewall の効果を見て「確認:」を 1 本足す。ufw なら `ufw allow from 10.10.128.4 to 10.10.128.1
   port 7441 proto tcp` を入れて、`ufw status` の表にその行(10.10.128.1 7441/tcp、ALLOW、
   10.10.128.4)が載ったこと。nft なら `nft list table inet uniqnode` にその規則が載っている
   こと(入れたのは serve の起動そのもので、install はここで入れ直さない: 再起動のたびに同じ
   道で入ることの証拠になる)。
5. /tmp/uniqnode-install-system.log を読む。「確認:」の行が 6 本(`--agent-writable` があれば
   7 本)並び、最後に「次の刻み:」があれば据え付けは完了である。unit の状態は
   `systemctl status uniqnode-serve uniqnode-viewer uniqnode-backup.timer`。
6. user 単位の unit ファイル(~/.config/systemd/user/uniqnode-*)は disable しても残る。
   消すなら `rm ~/.config/systemd/user/uniqnode-*.service ~/.config/systemd/user/uniqnode-*.timer`
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

firewall は `--firewall-allow <addr>` で install が入れる。ufw が active ならその規則、そうで
なければ nft の自分の表 inet uniqnode(上の「user 単位から移る」の 2 と 4)。この機械には
ufw と nft の両方が入っているが、ufw は inactive で規則は iptables-nft(docker・fail2ban)が
持っており、INPUT の policy は accept なので、nft の道で「10.10.128.4 以外から 7441 は落とす」
を自分の表に置く形になる(実測 2026-09-06)。読み口は wg1 のアドレスにしか束縛しないので、
127.0.0.1:7441 は接続拒否のままである。

### 手で同じことをするなら(専用ユーザーで置く形)

install を使わず、専用ユーザー uniqnode と /var/lib/uniqnode で unit の既定のまま置く形:

```
sudo useradd --system --home-dir /var/lib/uniqnode --shell /usr/sbin/nologin uniqnode
sudo install -d -o uniqnode -g uniqnode -m 0750 /var/backups/uniqnode
sudo cp docs/mop/systemd/system/uniqnode-*.service docs/mop/systemd/system/uniqnode-*.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now uniqnode-serve.service uniqnode-viewer.service uniqnode-backup.timer
```

useradd で作るのはユーザー(とその主グループ)だけで、/var/lib/uniqnode は StateDirectory=
が初回の起動で作る。既にあるストアを使うなら `chown -R uniqnode:uniqnode` してから起こす
(所有者が違えば StateDirectory= が直すが、中身の数だけ時間がかかる)。写し先の
/var/backups/uniqnode は unit が作らないので install -d で先に作る(別のディスクに置くなら
上の drop-in)。enable するのは backup の timer であって service ではない(service は timer が
起こす)。

serve が走っているあいだ、CLI の ingest・embed・sync はストアのロックに阻まれる。取り込みは
REST(`PUT /v1/collections/{c}/documents/{name}`)で行い、CLI が要る作業は serve を止めて
`sudo -u uniqnode /usr/local/bin/uniqnode ingest /var/lib/uniqnode …` のように実行ユーザーで
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
(~/uniqnode-backup)が user 単位の systemd に載り、命令は効果を見てから戻る。待ち受け・
写し先・置き場は引数で変える:

```
uniqnode install <dir> [--listen <addr>] [--viewer-listen <addr>] [--serve-options "<引数列>"]
                       [--backup-dir <dir>] [--bin <path>] [--unit-dir <dir>] [--no-start]
                       [--listen-agent <addr>] [--agent-writable <c>]...
                       [--system [--user <name>] [--after <unit>]...]
```

| 引数 | 既定 | 意味 |
|---|---|---|
| `<dir>` | (必須) | ストア。/tmp の下は断る(unit の PrivateTmp=yes から見えない) |
| `--listen` | 127.0.0.1:7440 | serve の待ち受け。viewer の転送先もここから導く |
| `--viewer-listen` | 127.0.0.1:7450 | viewer の待ち受け |
| `--serve-options` | 空 | serve の追加の引数を 1 つの文字列で(例 `"--embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank"`) |
| `--backup-dir` | ~/uniqnode-backup | 写し先 |
| `--bin` | ~/.local/bin/uniqnode | 実行ファイルの置き場。走っている自分自身をここへ写す |
| `--unit-dir` | ~/.config/systemd/user(`--system` なら /etc/systemd/system) | unit と drop-in の置き場(テスト用) |
| `--no-start` | — | daemon-reload までで止める(unit を置くだけ) |
| `--listen-agent` | 無し | serve の読み口(第 2 の待ち受け。許可表の外は 403)。drop-in に UNIQNODE_AGENT_LISTEN を書き、ExecStart= の末尾に `--listen-agent ${UNIQNODE_AGENT_LISTEN}` を足す。--listen と別のアドレスで、ポートは固定 |
| `--agent-writable` | 無し | 読み口から書けるコレクション(複数可。`--listen-agent` があるときだけ)。drop-in に `Environment="UNIQNODE_AGENT_WRITABLE=<c1> <c2>"` を書き、ExecStart= の末尾に `--agent-writable <c>` を集合の数だけ並べる。確認に「許したコレクションへの PUT が門を越え、許していないものは 403」を足す(試し書きはしない) |
| `--system` | — | system 単位に据える(上の「system 単位で起こす」)。root で走らせる |
| `--user` | SUDO_USER | `--system` の実行ユーザ(unit の User=/Group=)。root は断る。`--system` のときだけ |
| `--after` | 無し | drop-in の `[Unit]` に After= と Wants= で書く unit(複数可。`--system` のときだけ) |
| `--take-over-user-units` | — | 据える前に実行ユーザの user 単位の常駐を止めて外す(`--system` のときだけ。上の「user 単位から移る」) |
| `--firewall-allow` | 無し | このアドレスからだけ読み口へ届く規則を入れて効果を見る。ufw が active なら ufw allow、そうでなければ nft の表 inet uniqnode(規則ファイルを drop-in の隣に置き、serve の ExecStartPre=+nft -f が起動のたびに入れる)。`--system` で `--listen-agent` があるときだけ |

出力は手順ごとに 1 行で、最後に確認した観測(serve と viewer 経由の /v1/status が返した
node_id、backup の写し先の fsck の件数)と backup の次の刻みが出る。実測(2026-09-05、
`--listen 127.0.0.1:7443 --viewer-listen 127.0.0.1:7453`):

```
install: バイナリ /home/op/uniqnode-install-probe-bin/uniqnode ← …/target/debug/uniqnode(32219056 bytes)
install: unit /home/op/.config/systemd/user/uniqnode-serve.service
…
install: systemctl --user daemon-reload: 済み
install: uniqnode-serve.service: enable、起こした(直前は inactive)
…
install: loginctl enable-linger: 済み
install: 確認: http://127.0.0.1:7443/v1/status と viewer http://127.0.0.1:7453 経由が同じ node_id 2dce4e60… を返した(1 ms)
install: 確認: uniqnode-backup.service を 1 回走らせ、写し先 /home/op/uniqnode-install-probe-backup を開いて fsck: objects 0 refs 0 errors 0
install: 次の刻み: Sun 2026-09-06 00:00:00 JST 6h left … uniqnode-backup.timer uniqnode-backup.service
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
2. unit 4 本(serve・viewer・backup の service と backup の timer)を `--unit-dir` に書く。
   手でなら:

   ```
   mkdir -p ~/.config/systemd/user ~/uniqnode-backup
   cp docs/mop/systemd/user/uniqnode-*.service docs/mop/systemd/user/uniqnode-*.timer ~/.config/systemd/user/
   ```

3. 3 つの service に drop-in `<unit>.d/override.conf` を書く。中身は PATH(install 自身の
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
6. `systemctl --user enable` と `systemctl --user restart` を uniqnode-serve.service、
   uniqnode-viewer.service、uniqnode-backup.timer に(restart は止まっている unit も起こす
   ので、初回と更新で同じ手順)。手でなら
   `systemctl --user enable --now uniqnode-serve.service uniqnode-viewer.service uniqnode-backup.timer`。
7. `loginctl enable-linger`。取れなければ警告して続ける。無ければ、最後のセッションが閉じた
   ときに user 単位のマネージャごと止まり、backup の timer の刻みも来ない。
8. 確認(下の「効いていることの確かめ方」と同じ観測): serve の /v1/status と viewer 経由の
   /v1/status が同じ node_id を返すまで短い間隔で待ち(上限 90 秒。serve の unit が failed
   に落ちたら待たずに言う)、`--listen-agent` があれば読み口の /v1/status が同じ node_id を
   返し POST /v1/admin/gc(dry_run)が 403 であることを同じ上限で待ち、`--agent-writable` が
   あれば書く口を書かずに確かめ(上の「読み口(--listen-agent)の確認」)、
   `systemctl --user start uniqnode-backup.service` を 1 回走らせ、写し先をストアとして開いて
   fsck する(backup 命令の最後の検証と同じ関数。system 単位では実行ユーザで
   `<bin> fsck <写し先>` を起こし、最後にストアと写し先の所有者を見る)。
9. `systemctl --user list-timers uniqnode-backup.timer` の表を載せる。

unit だけを手で置いたときのデータディレクトリの既定は %S/uniqnode で、実際にどこへ
置かれたかは起動時のログの行「ログを <dir>/logs/serve.log に残す」が言う
(`journalctl --user -u uniqnode-serve`)。ProtectHome=read-only なので、home の下の別の場所
を使うなら ReadWritePaths= を添える(上の「unit の読み方」。install はこれを drop-in に書く)。

## 効いていることの確かめ方

設定は編集した時点ではなく、効果を見た時点で完了である(should/0116)。

```
systemctl --user status uniqnode-serve uniqnode-viewer   # system 単位なら --user を外す
curl http://127.0.0.1:7440/v1/status                      # serve が答える
curl http://127.0.0.1:7450/v1/status                      # viewer が serve へ転送して同じ答え
```

埋め込みを足したなら、起動直後の journal に `uniqnode: embedding: bge-m3 (http://…)` の
行があること、検索の応答の `method` が hybrid になる(または `degraded` が理由を言う)
ことまで見る。

backup は刻みを待たずに 1 回起こして、写しができたことを写し先で見る:

```
systemctl --user list-timers uniqnode-backup.timer     # 次の発火時刻が出る
systemctl --user start uniqnode-backup.service          # 一回きりに走る(終わるまで戻らない)
journalctl --user -u uniqnode-backup.service -n 5       # copied … / backup: … / verify: … errors 0
uniqnode fsck <backup_dir>                              # unit の外からも写しが開けて緑
```

実測(user 単位、serve が同じストアのロックを持ったまま): 1 回目は未封印の pack と node_key を
写して `verify: objects 4 refs 0 errors 0`、serve 経由で 1 件足した後の 2 回目も同じ 2 つだけを
写し直して緑、その間 serve は走り続けた。

## 停止

```
systemctl --user stop uniqnode-viewer uniqnode-serve
```

systemd は SIGTERM を送り、uniqnode はそれを受け取る手続きを持たないので即座に落ちる。
それでよい理由は、ストアが書き込みのたびに fsync 済みで、途中で裂かれても次の open が
回復するからである(上の Restart= の項と同じ根拠)。処理中の要求は応答なしに切れる。
実測では stop の完了に 30 ms、ポートの解放は即時、次の起動でオブジェクト数はそのまま
だった。

viewer だけ止めても serve は走り続け、serve だけ止めても viewer は残って 502 で「先に
serve を起こす」と言う。

backup を止めるのは timer である(`systemctl --user stop uniqnode-backup.timer`。刻みを
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
sudo systemctl restart uniqnode-serve uniqnode-viewer
```

install(1) は走っている実行ファイルを一度 unlink してから置くので、走行中に差し替えられる
(cp は Text file busy で断られる。`uniqnode install` は隣に書いて rename する)。走っているプロセスは古いイメージのまま動き続けるので、
restart で新しいものに替わる。restart は停止と同じく SIGTERM で落として起こし直す。

## ログの見方

同じ行が 2 箇所に残る。

| 場所 | 見方 | 中身 |
|---|---|---|
| journal | `journalctl --user -u uniqnode-serve -f`(system 単位なら `sudo journalctl -u …`) | 標準エラーと標準出力 |
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
  ので、journal にだけある(`journalctl --user -u uniqnode-backup`)。写せなかった・検証が赤
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
  `systemctl reset-failed uniqnode-serve` と `systemctl start uniqnode-serve`。
- viewer はロックを取らないので、同じストアに何本でも起こせる。衝突するのはアドレスだけである。
- serve が走っている間に CLI の ingest・embed・sync を叩くと、同じ「別プロセスが開いて
  いる」で断られる。REST を使うか、serve を止めてから行う(上の「system 単位で起こす」)。

## 2 つ目のストアを同じ機械で

この unit は 1 台に 1 ストアの形である。2 つ目を起こすときは unit を複製せず、
`uniqnode-serve@.service` のようなテンプレート unit にして、インスタンス名から
データディレクトリと待ち受けを引く形に直す(should/0118: 増やすのは設定の 1 項目で
あって、ファイルの写しではない)。必要になったときに、この文書と docs/mop/systemd/ を
その形に置き換える。

# LLM_UNITS: vega の 3 つの llama-server を systemd の unit にし、8082 を nft と CORS で締める(計画)

<a id="0ee79be6-f2bd-400e-8340-b9e60da6c9a7"></a>

版: 第 8 版(2026-10-01)。第 7 版への Codex のレビュー(中 1: 置き場のディレクトリだけを確かめ、3 つのログファイルそのもの(シンボリックリンク・普通でないもの・持ち主と mode)を確かめていない、低 1: ディレクトリの確かめが 0700 を通し、戻す命令の `>>` が作るログの mode が sudo の umask に左右される)を取り込んだ。据え付け・切り替え・戻すの 3 つの命令に同じ prepare_logs を置き、何かを止める前に、置き場を root:root 0755 に、3 つのログを root:root 0644 の普通のファイルに用意する(無ければ作り、在れば中身を切らずに直し、シンボリックリンクなどは止まる)。第 7 版(2026-10-01)。第 6 版への Codex のレビュー(高なし、第 5 版の指摘は全部解消、中 2: 同じモデルの残りの探しが読めないのを「無い」とみなし、差し替えで ` (deleted)` の付いた exe を見落とす・`chown -h` では systemd が root で append: の先を開くときにシンボリックリンクを辿るのを止められない)を取り込んだ。読めないものは失敗として起こす前に止め、exe の末尾の ` (deleted)` を落として比べる。unit のログは root の持ち物の /var/log/llama-server/ へ移し、chown と hikalium の持ち物への直しは外した。第 6 版(2026-10-01)。第 5 版への Codex のレビュー(高なし、中 3: 戻す命令が /slots の読めないのを空きとみなす・ExecStart= の比べが引数の境目を見ない・口を閉じたがまだ終わらない前の版の見落とし、低 1: analysis の「日々使われている」)と、ログの持ち主と mode の指摘を取り込んだ。戻す命令は読めないときも期限まで読み直して止まり、待たずに止めるのは `--force` を明示したときだけにした。ExecStart= は D-Bus の構造で比べ、同じモデルを読む残りのプロセスを探して待つ。ログは据え付けで持ち主と mode を直し、各 unit の `ExecStartPre=+/usr/bin/chown -h` が消された後も持ち主を直す。第 5 版(2026-10-01)。第 4 版への Claude のレビュー(高なし、中 1: `NoNewPrivileges=yes` が再起動の後の最初の GPU の利用者として setuid の nvidia-modprobe を止めうる、低 9: 出力の行き先・止める前の unit の argv の比べ・戻す側の待ちと上限・keeper の利用・analysis の根拠・ufw・止まる時間の表記・7441 の根拠)を取り込んだ。`NoNewPrivileges=` を外し、出力は `StandardOutput=append:` で今と同じログへ続ける。第 4 版(2026-10-01)。第 3 版への Codex のレビュー(高なし、中 3: pid の使い回しを受け入れる理由の誤り・1 行に並ぶ複数の持ち主の見落とし・連結された JSON を受け入れる /slots の判定、低 2: persona-review の経路・待ちの上限が実の経過時間でない)を取り込んだ。第 4 版への Codex の再レビュー(r4)は高・中なし、低 1(上限は概算で少し超えうる)で、段取りの節に概算である旨を書いた。第 3 版(2026-10-01)。第 2 版への Codex のレビュー(高 1: SIGTERM の宛先の固定、中 4: 身元の読みの失敗・/slots の JSON の検査・待ち受けの行ごとの検査・戻す命令の打ち直し、低 2: 8082 の補助の用途・固定の生成元の CORS の説明)を取り込んだ。高 1 は、身元の読み直しを kill の直前に寄せ、残るミリ秒の隙と受け入れる理由を書き、閉じる手段を問いにした。第 2 版(2026-10-01)。第 1 版への Codex のレビュー(高 2: 止める直前の身元の確かめ直し・打ち直しで確かめを
飛ばす、中 5: /slots の読めない場合・待ち受けの持ち主の決め方・据え付けの後の再起動・戻す側の上限と GPU・
After= の意味、低 3: 表が入り直す条件・手順ファイルの既定・plan に置く事実の量と参照の形)と、操作者の規則(2026-10-01T00:01:28Z、最終目標との
つながりを書く)を取り込んだ。観測は docs/analysis へ分けた。
状態: 設計草案。下の命令はまだ誰も打っていない。操作者が打つまで、手で起こした 3 つのプロセスと
ファイアウォールは今のままである。

出所は操作者の裁定(2026-09-30T23:41:46Z、lamalium の統括が中継)。vega の gpt-oss-120b の
llama-server は 0.0.0.0:8082 で認証なし・CORS は全部許しで動いている。裁定は 2 つ:
(1) 8082 を、uniqnode の他の口と同じく nft で決めた相手だけに限り、CORS を締める。
(2) LLM の 3 つのプロセスを systemd の unit にして、機械の再起動の後に戻るようにする。
あわせて、立っている指示「設定は変えないでください」(VRAM・gpt-oss・llama-server の設定)を守る。
unit は今の命令行(バイナリ・モデル・引数・口)をそのまま再現し、変わるのは見張り方(誰が起こし、
落ちたら誰が起こし直すか)だけである。意図した振る舞いの変化は裁定の 2 つ、8082 の nft と CORS に
限る。CORS を締めるには llama-server の引数を 2 つ足す必要があり、これが指示への唯一の例外である
(下の「CORS」)。

読み手は、vega で命令を打つ操作者と、その結果を確かめるセッション。uniqnode の unit と nft の表の
作りは [docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2) にあり、ここはそれに合わせる。

## 最終目標とのつながり

最終目標は、手元の GPU で lamalium を毎日実際に使えることである(2026-10-01 の操作者の規則で、各計画がここに
どうつながるかを書く)。

- 支える日々の仕事:
  - 8082 の gpt-oss-120b は lamalium の helpdesk の脳である。手元の lamalium の写し(ca1fdca、2026-08-15)では、
    roles/helpdesk/role.toml が `model = "gpt-oss-120b"` で vega に pin し、host/llm-servers.toml と
    host/backend-proxy.toml が vega の 8082 を `10.100.0.1:11436` として渡す。llm-servers.toml の vega の `models` は
    gpt-oss-120b だけなので、既定の gemma4 を選ぶ worker の席は 8082 へ振り分けられない(禁止ではなく、モデルの
    選び方の結果)。補助の用途として、CLI の persona-review は既定の URL `http://10.10.128.1:8082` で 8082 へ直接
    (container/src/cli.rs)、qa-translate と prompt-probe(host/prompt-probe-delivered.toml)は `10.100.0.1:11436`
    越しに 8082 を使う。viewer の keeper の外部レビューも、`http://10.100.0.1:11436` の gpt-oss-120b を呼ぶ
    (container/src/keeper.rs:270)。
    2026-10-01 に crystal 移行のセッションが、今の crystal の設定(roles、models.toml、
    /var/lib/lamalium/etc/server-proxy.toml)と lamalium の main 7318a6b9 を読んで確かめた(問いの 6 の答え)。
    呼び手はこの 5 つで全部で、送信元はどれも crystal の 10.10.128.2 である。vega を pin する role は helpdesk だけ。
    persona-review と prompt-probe は、今は host_llm_target で server-proxy.toml の上流を引き、crystal のホストから
    10.10.128.1:8082 へ直接つなぐ(既定の URL の直書きは無くなった)。keeper の行は keeper.rs:272 に移った。
    models.toml は vega の llm_instance を宣言したままなので、今後の role や task が名指しても、同じ 10.10.128.2 から来る。
    proxy は接続ごとのログを残さないので、呼び手ごとの頻度は分からない。
    orion 側の呼び手(2026-10-01T18:46+09:00、orion のセッションが読むだけで確かめた): vega の 8082 へ繋ぐのは
    orion の lamalium-server-proxy.service の 1 プロセスだけで(/var/lib/lamalium/etc/server-proxy.toml:27-30、
    10.100.0.1:11436 → 10.10.128.1:8082)、送信元は wg の 10.10.128.4 である。直近に呼んだのは agent の r18vegoss
    (2026-10-01 03:35、llm_start 4 回)と chat の subtask(2026-09-27 に計 45 回。子は done か halted だが、chat が
    vega を選ぶ経路は今も有効)。helpdesk・pvve0912・bench3 の role、keeper の講評と QA の翻訳、paused のタスク 4 件は
    宣言だけで、直近 7 日は呼んでいない。cron・timer・proxy 以外の unit・スクリプトからの呼び出しは無く、確かめた時点で
    8082 宛ての接続は 0 本だった。openclaw-gateway の 192.168.100.22:8082 は aero で、vega ではない。したがって、
    この計画の許可(loopback・10.10.128.2・10.10.128.4)のままで、orion の呼び手も全て通る。
    /work2/llm/server-gptoss.log には 2026-07-17 からの約 76 日で 2,239 回の処理の始まり(`launch_slot_` の行。1 日
    平均約 30)があり、最後は 2026-10-01 03:36 JST。どの用途の分かはログから分からない。今の crystal の lamalium の
    設定は読めていない(問いの 6)。切り替えの後は、この根拠は /var/log/llama-server/gpt-oss-120b.log に残る
    (古いログは履歴として残し、伸びなくなる。下の「設計」)。
  - 8083 の bge-m3 と 8084 の reranker は、uniqnode の検索の埋め込みと順位の付け直しである。本番の serve の
    override.conf が `--embed`・`--rerank` で指し、15 分の標本でも 127.0.0.1 からの接続が続いていた
    (/work2/llm/server-bge-m3.log の最終更新は 2026-10-01 08:35)。lamalium の読み口(7441)の検索もこれを通る:
    読み口は同じ serve のプロセスが `--listen-agent 10.10.128.1:7441` で開くもので、同じ override.conf の
    `UNIQNODE_SERVE_OPTIONS` が `--embed`・`--rerank` を渡している(/etc/systemd/system/uniqnode-serve.service.d/override.conf
    の 15〜16 行と 24 行、[docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2) の据え付けの命令も同じ形)。
- 今より良くなること: 機械を再起動しても 3 つが戻る(今は手で起こし直すまで、helpdesk は答えず、uniqnode の検索は
  BM25 だけに落ちる)。8082 が LAN と docker の橋に認証なしで開いている状態と、どのサイトの頁からでも応答を読める
  CORS が閉じる。
- 費用: 切り替えで 8082 が 40 秒〜数分止まり、その間の helpdesk と補助の CLI の要求は失敗する(8083・8084 は数秒で、検索は劣化
  して答え続ける)。nft の許可から漏れた呼び手があれば、気づくまで落ちる(`dropped_sources` で見つける)。保守は、
  llama-server やモデルを替えるときに unit の `ExecStart=` と切り替えの命令の argv を一緒に直すことと、許す相手の
  出入り(orion を外す、など)のたびに firewall.nft を直すこと。

## 前提

観測の全体(命令行・環境・上限・GPU・llama-server b10025 のソースの読み・利用者・ファイアウォール)は
[docs/analysis/20261001-vega-llm-processes.md](#0a4b121f-6ba8-4bc2-87ef-d804ff3401fa) にある。計画が拠る点だけ:

- 3 つ(8084 の reranker・8083 の bge-m3・8082 の gpt-oss-120b)は同じバイナリ
  /work2/llm/vulkan/llama-b10025/llama-server で、hikalium の端末の session scope の下で手で起こしてある。
  再起動で戻らない。
- 8083・8084 は 127.0.0.1 に束縛し、uniqnode の serve だけが使う。serve は届かないと劣化して答え続ける。
- 8082 は 0.0.0.0(wg1・br0 の LAN・docker の橋・loopback の全部)に開いている。分かっている呼び手は lamalium
  (orion 10.10.128.4 から crystal 10.10.128.2 へ移している途中)で、wg1 越しに来る。
- llama-server は束縛をモデルの読み込みより先に行い、塞がっていればすぐ exit 1 で終わる。SIGTERM で行儀よく
  終わる。`/health` は読み込み中 503、済めば 200。
- 既定の CORS(`*` と資格情報の許し)は要求の Origin をそのまま返す。`--cors-origins` は 1 つの生成元か
  `localhost` しか効かない(カンマの一覧は 1 つのヘッダにそのまま入る)。
- nftables.service は disabled で、uniqnode の表は unit の `ExecStartPre=+/usr/sbin/nft -f …` が起動のたびに入れる。

## 設計

### system 単位にする

unit は /etc/systemd/system/ に置く system 単位で、`User=hikalium`・`Group=hikalium` で走らせる。user 単位
(linger は既に yes)にしない理由:

- uniqnode の本番が 2026-09-06 の操作者の裁定で system 単位に移っており、lamalium の規律(向こうの policy
  0108: ホストの常駐は system unit で `User=hikalium`)とも同じ形になる。
- 8082 の nft の表を unit の起動で入れるには root が要る。system 単位なら `ExecStartPre=+` で root として
  走らせられる(uniqnode と同じ仕組み)。user 単位からは入れられない。
- /work2 の載りを待つ(`RequiresMountsFor=`)などの、system の unit への順序が書ける。

### unit の名と中身

| unit | 口 | 置き場 |
|---|---|---|
| llama-server-bge-reranker-v2-m3.service | 127.0.0.1:8084 | /etc/systemd/system/ |
| llama-server-bge-m3.service | 127.0.0.1:8083 | /etc/systemd/system/ |
| llama-server-gpt-oss-120b.service | 0.0.0.0:8082 | /etc/systemd/system/(nft の規則は隣の llama-server-gpt-oss-120b.service.d/firewall.nft) |

全文は下の「据え付けの命令」の中にあり、そこだけに置く(2 か所に書くと食い違うため)。同じ中身の単体の
ファイルで `systemd-analyze verify` を通した(警告なし)。3 つに共通の決めごと:

- `ExecStart=` は今の命令行と同じ引数を同じ順に並べる。gpt-oss-120b だけ末尾に CORS の 2 つを足す。systemd は
  `ExecStart=` の中で C の逃がしを解くので、`-ot` の値は `"\\.ffn_(gate|up|down)_exps\\.=CPU"` と書く
  (解いた結果が今の `\.ffn_(gate|up|down)_exps\.=CPU`)。JSON は単引用符で囲む。引数が今と同じかは、切り替えの
  命令が起動後の `/proc/<MainPID>/cmdline` を期待の argv とバイト単位で比べて確かめる。起こす前にも、systemd が
  読み込んだ `ExecStart=` を D-Bus の構造のまま(`busctl --json=short get-property … ExecStart` の path と argv の
  配列)取り、NUL で区切って期待の argv とバイト単位で比べる。空白で繋いだ文字列と違い、引数の境目まで比べられる
  (getty@tty1.service の `-o '-p -- \\u'` で、境目の違いを見分けることを root なしで確かめた)。
- `Type=exec`。llama-server は sd_notify を持たないので、読み込みの完了は systemd には見えない。
  `TimeoutStartSec=` は exec までの時間にしか効かないので書かない。読み込みの完了は切り替えの命令が `/health`
  で待つ。
- `Environment=LD_LIBRARY_PATH=/work2/llm/vulkan/llama-b10025`。今の値から末尾の `:`(作業ディレクトリを探す
  要素)だけを落とした。RUNPATH が同じディレクトリなので、読む .so は変わらない。8084 にも同じ値を置く。
  他の環境は systemd が User= から作るもの(HOME=/home/hikalium など)だけになる。`LLAMA_ARG_*` は置かない。
- `WorkingDirectory=/work2/llm`。8084 の今の作業ディレクトリ(uniqnode の作業ツリー)はたまたまのもので、
  命令行のパスは全部絶対パスなので効かない。
- `LimitNOFILE=1048576`・`LimitMEMLOCK=67581657088`: 今の値と同じ(system 単位の既定より大きい)。
- `RequiresMountsFor=/work2/llm`: /work2 は別のファイルシステム。
- 閉じ込めは足さない。`NoNewPrivileges=yes` も置かない: 今の 3 つは NoNewPrivs 0(/proc/<pid>/status)で、
  機械の再起動の後に unit が最初の GPU の利用者になると、ドライバは setuid の /usr/bin/nvidia-modprobe で
  /dev/nvidia-modeset などのノードを作る。NoNewPrivileges= はその setuid を効かなくするので、ノードが無いまま
  Vulkan が lvp(CPU)へ黙って落ちうる(誤りは出ず、設定の変化になる)。`PrivateDevices=` は /dev/nvidia* を隠し、
  `ProtectHome=` は ~/.cache/nvidia を隠す。どれも、効くかどうかを再起動を含めて実機で確かめてからでないと足せない。
  GPU を掴んだかは、切り替えの命令と再起動の後の確かめが、MainPID が `nvidia-smi --query-compute-apps` に載る
  ことで見る。
- 出力は `StandardOutput=append:/var/log/llama-server/<名>.log` と `StandardError=inherit` で、root の持ち物の
  ディレクトリ /var/log/llama-server/(root:root 0755)の下のファイルへ追記する(systemd 249 で使える)。8082 は
  gpt-oss-120b.log、8083 は bge-m3.log、8084 は bge-reranker-v2-m3.log。journal には起動と終了の記録だけが残り、
  llama-server の出力は残らない。
- 置き場を今の /work2/llm から移す理由: systemd は append: の先を、User= に切り替える前に root で開き、無ければ作る。
  249 はこのときシンボリックリンクを辿る(O_NOFOLLOW を付けない)ので、hikalium が書ける /work2/llm に置くと、
  そこに置かれたシンボリックリンクの先へ root が追記しうる。hikalium の書けないディレクトリに置けば、ファイルも
  リンクも root しか置けない。書くのは systemd(と戻す命令の root のシェル)が開いた fd だけなので、持ち主は
  root のままでよい。読むのは hikalium も adm の組の誰でもできる。
- 置き場とファイルは、据え付け・切り替え・戻すの 3 つの命令が、何かを止める前に同じ手順(prepare_logs)で用意する。
  ディレクトリは root:root 0755 の本物のディレクトリにする(無ければ作り、root:root で組と他人に書き込みを許さない
  なら、0700 のように狭くても 0755 に直す。シンボリックリンク・ディレクトリでないもの・root 以外の持ち物・組か他人が
  書けるものなら止まる)。3 つのログは root:root 0644 の普通のファイルにする(無ければその中に `install -m 0644`
  で空のまま作り、在れば中身を切らずに持ち主と mode を直す。シンボリックリンクや普通のファイルでないものが在れば
  止まる)。こうしておけば、systemd や戻す命令の `>>` がファイルを作ることはなく、mode が sudo の umask に
  左右されない。据え付けでは /etc に何も置かずに、切り替えと戻すでは何も止めずに止まる。
- 今の /work2/llm/server-gptoss.log・server-bge-m3.log(と 8084 の /tmp/rerank-server.log)は履歴として残し、
  動かさない。切り替えの後の使われ方の根拠(`launch_slot_` の行など)は /var/log/llama-server/ の新しいログに
  残る。
- ログの回し(logrotate)は今は置かない。大きさは server-gptoss.log が約 76 日で 4 MB、server-bge-m3.log が
  約 45 日で 28 MB(年に数百 MB)で小さい。要るようになったら見直す。
- `Restart=on-failure`。gpt-oss-120b は `RestartSec=30s` と `StartLimitBurst=3`/`StartLimitIntervalSec=30min`
  (失敗のたびに 63 GB を読み直すので、回数を絞る)、小さい 2 つは 10 秒と 10 分に 5 回。
  `RestartPreventExitStatus=` は置かない: 読み込みの失敗も exit 1 で、起動直後の GPU の一時の失敗は起こし
  直しで直りうる。口が塞がっているときも exit 1 だが、束縛が読み込みより先なので、上限の回数だけ即座に落ちて
  止まるだけで、重い読み込みは起きない。
- `TimeoutStopSec=`: gpt-oss-120b は 2 分、小さい 2 つは 1 分。
- `WantedBy=multi-user.target`。

### 起動の順とメモリ

- gpt-oss-120b は `After=llama-server-bge-m3.service llama-server-bge-reranker-v2-m3.service`(順序だけで、
  依存は付けない)。`Type=exec` なので、これが決めるのは exec の順だけで、小さい 2 つの読み込みの完了は待たない。
  機械の起動のときは 3 つの読み込みが重なりうる。重なっても VRAM は 3 つで約 4.1 GiB で 8 GiB に収まり、
  ディスクの取り合いで数秒遅れるだけなので、読み込みを直列にする仕組み(読み込みの完了を待つ ExecStartPost= など)は
  足さない。
- メモリは gpt-oss-120b の 66.5 GiB が大半で、機械は 503 GiB。`MemoryMax=` などは置かない(今の設定に無い
  上限を足すと、設定の変化になる)。
- 切り替えは 1 つずつ、前の版が終わったのを確かめてから unit を起こす。口が空いていても、同じ利用者の同じ
  バイナリで同じモデルを読むプロセス(口を閉じたがまだ終わらず VRAM を持つ前の版など)が残っていれば、それが
  終わるのを最長 120 秒待ち、終わらなければ起こさずに止まる。打ち直しのときも同じ。これで、同じモデルが 2 本
  ぶんメモリと VRAM に載る時間を作らない。戻す命令も、手の形で起こす前に同じ確かめをする。
- uniqnode の serve からこれらへの依存は付けない(uniqnode の unit は変えない)。再起動の直後、serve が
  bge-m3 より先に起きても、届くまでの数秒は BM25 だけに劣化して理由を返し、届けば戻る。

### 二重起動の防止

- 据え付けの命令は unit を置いて daemon-reload するだけで、enable も start もしない。手で起こした 3 つが
  走っている間、unit は起きない。据え付けの後で機械が再起動しても、enable していないので何も起きない。その
  ときは手の版も表も消えているので、切り替えの命令を打つ。切り替えの命令は、表が無ければ据え付けが置いた
  firewall.nft を入れてから進む。
- 切り替えの命令は、口を持っているプロセスが「手で起こした版」(uid が hikalium、cgroup が利用者のセッションの
  scope、argv が一致)であることを全部の口で確かめてから、1 つずつ止めて unit を起こす。知らないプロセスが口を
  持っていれば、何も止めずに止まる。口だけでは、口を閉じたがまだ終わらないプロセスを見落とすので、口ごとに
  /proc の全プロセスから hikalium の同じバイナリで同じモデル(argv の `--model` の次)を読むものを探し、口の持ち主と
  unit の MainPID の他にあれば、最長 120 秒待って、残れば止まる(事前の確かめ、unit を起こす直前、起こした後の
  確かめの 3 か所。打ち直しでも同じ)。走査の途中で消えたプロセスとゾンビは数えないが、生きているのに持ち主・
  バイナリ・引数を読めないプロセスがあれば「無い」とみなさず、起こす前に止まる。バイナリが差し替えられて
  /proc/<pid>/exe の末尾に ` (deleted)` が付いた前の版も、末尾を落として同じバイナリとみなす。
- 事前の確かめから SIGTERM までの間(処理中の要求を待つ最長 15 分を含む)に pid が使い回されないよう、事前の
  確かめで uid と起動の刻み(/proc/<pid>/stat の 22 番目)を控え、SIGTERM の直前に、口の持ち主・uid・バイナリ
  (/proc/<pid>/exe)・cgroup・argv を確かめ直し、身元(uid と起動の刻み)は kill の 1 つ前の命令で読み直す。
  どれかが違えば止めずに止まる。SIGTERM の後も、同じ pid に別の身元が見えたら止まる。
- 残る隙: 身元の読み直しと kill の間(同じシェルの続く 2 つの命令。通常は短いが、スケジューラの中断などが
  あり上限の保証はない)に、対象が自分で終わり、同じ番号が別のプロセスに渡れば、そのプロセスに SIGTERM が
  届く。pid の割り当て位置は対象が動いている間も進むので、番号が一巡するまでの生成の回数は安全の根拠に
  ならない。この計画は、通常は短いが上限の保証はない競合区間が残り、別プロセスへ SIGTERM を送る可能性を
  受け入れる。受け入れる理由は、長い待ちの後の確かめ直し・バイナリの検査・直前の起動の刻みの比較で、対象が
  その区間でちょうど終わり番号が渡るという重なりが要ることと、届くのが SIGTERM(SIGKILL ではない)であること。
  完全に閉じるには pidfd(pidfd_open で取ってから身元を確かめ、同じ pidfd へ pidfd_send_signal)が要るが、
  bash から呼べず、補助プログラムを足すのはこの計画の外になる(問いの 7)。
- 口の持ち主は `ss -Hltnp` の答えを行ごとに読んで決める。`ss` の失敗、pid の読めない行が 1 つでもあること、
  2 つ以上の持ち主(1 行に共有の持ち主が並ぶ場合も、行の pid を全部取って数える)は「空き」と区別して止まる。
- それでも unit と手の版が重なったときは、束縛の失敗で unit の側がすぐ落ちる。

### 8082 の nft

表 `inet llama_server_gpt_oss_120b` を、uniqnode と同じ形(unit の `ExecStartPre=+/usr/sbin/nft -f <ファイル>`
で起動のたびに入れる。ファイルは空の表を作り、消し、作り直すので、何度読んでも同じ 1 表になる)で入れる。
中身(据え付けの命令が書く firewall.nft):

- 8082 への TCP を、loopback から(`iifname "lo"`。vega の中からは宛先のアドレスに関わらず lo を通る)と、
  wg1 から来た 10.10.128.2(crystal)・10.10.128.4(orion)だけ通し、それ以外は drop する。
- 通す相手を wg1 に限るのは、br0 の LAN から 10.10.128.x を名乗る偽の送信元を通さないため(wg1 では
  WireGuard が送信元を鍵で縛る)。
- 規則は宛先のアドレスを見ない。0.0.0.0 に束縛しているので、br0・docker の橋・wg1 のどれで来ても同じに
  扱う。docker のコンテナから vega の橋のアドレスへ来る接続も落ちる(問いの 1)。
- IPv6 は束縛していないが、最後の規則が `inet` の表で IPv6 も落とすので、将来 `--host ::` になっても開かない。
- 落とした IPv4 の送信元を、動的な集合 `dropped_sources`(30 日で消える)に残す。誰を落としたかを後で
  `nft list table inet llama_server_gpt_oss_120b` で読める(root)。通した数と落とした数も counter で残る。
  unit の起動(`ExecStartPre=`)は表を作り直すので、数と集合はそこで空に戻る。切り替えの命令は、作り直す前の
  表を記録に写す。
- uniqnode の表とは名が別で、互いに消し合わない。`accept` はその表の中で終わるだけで、他の表の drop は
  そのまま効く(base chain は表ごとに評価される)。uniqnode の表は 744x しか見ないので 8082 には触れない。
- 表は、gpt-oss-120b の unit が起きるたびに入る(手の start・restart、落ちた後の起こし直し、enable なら機械の
  起動)。`ExecStartPre=` は `ExecStart=` より先なので、unit が 8082 を開く前に表がある。unit を使わずに手で
  起こしている間に機械を再起動すると、表は無い(今と同じ)。
- 裁定の (1) は unit と独立なので、据え付けの命令が表をその場で入れる。手で起こした gpt-oss-120b は
  止めずに、その時点から限られる。
- 自分の名前空間(`unshare -rn`)で `nft -c -f` と 2 回続けての `nft -f` を通した(vega の表には触れていない)。

### CORS

gpt-oss-120b の命令行の末尾に `--cors-origins http://localhost:8082 --no-cors-credentials` を足す。これが
「設定は変えない」の唯一の例外で、裁定の (1) の「CORS を締める」に当たる。b10025 の `--help` に在る引数で
あることを確かめた(`--cors-origins`・`--cors-methods`・`--cors-headers`・`--cors-credentials`/
`--no-cors-credentials`)。環境変数(`LLAMA_ARG_CORS_ORIGINS`)でも同じことができるが、命令行に出る方が `ps` で
見え、比べられるので引数にする。

- 効果: `Access-Control-Allow-Origin` は常に `http://localhost:8082` になり(要求の Origin を映さない)、
  OPTIONS は `Access-Control-Allow-Credentials: false` を返す。他のサイトの頁のスクリプトは、応答を読めず、
  JSON の POST は事前の問い合わせで止まる。起動時の「CORS is set to allow all origins」の警告も消える。
- 限界: サーバは許さない生成元の要求も処理する。事前の問い合わせの要らない形の POST は、他のサイトの頁からも
  届いて処理される(応答は読めない)。口へ届く相手を限るのは nft である。
- 変わらないもの: lamalium の呼び手(Rust の ureq と TCP の転送)はブラウザではないので CORS に関わらない。
  vega の Web UI を http://10.10.128.1:8082/ から開いたときは同じ生成元なので CORS に関わらない。
  ssh の転送で http://localhost:8082/ から開いた頁も通る。
- `localhost` の特別な値にしない理由: Origin を送らない呼び手(lamalium の全部)の要求ごとに警告が 1 行
  ログ(/var/log/llama-server/gpt-oss-120b.log)に出る。1 つの生成元を書く形なら出ない。
- `--cors-methods` と `--cors-headers` は既定のまま。締めても、上の効果に足すものが無い。
- API キーは裁定に無いので足さない(lamalium の呼び手の側も変える必要がある。問いの 3)。

## 段取り

命令は全部 vega で打つ。sudo を使えるどの利用者がどのディレクトリから貼っても同じに動くように、パスは全部
絶対パスにしてある。出力は時刻を付けて /tmp の記録に追記し、最後に `exit status:` を足す
([docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2) の「旧い名の unit から移る」と同じ形)。打った後は、
記録を確かめるセッションが読む。

### 1. 据え付け(unit を置き、8082 を nft で限る)

手で起こした 3 つには触れない。unit の出力先のディレクトリ /var/log/llama-server と 3 つのログを prepare_logs で
root:root 0755 と root:root 0644 に用意し(在るものが本物のディレクトリ・普通のファイルでなければ、/etc に何も
置かずに止まる)、
unit を /etc/systemd/system/ に置いて daemon-reload し、8082 の表を入れる。
enable も start もしない。書く前に /run の下の作業用のディレクトリで `systemd-analyze verify` と `nft -c` を通し、
通らなければ /etc に何も置かずに止まる。打ち直してよい(同じ中身で上書きし、表を作り直す)。vega で打つ命令:

```
( set -o pipefail; sudo bash -s <<'EOF' 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee -a /tmp/llama-server-units-install.log; echo "exit status: $?" | /usr/bin/tee -a /tmp/llama-server-units-install.log )
set -euo pipefail
dir=/etc/systemd/system
units=(llama-server-bge-reranker-v2-m3.service llama-server-bge-m3.service llama-server-gpt-oss-120b.service)
nft_file=$dir/llama-server-gpt-oss-120b.service.d/firewall.nft
staging=$(/usr/bin/mktemp -d /run/llama-server-units.XXXXXX)
trap '/usr/bin/rm -rf "$staging"' EXIT
/usr/bin/mkdir "$staging/llama-server-gpt-oss-120b.service.d"
/usr/bin/cat > "$staging/llama-server-gpt-oss-120b.service" <<'UNIT'
# /etc/systemd/system/llama-server-gpt-oss-120b.service
# vega の汎用 LLM(gpt-oss-120b、OpenAI 互換の API、0.0.0.0:8082)。LLM_UNITS (uuid:0ee79be6-f2bd-400e-8340-b9e60da6c9a7)。
# ExecStart= の引数は、手で起こしていた版(/work2/llm/run-gptoss120b.sh を HOST=0.0.0.0 で起こしたもの)と
# 同じ並びに、CORS を締める 2 つ(--cors-origins と --no-cors-credentials)だけを足したもの。
# それ以外の設定(バイナリ・モデル・-ngl・-ot・スレッド・文脈長・スロット)は変えない(操作者の指示)。

[Unit]
Description=llama-server gpt-oss-120b (OpenAI-compatible API on 0.0.0.0:8082)
# /work2 は別のファイルシステム(/dev/nvme0n1p1)なので、載るまで待つ。
RequiresMountsFor=/work2/llm
# 埋め込みと rerank(uniqnode が使う小さい 2 つ)の exec を先にする。Type=exec なので読み込みの完了は待たず、
# 読み込みは重なりうる(VRAM はどの順でも収まる)。順序だけで、依存(Requires=)は付けない。
After=network.target llama-server-bge-m3.service llama-server-bge-reranker-v2-m3.service
# 起動の失敗が続くときは 30 分に 3 回で諦める(1 回の読み込みで 63 GB を読むため)。
StartLimitIntervalSec=30min
StartLimitBurst=3

[Service]
Type=exec
User=hikalium
Group=hikalium
WorkingDirectory=/work2/llm
# 手で起こしていた版の環境と同じ(バイナリの RUNPATH も $ORIGIN なので、同じ .so を読む)。
Environment=LD_LIBRARY_PATH=/work2/llm/vulkan/llama-b10025
# 手で起こしていた版の上限と同じ(利用者のセッションから継いでいた値)。
LimitNOFILE=1048576
LimitMEMLOCK=67581657088
# 8082 へ届いてよい相手を nft の表 inet llama_server_gpt_oss_120b で限る(隣の .d の firewall.nft)。
# 先頭の + は User= に関わらず root で走らせる印。nftables.service は無効なので、起動のたびにここで入れる。
ExecStartPre=+/usr/sbin/nft -f /etc/systemd/system/llama-server-gpt-oss-120b.service.d/firewall.nft
ExecStart=/work2/llm/vulkan/llama-b10025/llama-server --model /work2/llm/models/gpt-oss-120b/gpt-oss-120b-mxfp4-00001-of-00003.gguf --host 0.0.0.0 --port 8082 -ngl 99 -ot "\\.ffn_(gate|up|down)_exps\\.=CPU" --threads 26 --ctx-size 16384 --parallel 1 -fa on --no-mmap --jinja --chat-template-kwargs '{"reasoning_effort":"low"}' --cors-origins http://localhost:8082 --no-cors-credentials
# llama-server は束縛をモデルの読み込みより先に行うので、口が塞がっていればすぐ exit 1 で止まる
# (手で起こした版が残っているときに二重に読み込まない)。
Restart=on-failure
RestartSec=30s
# SIGTERM で行儀よく終わる(server.cpp の sigaction)。
TimeoutStopSec=2min
# 出力は root の持ち物の /var/log/llama-server/ の下へ追記する(systemd が root で開き、シンボリックリンクを辿るので、
# hikalium の書ける /work2/llm には置かない)。journal には起動と終了の記録だけが残る。
StandardOutput=append:/var/log/llama-server/gpt-oss-120b.log
StandardError=inherit
# NoNewPrivileges= は置かない: 再起動の後に最初の GPU の利用者になったとき、setuid の nvidia-modprobe が
# /dev/nvidia-modeset などを作れず、Vulkan が CPU に黙って落ちうる(手で起こしていた版も NoNewPrivs 0)。

[Install]
WantedBy=multi-user.target
UNIT
/usr/bin/cat > "$staging/llama-server-bge-m3.service" <<'UNIT'
# /etc/systemd/system/llama-server-bge-m3.service
# uniqnode の埋め込み(bge-m3、127.0.0.1:8083)。LLM_UNITS (uuid:0ee79be6-f2bd-400e-8340-b9e60da6c9a7)。
# ExecStart= の引数は、手で起こしていた版(/work2/llm/run-bge-m3.sh の既定)と同じ。設定は変えない。

[Unit]
Description=llama-server bge-m3 (embeddings for uniqnode on 127.0.0.1:8083)
RequiresMountsFor=/work2/llm
After=network.target
StartLimitIntervalSec=10min
StartLimitBurst=5

[Service]
Type=exec
User=hikalium
Group=hikalium
WorkingDirectory=/work2/llm
Environment=LD_LIBRARY_PATH=/work2/llm/vulkan/llama-b10025
LimitNOFILE=1048576
LimitMEMLOCK=67581657088
ExecStart=/work2/llm/vulkan/llama-b10025/llama-server --model /work2/llm/models/bge-m3/bge-m3-FP16.gguf --host 127.0.0.1 --port 8083 --embeddings --pooling cls --embd-normalize 2 -ngl 99 --ctx-size 8192 --parallel 4 --batch-size 2048 --ubatch-size 2048 --threads 8
Restart=on-failure
RestartSec=10s
TimeoutStopSec=1min
StandardOutput=append:/var/log/llama-server/bge-m3.log
StandardError=inherit

[Install]
WantedBy=multi-user.target
UNIT
/usr/bin/cat > "$staging/llama-server-bge-reranker-v2-m3.service" <<'UNIT'
# /etc/systemd/system/llama-server-bge-reranker-v2-m3.service
# uniqnode の rerank(bge-reranker-v2-m3、127.0.0.1:8084)。LLM_UNITS (uuid:0ee79be6-f2bd-400e-8340-b9e60da6c9a7)。
# ExecStart= の引数は、手で起こしていた版(pid 888085 の /proc/<pid>/cmdline)と同じ。設定は変えない。

[Unit]
Description=llama-server bge-reranker-v2-m3 (rerank for uniqnode on 127.0.0.1:8084)
RequiresMountsFor=/work2/llm
After=network.target
StartLimitIntervalSec=10min
StartLimitBurst=5

[Service]
Type=exec
User=hikalium
Group=hikalium
WorkingDirectory=/work2/llm
Environment=LD_LIBRARY_PATH=/work2/llm/vulkan/llama-b10025
LimitNOFILE=1048576
LimitMEMLOCK=67581657088
ExecStart=/work2/llm/vulkan/llama-b10025/llama-server --model /work2/llm/models/bge-reranker-v2-m3/bge-reranker-v2-m3-FP16.gguf --host 127.0.0.1 --port 8084 --reranking --pooling rank -ngl 99 --ctx-size 8192 --batch-size 2048 --ubatch-size 2048
Restart=on-failure
RestartSec=10s
TimeoutStopSec=1min
StandardOutput=append:/var/log/llama-server/bge-reranker-v2-m3.log
StandardError=inherit

[Install]
WantedBy=multi-user.target
UNIT
/usr/bin/cat > "$staging/llama-server-gpt-oss-120b.service.d/firewall.nft" <<'NFT'
# vega の llama-server gpt-oss-120b(0.0.0.0:8082)へ届いてよい相手を限る。LLM_UNITS (uuid:0ee79be6-f2bd-400e-8340-b9e60da6c9a7)。
# llama-server-gpt-oss-120b.service の ExecStartPre= が起動のたびに読む(nft -f)。手で入れるなら同じ命令。
# 表は uniqnode の表(inet uniqnode・inet uniqnode_graph_a・inet uniqnode_graph_b)と別の名で、互いに消し合わない。
table inet llama_server_gpt_oss_120b {}
delete table inet llama_server_gpt_oss_120b
table inet llama_server_gpt_oss_120b {
	set dropped_sources {
		type ipv4_addr
		flags dynamic, timeout
		timeout 30d
	}
	chain input {
		type filter hook input priority filter; policy accept;
		tcp dport 8082 iifname "lo" counter accept
		tcp dport 8082 iifname "wg1" ip saddr { 10.10.128.2, 10.10.128.4 } counter accept
		tcp dport 8082 meta nfproto ipv4 update @dropped_sources { ip saddr } counter drop
		tcp dport 8082 counter drop
	}
}
NFT
echo "書いたものを確かめる(まだ /etc には置いていない)"
/usr/bin/systemd-analyze verify "$staging"/llama-server-*.service
/usr/sbin/nft -c -f "$staging/llama-server-gpt-oss-120b.service.d/firewall.nft"
for u in "${units[@]}"; do
  state=$(systemctl is-active "$u" || true)
  echo "据える前の $u: $state"
done
# unit の StandardOutput=append: の先(切り替えの命令・戻す命令と同じ)。
declare -A log_of=([8084]=/var/log/llama-server/bge-reranker-v2-m3.log [8083]=/var/log/llama-server/bge-m3.log [8082]=/var/log/llama-server/gpt-oss-120b.log)
# ログの置き場とファイルを用意する(据え付け・切り替え・戻すの 3 つの命令で同じもの。何かを止める前に呼ぶ)。
# systemd も戻す命令の >> も root で開き、シンボリックリンクを辿るので、置き場は hikalium の書けない root:root 0755 の
# 本物のディレクトリにする(無ければ作る。root:root で組と他人に書き込みを許さないなら 0755 に直す。それ以外は止まる)。
# 3 つのログは root:root 0644 の普通のファイルにする(無ければ作り、在れば中身を切らずに持ち主と mode を直す。
# シンボリックリンク・ディレクトリ・FIFO など普通のファイルでないものが在れば止まる)。
log_dir=/var/log/llama-server
prepare_logs() {
  local f
  if [ ! -e "$log_dir" ] && [ ! -L "$log_dir" ]; then /usr/bin/install -d -m 0755 -o root -g root "$log_dir"; echo "$log_dir を作った"; fi
  if [ -L "$log_dir" ] || [ ! -d "$log_dir" ] || [ "$(/usr/bin/stat -c %u:%g "$log_dir")" != 0:0 ] || [ $((0$(/usr/bin/stat -c %a "$log_dir") & 022)) -ne 0 ]; then
    echo "$log_dir が root の持ち物の本物のディレクトリでない(または組か他人が書ける):" >&2; /usr/bin/ls -ld "$log_dir" >&2; return 1
  fi
  /usr/bin/chmod 0755 "$log_dir" || return 1
  for f in "${log_of[@]}"; do
    if [ ! -e "$f" ] && [ ! -L "$f" ]; then /usr/bin/install -m 0644 -o root -g root /dev/null "$f" || return 1; echo "$f を作った"; fi
    if [ -L "$f" ] || [ ! -f "$f" ]; then echo "$f が普通のファイルでない(シンボリックリンクなど):" >&2; /usr/bin/ls -ld "$f" >&2; return 1; fi
    /usr/bin/chown root:root "$f" || return 1
    /usr/bin/chmod 0644 "$f" || return 1
  done
  echo "ログの置き場: $(/usr/bin/ls -ld "$log_dir")"
  /usr/bin/ls -l "${log_of[@]}"
}
prepare_logs || { echo "ログの置き場を用意できない。/etc に何も置かずに止まる"; exit 1; }
/usr/bin/install -d -m 0755 -o root -g root "$dir/llama-server-gpt-oss-120b.service.d"
for u in "${units[@]}"; do /usr/bin/install -m 0644 -o root -g root "$staging/$u" "$dir/$u"; done
/usr/bin/install -m 0644 -o root -g root "$staging/llama-server-gpt-oss-120b.service.d/firewall.nft" "$nft_file"
systemctl daemon-reload
echo "unit を置いた(enable も start もしていない。手で起こした 3 つはそのまま)"
for u in "${units[@]}"; do echo "$u: $(systemctl is-enabled "$u" || true) / $(systemctl is-active "$u" || true)"; done
/usr/sbin/nft -f "$nft_file"
listing=$(/usr/sbin/nft list table inet llama_server_gpt_oss_120b)
echo "$listing"
/usr/bin/grep -q 'tcp dport 8082 counter packets [0-9]* bytes [0-9]* drop' <<<"$listing" || { echo "表 inet llama_server_gpt_oss_120b に 8082 の drop の規則が見えない"; exit 1; }
echo "8082 の待ち受け(手で起こした版のまま):"
/usr/bin/ss -Hltnp 'sport = :8082'
echo "据え付けを終えた(8082 は nft で限った。unit は切り替えの命令で起こす)"
EOF
```

済んだかは、/tmp/llama-server-units-install.log の最後が「据え付けを終えた」と `exit status: 0` であること。
その後:

- lamalium の helpdesk が crystal から今までどおり答えるかを、lamalium の統括に確かめてもらう。
- 半日〜1 日おいて、`sudo /usr/sbin/nft list table inet llama_server_gpt_oss_120b` の `dropped_sources` を読む。
  知らない相手が落ちていれば、その相手が要るかを操作者が決める(問いの 1)。要るなら firewall.nft の `{ }` に
  足す命令を別に用意する。
- 困ったときは下の「8082 の制限を外す」をすぐ打てる。

### 2. 切り替え(手で起こした版を止めて、unit で起こす)

8084 → 8083 → 8082 の順に 1 つずつ。

1. 先に、据え付けと同じ prepare_logs で、ログの置き場(root:root 0755)と 3 つのログ(root:root 0644 の普通の
   ファイル)を用意する。それから全部の口を確かめる: 表が入っている(無ければ firewall.nft を入れる)、unit のファイルが在り、systemd が
   読み込んだ `ExecStart=`(`busctl` で取る path と argv の配列)が計画の argv と引数の境目まで同じ、口の持ち主が
   1 つに決まり、それが手で起こした版である、口の持ち主(と動いている unit の MainPID)の他に同じモデルを読む
   プロセスが無い(あれば最長 120 秒待つ)。ここで止まれば何も止めていない。
2. 口ごとに、unit が動いていなければ:
   `/slots` を `jq -s` で読み、JSON の値がちょうど 1 個の空でない配列で、全要素の `is_processing` が false と
   読めるまで待つ(待ち始めからの実の経過時間で最長 15 分)。読めない・形が違う・値が連結されているときは止まる。
   SIGTERM の直前に身元を確かめ直し、SIGTERM を送り、プロセスが消えて口が空くのを最長 120 秒待つ。終わらなければ
   SIGKILL はせず、unit も起こさずに止まる(そのときは操作者が決める)。同じモデルを読むプロセスが残っていないことを
   もう一度確かめ(最長 120 秒待つ)、それから `systemctl enable --now`。
3. 口ごとに、起こしたばかりでも前の回から動いていても毎回: `/health` が 200 になるまで待つ(8082 は最長 900 秒、
   他は 300 秒。unit が落ちれば journal とログの末尾を写して止まる)、口の持ち主が MainPID である、MainPID の argv が
   計画どおり(8082 は CORS の 2 つを足した形)、MainPID の他に同じモデルを読むプロセスが無い、MainPID が nvidia-smi の GPU の利用者に載っている、unit が enabled。
   途中で止まった後に打ち直すと、前の回に起こした unit もここで確かめ直される。
   ここに書いた待ちの上限(15 分・120 秒・900 秒・300 秒)は、待ち始めからの実の経過時間を失敗のたびに見る概算で
   ある。最後の 1 回の要求(curl は 5〜10 秒で打ち切る)と待ちの間隔のぶん、上限を少し超えて成功と判定しうる。
4. 最後に、8082 の OPTIONS に知らない Origin を送って CORS のヘッダが締まったことと、表が入っていることを確かめる。

止まる時間: 8084・8083 は数秒(serve は劣化して答え続ける)。8082 は読み込みの 40 秒〜数分で、その間の
lamalium の要求は失敗する。打つ前に lamalium の統括へ知らせ、helpdesk が静かな時間を選ぶ。`/slots` を見てから
SIGTERM までの間に入った要求は失われうる。vega で打つ命令:

```
( set -o pipefail; sudo bash -s <<'EOF' 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee -a /tmp/llama-server-units-cutover.log; echo "exit status: $?" | /usr/bin/tee -a /tmp/llama-server-units-cutover.log )
set -euo pipefail
llama=/work2/llm/vulkan/llama-b10025/llama-server
nft_file=/etc/systemd/system/llama-server-gpt-oss-120b.service.d/firewall.nft
hand_uid=$(/usr/bin/id -u hikalium)
# 手で起こした版の argv(/proc/<pid>/cmdline、2026-10-01 に読んだもの)と、unit が起こす版の argv。
# 違うのは gpt-oss-120b の末尾の CORS の 2 つだけ。
hand_8084=("$llama" --model /work2/llm/models/bge-reranker-v2-m3/bge-reranker-v2-m3-FP16.gguf --host 127.0.0.1 --port 8084 --reranking --pooling rank -ngl 99 --ctx-size 8192 --batch-size 2048 --ubatch-size 2048)
hand_8083=("$llama" --model /work2/llm/models/bge-m3/bge-m3-FP16.gguf --host 127.0.0.1 --port 8083 --embeddings --pooling cls --embd-normalize 2 -ngl 99 --ctx-size 8192 --parallel 4 --batch-size 2048 --ubatch-size 2048 --threads 8)
hand_8082=("$llama" --model /work2/llm/models/gpt-oss-120b/gpt-oss-120b-mxfp4-00001-of-00003.gguf --host 0.0.0.0 --port 8082 -ngl 99 -ot '\.ffn_(gate|up|down)_exps\.=CPU' --threads 26 --ctx-size 16384 --parallel 1 -fa on --no-mmap --jinja --chat-template-kwargs '{"reasoning_effort":"low"}')
unit_8084=("${hand_8084[@]}")
unit_8083=("${hand_8083[@]}")
unit_8082=("${hand_8082[@]}" --cors-origins http://localhost:8082 --no-cors-credentials)
ports=(8084 8083 8082)
declare -A unit_of=([8084]=llama-server-bge-reranker-v2-m3.service [8083]=llama-server-bge-m3.service [8082]=llama-server-gpt-oss-120b.service)
declare -A load_limit=([8084]=300 [8083]=300 [8082]=900)
# unit の StandardOutput=append: の先(llama-server の出力は journal でなくここに出る)。
declare -A log_of=([8084]=/var/log/llama-server/bge-reranker-v2-m3.log [8083]=/var/log/llama-server/bge-m3.log [8082]=/var/log/llama-server/gpt-oss-120b.log)
# ログの置き場とファイルを用意する(据え付け・切り替え・戻すの 3 つの命令で同じもの。何かを止める前に呼ぶ)。
# systemd も戻す命令の >> も root で開き、シンボリックリンクを辿るので、置き場は hikalium の書けない root:root 0755 の
# 本物のディレクトリにする(無ければ作る。root:root で組と他人に書き込みを許さないなら 0755 に直す。それ以外は止まる)。
# 3 つのログは root:root 0644 の普通のファイルにする(無ければ作り、在れば中身を切らずに持ち主と mode を直す。
# シンボリックリンク・ディレクトリ・FIFO など普通のファイルでないものが在れば止まる)。
log_dir=/var/log/llama-server
prepare_logs() {
  local f
  if [ ! -e "$log_dir" ] && [ ! -L "$log_dir" ]; then /usr/bin/install -d -m 0755 -o root -g root "$log_dir"; echo "$log_dir を作った"; fi
  if [ -L "$log_dir" ] || [ ! -d "$log_dir" ] || [ "$(/usr/bin/stat -c %u:%g "$log_dir")" != 0:0 ] || [ $((0$(/usr/bin/stat -c %a "$log_dir") & 022)) -ne 0 ]; then
    echo "$log_dir が root の持ち物の本物のディレクトリでない(または組か他人が書ける):" >&2; /usr/bin/ls -ld "$log_dir" >&2; return 1
  fi
  /usr/bin/chmod 0755 "$log_dir" || return 1
  for f in "${log_of[@]}"; do
    if [ ! -e "$f" ] && [ ! -L "$f" ]; then /usr/bin/install -m 0644 -o root -g root /dev/null "$f" || return 1; echo "$f を作った"; fi
    if [ -L "$f" ] || [ ! -f "$f" ]; then echo "$f が普通のファイルでない(シンボリックリンクなど):" >&2; /usr/bin/ls -ld "$f" >&2; return 1; fi
    /usr/bin/chown root:root "$f" || return 1
    /usr/bin/chmod 0644 "$f" || return 1
  done
  echo "ログの置き場: $(/usr/bin/ls -ld "$log_dir")"
  /usr/bin/ls -l "${log_of[@]}"
}

# 口を待つプロセスの pid を 1 つ返す。待つものが無ければ空。ss の失敗、pid の読めない待ち受け、
# 2 つ以上の持ち主は、空と区別して失敗(2)にする。
port_owner() {
  local out line found pid pids=""
  out=$(/usr/bin/ss -Hltnp "sport = :$1") || { echo "ss が失敗した(口 $1)" >&2; return 2; }
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    # 1 行に共有の持ち主が並ぶこともある(users:(("…",pid=111,…),("…",pid=222,…)))ので、行の pid を全部取る。
    found=$(/usr/bin/grep -o 'pid=[0-9]*' <<<"$line" | /usr/bin/cut -d= -f2) || { echo "口 $1 に持ち主の読めない待ち受けがある: $line" >&2; return 2; }
    for pid in $found; do
      if [ -n "$pids" ] && [ "$pid" != "$pids" ]; then echo "口 $1 の持ち主が 2 つ以上ある: $out" >&2; return 2; fi
      pids=$pid
    done
  done <<<"$out"
  [ -z "$pids" ] || echo "$pids"
}
argv_is() { local -n want=$2; /usr/bin/cmp -s <(printf '%s\0' "${want[@]}") "/proc/$1/cmdline"; }
show_argv() { /usr/bin/tr '\0' ' ' < "/proc/$1/cmdline"; echo; }
# systemd が読み込んだ unit の ExecStart= が期待の argv か。D-Bus の ExecStart(a(sasbttttuii))を JSON で取り、
# 項目がちょうど 1 つで、失敗を無視する印が無いときに、path と argv を NUL で区切って期待とバイト単位で比べる
# (引数の境目まで比べる)。
unit_execstart_is() {
  local -n want=$2
  local path
  path=$(/usr/bin/busctl --json=short call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager LoadUnit s "$1" | /usr/bin/jq -r '.data[0]') || return 1
  [ -n "$path" ] || return 1
  /usr/bin/cmp -s <(printf '%s\0' "${want[0]}" "${want[@]}") <(/usr/bin/busctl --json=short get-property org.freedesktop.systemd1 "$path" org.freedesktop.systemd1.Service ExecStart | /usr/bin/jq -j 'if (.data | length) == 1 and .data[0][2] == false then (.data[0][0] + "\u0000" + (.data[0][1] | map(. + "\u0000") | add)) else empty end')
}
# 口に関わらず、hikalium の、同じバイナリで同じモデル(argv の --model の次)を読むプロセスの pid を空白で並べる。
# 第 2 引数の pid は除く。口を閉じたがまだ終わらず VRAM を持つ前の版を見つけるため。
# 走査の途中で消えたプロセスは数えない。生きているのに持ち主・バイナリ・引数を読めないプロセスがあれば、
# 「無い」と区別して失敗(2)にする(ゾンビは VRAM を持たないので数えない)。
# バイナリが差し替えられて /proc/<pid>/exe の末尾に " (deleted)" が付いていても、同じバイナリとみなす。
same_model_pids() {
  local -n want=hand_$1
  local model=${want[2]} p pid uid exe st rc out=""
  for p in /proc/[0-9]*; do
    pid=${p#/proc/}
    [ "$pid" != "${2:-}" ] || continue
    if ! uid=$(/usr/bin/stat -c %u "$p" 2> /dev/null); then
      [ -e "$p" ] || continue
      echo "pid $pid の持ち主を読めない" >&2; return 2
    fi
    [ "$uid" = "$hand_uid" ] || continue
    if ! exe=$(/usr/bin/readlink "$p/exe" 2> /dev/null); then
      { st=$(< "$p/stat"); } 2> /dev/null || st=""
      [ -e "$p" ] || continue
      st=${st##*) }
      [ "${st%% *}" != Z ] || continue
      echo "pid $pid(hikalium)のバイナリを読めない" >&2; return 2
    fi
    exe=${exe% (deleted)}
    [ "$exe" = "$llama" ] || continue
    rc=0; /usr/bin/grep -qxFz -- "$model" "$p/cmdline" 2> /dev/null || rc=$?
    if [ "$rc" -eq 1 ]; then continue; fi
    if [ "$rc" -ne 0 ]; then
      [ -e "$p" ] || continue
      echo "pid $pid(hikalium の llama-server)の引数を読めない" >&2; return 2
    fi
    out+="$pid "
  done
  echo "${out% }"
}
# 同じモデルの別のプロセスが終わるのを最長 120 秒待つ。終わらなければ 1、確かめられなければ 2。
wait_no_same_model() {
  local started=$SECONDS left
  while :; do
    left=$(same_model_pids "$1" "${2:-}") || { echo "口 $1 のモデルを読む別のプロセスの有無を確かめられない" >&2; return 2; }
    [ -n "$left" ] || return 0
    if [ $((SECONDS - started)) -ge 120 ]; then echo "口 $1 のモデルを読む別のプロセス(pid $left)が 120 秒で終わらない" >&2; return 1; fi
    echo "口 $1 のモデルを読む別のプロセス(pid $left)が残っている。終わるのを待つ"
    sleep 2
  done
}
# プロセスの身元: uid と起動の刻み(/proc/<pid>/stat の 22 番目。名前の括弧の後ろから数えて 20 番目)。
# 読めなければ失敗を返す(呼び手の代入が set -e で止まる)。
identity() {
  local uid stat rest start
  local -a fields
  uid=$(/usr/bin/stat -c %u "/proc/$1") || return 1
  stat=$(< "/proc/$1/stat") || return 1
  rest=${stat##*) }
  read -r -a fields <<<"$rest"
  start=${fields[19]:-}
  [[ $uid =~ ^[0-9]+$ && $start =~ ^[0-9]+$ ]] || { echo "pid $1 の身元を読めない" >&2; return 1; }
  echo "$uid $start"
}
# 手で起こした版: hikalium の、端末のセッションの scope の下の、同じバイナリで argv が一致するプロセス。
is_hand() {
  [ "$(/usr/bin/stat -c %u "/proc/$1")" = "$hand_uid" ] || return 1
  [ "$(/usr/bin/readlink "/proc/$1/exe")" = "$llama" ] || return 1
  /usr/bin/grep -Eqx "0::/user.slice/user-[0-9]+.slice/session-[0-9]+.scope" "/proc/$1/cgroup" || return 1
  argv_is "$1" "hand_$2"
}
# /slots を JSON として読み、空でない配列で、全要素の is_processing が真偽値で、全部 false のときだけ 0。
# どれかが処理中なら 1。読めない・形が違うときは 2。
slots_idle() {
  local body verdict
  body=$(/usr/bin/curl -sf --max-time 10 "http://127.0.0.1:$1/slots") || return 2
  # -s で本文全体を 1 つの列に取り込み、JSON の値がちょうど 1 個のときだけ判定する(連結された値は「読めない」)。
  verdict=$(/usr/bin/jq -rs 'if length == 1 and (.[0] | type == "array" and length > 0 and all(.[]; type == "object" and (.is_processing | type) == "boolean")) then (if all(.[0][]; .is_processing == false) then "idle" else "busy" end) else "bad" end' <<<"$body" 2>/dev/null) || return 2
  case $verdict in
    idle) return 0 ;;
    busy) return 1 ;;
    *) return 2 ;;
  esac
}

echo "== 事前の確かめ(ここで止まれば何も止めていない)"
prepare_logs || { echo "ログの置き場を用意できない。何も止めずに止まる"; exit 1; }
tables=$(/usr/sbin/nft list tables)
if /usr/bin/grep -qxF 'table inet llama_server_gpt_oss_120b' <<<"$tables"; then
  echo "切り替えの前の表(unit の起動で入れ直すと、数と dropped_sources は空に戻る):"
  /usr/sbin/nft list table inet llama_server_gpt_oss_120b
else
  [ -f "$nft_file" ] || { echo "$nft_file が無い。先に据え付けの命令を打つ"; exit 1; }
  echo "表 inet llama_server_gpt_oss_120b が無い(据え付けの後に再起動したとき)。$nft_file を入れる"
  /usr/sbin/nft -f "$nft_file"
fi
declare -A hand_pid=() hand_identity=()
for port in "${ports[@]}"; do
  u=${unit_of[$port]}
  systemctl cat "$u" > /dev/null || { echo "$u が無い。先に据え付けの命令を打つ"; exit 1; }
  if ! unit_execstart_is "$u" "unit_$port"; then echo "$u の ExecStart= が計画の argv と違う(据え付けの命令を打ち直す)。何も止めずに止まる:"; systemctl show -p ExecStart --value "$u"; exit 1; fi
  pid=$(port_owner "$port")
  if systemctl is-active --quiet "$u"; then
    main=$(systemctl show -p MainPID --value "$u")
    wait_no_same_model "$port" "$main" || { echo "何も止めずに止まる"; exit 1; }
    echo "$port: $u は既に動いている(起こし直さず、下で確かめ直す)"
    continue
  fi
  # 口の持ち主(手の版、または無し)の他に、同じモデルを読むプロセスが残っていれば、何かを止める前に待つか止まる。
  wait_no_same_model "$port" "$pid" || { echo "何も止めずに止まる"; exit 1; }
  if [ -z "$pid" ]; then echo "$port: 待つプロセスは無い。$u をそのまま起こす"; continue; fi
  if ! is_hand "$pid" "$port"; then echo "$port を持つ pid $pid は手で起こした版ではない(利用者・scope・引数のどれかが違う)。何も止めずに止まる:"; /usr/bin/cat "/proc/$pid/cgroup"; show_argv "$pid"; exit 1; fi
  hand_pid[$port]=$pid
  hand_identity[$port]=$(identity "$pid")
  echo "$port: 手で起こした版 pid $pid(身元 ${hand_identity[$port]})"
done

for port in "${ports[@]}"; do
  u=${unit_of[$port]}
  echo "== $port($u)"
  if ! systemctl is-active --quiet "$u"; then
    pid=${hand_pid[$port]:-}
    if [ -n "$pid" ]; then
      started=$SECONDS
      while :; do
        rc=0; slots_idle "$port" || rc=$?
        if [ "$rc" -eq 0 ]; then break; fi
        if [ "$rc" -eq 2 ]; then echo "$port の /slots を読めない。止めずに止まる"; exit 1; fi
        if [ $((SECONDS - started)) -ge 900 ]; then echo "$port は 15 分たっても処理中。止めずに止まる"; exit 1; fi
        echo "$port は要求を処理中。空くのを待つ"
        sleep 10
      done
      # 止める直前に、同じプロセスがまだ口を持っていることを確かめ直す。身元(uid と起動の刻み)は kill の
      # 直前に読み、kill との間に他の命令を挟まない。
      now=$(port_owner "$port")
      if [ "$now" != "$pid" ] || ! is_hand "$pid" "$port"; then
        echo "$port の持ち主が事前の確かめの後に変わった(今: '${now}')。止めずに止まる"; exit 1
      fi
      echo "pid $pid に SIGTERM を送る"
      last=$(identity "$pid")
      if [ "$last" != "${hand_identity[$port]}" ]; then echo "pid $pid の身元が変わった('$last')。止めずに止まる"; exit 1; fi
      kill -TERM "$pid"
      started=$SECONDS
      while :; do
        now=$(port_owner "$port")
        if [ -e "/proc/$pid" ]; then
          after=$(identity "$pid" 2> /dev/null || true)
          if [ -n "$after" ] && [ "$after" != "${hand_identity[$port]}" ]; then echo "pid $pid が別のプロセス('$after')になっている。SIGTERM の宛先を確かめるため止まる"; exit 1; fi
        fi
        if [ ! -e "/proc/$pid" ] && [ -z "$now" ]; then break; fi
        if [ $((SECONDS - started)) -ge 120 ]; then echo "pid $pid が 120 秒で終わらない。SIGKILL はせず、unit も起こさずに止まる"; exit 1; fi
        sleep 2
      done
      echo "pid $pid は終わった($((SECONDS - started)) 秒)"
    fi
    # 口が空いていても、口を閉じたがまだ終わらない前の版(VRAM を持つ)が残っていれば、起こさない。
    wait_no_same_model "$port" "" || { echo "$u は起こさずに止まる"; exit 1; }
    systemctl enable --now "$u"
  fi
  # 起こしたばかりでも、前の回から動いていても、同じ確かめを毎回行う。
  started=$SECONDS
  until /usr/bin/curl -sf --max-time 5 -o /dev/null "http://127.0.0.1:$port/health"; do
    state=$(systemctl is-active "$u" || true)
    if [ "$state" = failed ] || [ "$state" = inactive ]; then echo "$u が $state になった"; journalctl -u "$u" -n 40 --no-pager; echo "${log_of[$port]} の末尾:"; /usr/bin/tail -n 40 "${log_of[$port]}"; exit 1; fi
    if [ $((SECONDS - started)) -ge "${load_limit[$port]}" ]; then echo "$u が ${load_limit[$port]} 秒で応答しない(状態: $state)"; journalctl -u "$u" -n 40 --no-pager; echo "${log_of[$port]} の末尾:"; /usr/bin/tail -n 40 "${log_of[$port]}"; exit 1; fi
    sleep 5
  done
  main=$(systemctl show -p MainPID --value "$u")
  owner=$(port_owner "$port")
  if [ "$owner" != "$main" ]; then echo "確認: 失敗。口 $port の持ち主($owner)が $u の MainPID($main)でない"; exit 1; fi
  if ! argv_is "$main" "unit_$port"; then echo "確認: 失敗。$u の引数が計画と違う:"; show_argv "$main"; exit 1; fi
  wait_no_same_model "$port" "$main" || { echo "確認: 失敗。$u の MainPID($main)の他に、同じモデルを読むプロセスが残っている"; exit 1; }
  echo "確認: $u は口 $port を持ち、引数は計画どおり(MainPID $main)"
  if ! /usr/bin/nvidia-smi --query-compute-apps=pid --format=csv,noheader | /usr/bin/grep -qx "$main"; then echo "確認: 失敗。MainPID $main が GPU の利用者の一覧に無い(Vulkan が GPU を掴んでいない)"; /usr/bin/nvidia-smi; exit 1; fi
  echo "確認: MainPID $main は GPU を使っている"
  enabled=$(systemctl is-enabled "$u" || true)
  if [ "$enabled" != enabled ]; then echo "確認: 失敗。$u が enable されていない($enabled)"; exit 1; fi
done

echo "== 仕上げの確かめ"
headers=$(/usr/bin/curl -s --max-time 10 -D - -o /dev/null -X OPTIONS -H 'Origin: http://example.invalid' -H 'Access-Control-Request-Method: POST' http://127.0.0.1:8082/v1/chat/completions | /usr/bin/tr -d '\r')
echo "$headers"
/usr/bin/grep -qix 'Access-Control-Allow-Origin: http://localhost:8082' <<<"$headers" || { echo "確認: 失敗。8082 の CORS の許す生成元が http://localhost:8082 でない"; exit 1; }
/usr/bin/grep -qix 'Access-Control-Allow-Credentials: false' <<<"$headers" || { echo "確認: 失敗。8082 の CORS が資格情報を許している"; exit 1; }
echo "確認: 8082 の CORS は http://localhost:8082 だけを許し、資格情報を許さない"
listing=$(/usr/sbin/nft list table inet llama_server_gpt_oss_120b)
echo "$listing"
/usr/bin/grep -q 'tcp dport 8082 counter packets [0-9]* bytes [0-9]* drop' <<<"$listing" || { echo "確認: 失敗。表に 8082 の drop の規則が見えない"; exit 1; }
echo "確認: 8082 の nft の表が入っている"
echo "切り替えを終えた"
EOF
```

済んだかは、/tmp/llama-server-units-cutover.log の最後が「切り替えを終えた」と `exit status: 0` であること。
「確認:」の行は、口ごとに 2 本(持ち主と引数、GPU)と、CORS と表の 2 本で、計 8 本並ぶ。途中で止まったときは、
記録の最後の数行が理由を言う。手の版を止めた後に unit が起きなかった口は、下の「unit から手の形に戻す」を
打てば戻る。

### 3. 再起動の後の確かめ(操作者が時を選ぶ)

裁定の (2) が効いたことは、機械を再起動して初めて確かめられる。再起動の後、下の「完了の確かめ」の
切り替えの後の項を読む。特に、起動の直後に GPU(ドライバ)が間に合わず Vulkan が GPU を見つけられないと、
llama-server は CPU だけで走りうる。nvidia-smi の一覧に 3 つの MainPID が載っていることで確かめる。切り替えの
命令を打ち直しても同じ確かめになる(動いている unit は止めない)。

## 戻す

### unit から手の形に戻す

3 つの unit を disable して止め、今と同じ命令行で hikalium として手の形で起こし直す。CORS も今の既定に戻る。
unit が動いていれば、止める前に `/slots` で全部のスロットが空いたと読めるまで待つ(最長 15 分)。読めないときも
空いたとはみなさずに読み直し、期限を過ぎたらその unit を止めずに止まる(前の口で戻したものはそのまま)。応答しない unit を止めてでも戻すときは、1 行目の
`sudo bash -s` を `sudo bash -s -- --force` に替えて打つ(待たずに止め、処理中の要求は失われる。記録に `--force:` の
行が残る)。口が空いても、同じモデルを読む前のプロセスが残っていれば、終わるのを最長 120 秒待ってから起こす。
sudo の PAM や打つ人の環境に左右されないよう、上限は `prlimit` で今の手の版と同じ値(開けるファイル・ロックできる
メモリ・コアファイル 0:unlimited・スタック 8388608:unlimited)を明示し、`setpriv` で hikalium に
なり、環境は `env -i` で必要なものだけにし、`setsid` で端末から切り離す。出力は unit と同じ /var/log/llama-server/ の下へ追記する(今の手の版の
/work2/llm と /tmp ではない。`>>` は root のシェルが開くので、unit と同じく hikalium の書けない置き場にし、使われ方の
記録も切り替えの後と同じ 1 か所に続ける。llama-server の設定ではないので、立っている指示には当たらない)。
何かを止める前に、据え付けと同じ prepare_logs で置き場と 3 つのログを用意するので、`>>` がファイルを作ることはない。
起こした後に、引数・uid・上限・GPU を確かめる。unit のファイルと nft の表は残す(enable していないので
起きない。表は裁定の (1) なので別に外す)。口を既に手の形の版が持っていれば、その口は飛ばす。vega で打つ命令:

```
( set -o pipefail; sudo bash -s <<'EOF' 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee -a /tmp/llama-server-units-rollback.log; echo "exit status: $?" | /usr/bin/tee -a /tmp/llama-server-units-rollback.log )
set -euo pipefail
# 既定は、処理中の要求を待ち、/slots が読めなければ止まる。`sudo bash -s -- --force` で打ったときだけ、待たずに止める。
force=no
if [ "${1:-}" = --force ]; then force=yes; echo "--force で打たれた: 動いている unit は処理中の要求を待たずに止める(処理中の要求は失われる)"; fi
llama=/work2/llm/vulkan/llama-b10025/llama-server
hand_uid=$(/usr/bin/id -u hikalium)
hand_8084=("$llama" --model /work2/llm/models/bge-reranker-v2-m3/bge-reranker-v2-m3-FP16.gguf --host 127.0.0.1 --port 8084 --reranking --pooling rank -ngl 99 --ctx-size 8192 --batch-size 2048 --ubatch-size 2048)
hand_8083=("$llama" --model /work2/llm/models/bge-m3/bge-m3-FP16.gguf --host 127.0.0.1 --port 8083 --embeddings --pooling cls --embd-normalize 2 -ngl 99 --ctx-size 8192 --parallel 4 --batch-size 2048 --ubatch-size 2048 --threads 8)
hand_8082=("$llama" --model /work2/llm/models/gpt-oss-120b/gpt-oss-120b-mxfp4-00001-of-00003.gguf --host 0.0.0.0 --port 8082 -ngl 99 -ot '\.ffn_(gate|up|down)_exps\.=CPU' --threads 26 --ctx-size 16384 --parallel 1 -fa on --no-mmap --jinja --chat-template-kwargs '{"reasoning_effort":"low"}')
ports=(8084 8083 8082)
declare -A unit_of=([8084]=llama-server-bge-reranker-v2-m3.service [8083]=llama-server-bge-m3.service [8082]=llama-server-gpt-oss-120b.service)
declare -A log_of=([8084]=/var/log/llama-server/bge-reranker-v2-m3.log [8083]=/var/log/llama-server/bge-m3.log [8082]=/var/log/llama-server/gpt-oss-120b.log)
declare -A load_limit=([8084]=300 [8083]=300 [8082]=900)
# 口に関わらず、hikalium の、同じバイナリで同じモデルを読むプロセスの pid を並べる(第 2 引数の pid は除く)。
# 切り替えの命令と同じ: 途中で消えたものとゾンビは数えず、生きているのに読めないものがあれば失敗(2)。
# exe の末尾の " (deleted)" は落として比べる。
same_model_pids() {
  local -n want=hand_$1
  local model=${want[2]} p pid uid exe st rc out=""
  for p in /proc/[0-9]*; do
    pid=${p#/proc/}
    [ "$pid" != "${2:-}" ] || continue
    if ! uid=$(/usr/bin/stat -c %u "$p" 2> /dev/null); then
      [ -e "$p" ] || continue
      echo "pid $pid の持ち主を読めない" >&2; return 2
    fi
    [ "$uid" = "$hand_uid" ] || continue
    if ! exe=$(/usr/bin/readlink "$p/exe" 2> /dev/null); then
      { st=$(< "$p/stat"); } 2> /dev/null || st=""
      [ -e "$p" ] || continue
      st=${st##*) }
      [ "${st%% *}" != Z ] || continue
      echo "pid $pid(hikalium)のバイナリを読めない" >&2; return 2
    fi
    exe=${exe% (deleted)}
    [ "$exe" = "$llama" ] || continue
    rc=0; /usr/bin/grep -qxFz -- "$model" "$p/cmdline" 2> /dev/null || rc=$?
    if [ "$rc" -eq 1 ]; then continue; fi
    if [ "$rc" -ne 0 ]; then
      [ -e "$p" ] || continue
      echo "pid $pid(hikalium の llama-server)の引数を読めない" >&2; return 2
    fi
    out+="$pid "
  done
  echo "${out% }"
}
# 同じモデルの別のプロセスが終わるのを最長 120 秒待つ。終わらなければ 1、確かめられなければ 2。
wait_no_same_model() {
  local started=$SECONDS left
  while :; do
    left=$(same_model_pids "$1" "${2:-}") || { echo "口 $1 のモデルを読む別のプロセスの有無を確かめられない" >&2; return 2; }
    [ -n "$left" ] || return 0
    if [ $((SECONDS - started)) -ge 120 ]; then echo "口 $1 のモデルを読む別のプロセス(pid $left)が 120 秒で終わらない" >&2; return 1; fi
    echo "口 $1 のモデルを読む別のプロセス(pid $left)が残っている。終わるのを待つ"
    sleep 2
  done
}
port_owner() {
  local out line found pid pids=""
  out=$(/usr/bin/ss -Hltnp "sport = :$1") || { echo "ss が失敗した(口 $1)" >&2; return 2; }
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    # 1 行に共有の持ち主が並ぶこともある(users:(("…",pid=111,…),("…",pid=222,…)))ので、行の pid を全部取る。
    found=$(/usr/bin/grep -o 'pid=[0-9]*' <<<"$line" | /usr/bin/cut -d= -f2) || { echo "口 $1 に持ち主の読めない待ち受けがある: $line" >&2; return 2; }
    for pid in $found; do
      if [ -n "$pids" ] && [ "$pid" != "$pids" ]; then echo "口 $1 の持ち主が 2 つ以上ある: $out" >&2; return 2; fi
      pids=$pid
    done
  done <<<"$out"
  [ -z "$pids" ] || echo "$pids"
}
argv_is() { local -n want=$2; /usr/bin/cmp -s <(printf '%s\0' "${want[@]}") "/proc/$1/cmdline"; }
# /slots が読めて全部のスロットが空いていれば 0、処理中なら 1、読めない・形が違うなら 2(切り替えの命令と同じ判定)。
slots_idle() {
  local body verdict
  body=$(/usr/bin/curl -sf --max-time 10 "http://127.0.0.1:$1/slots") || return 2
  verdict=$(/usr/bin/jq -rs 'if length == 1 and (.[0] | type == "array" and length > 0 and all(.[]; type == "object" and (.is_processing | type) == "boolean")) then (if all(.[0][]; .is_processing == false) then "idle" else "busy" end) else "bad" end' <<<"$body" 2>/dev/null) || return 2
  case $verdict in
    idle) return 0 ;;
    busy) return 1 ;;
    *) return 2 ;;
  esac
}
# 手の形で起こす: 上限は今の手の版と同じ値を prlimit で明示し、setpriv で hikalium になり、環境は必要なものだけにする
# (sudo の PAM や打つ人の環境に左右されないように)。setsid で端末から切り離す。出力の >> は root のこのシェルが
# 開くので、unit と同じ root の持ち物の /var/log/llama-server の下に書く(使われ方の記録も 1 か所に続く)。
start_hand() {
  local -n want=hand_$1
  /usr/bin/setsid -f /usr/bin/prlimit --nofile=1048576:1048576 --memlock=67581657088:67581657088 --core=0:unlimited --stack=8388608:unlimited -- /usr/bin/setpriv --reuid=hikalium --regid=hikalium --init-groups -- /usr/bin/env -i HOME=/home/hikalium USER=hikalium LOGNAME=hikalium LANG=en_US.UTF-8 PATH=/usr/local/bin:/usr/bin:/bin LD_LIBRARY_PATH=/work2/llm/vulkan/llama-b10025 "${want[@]}" >> "${log_of[$1]}" 2>&1 < /dev/null
}

# ログの置き場とファイルを用意する(据え付け・切り替え・戻すの 3 つの命令で同じもの。何かを止める前に呼ぶ)。
# systemd も戻す命令の >> も root で開き、シンボリックリンクを辿るので、置き場は hikalium の書けない root:root 0755 の
# 本物のディレクトリにする(無ければ作る。root:root で組と他人に書き込みを許さないなら 0755 に直す。それ以外は止まる)。
# 3 つのログは root:root 0644 の普通のファイルにする(無ければ作り、在れば中身を切らずに持ち主と mode を直す。
# シンボリックリンク・ディレクトリ・FIFO など普通のファイルでないものが在れば止まる)。
log_dir=/var/log/llama-server
prepare_logs() {
  local f
  if [ ! -e "$log_dir" ] && [ ! -L "$log_dir" ]; then /usr/bin/install -d -m 0755 -o root -g root "$log_dir"; echo "$log_dir を作った"; fi
  if [ -L "$log_dir" ] || [ ! -d "$log_dir" ] || [ "$(/usr/bin/stat -c %u:%g "$log_dir")" != 0:0 ] || [ $((0$(/usr/bin/stat -c %a "$log_dir") & 022)) -ne 0 ]; then
    echo "$log_dir が root の持ち物の本物のディレクトリでない(または組か他人が書ける):" >&2; /usr/bin/ls -ld "$log_dir" >&2; return 1
  fi
  /usr/bin/chmod 0755 "$log_dir" || return 1
  for f in "${log_of[@]}"; do
    if [ ! -e "$f" ] && [ ! -L "$f" ]; then /usr/bin/install -m 0644 -o root -g root /dev/null "$f" || return 1; echo "$f を作った"; fi
    if [ -L "$f" ] || [ ! -f "$f" ]; then echo "$f が普通のファイルでない(シンボリックリンクなど):" >&2; /usr/bin/ls -ld "$f" >&2; return 1; fi
    /usr/bin/chown root:root "$f" || return 1
    /usr/bin/chmod 0644 "$f" || return 1
  done
  echo "ログの置き場: $(/usr/bin/ls -ld "$log_dir")"
  /usr/bin/ls -l "${log_of[@]}"
}
prepare_logs || { echo "ログの置き場を用意できない。何も止めずに止まる"; exit 1; }
cd /work2/llm
for port in "${ports[@]}"; do
  u=${unit_of[$port]}
  echo "== $port($u)"
  # unit が動いていれば、/slots で全部のスロットが空いたと読めるまで待ってから止める(最長 15 分)。処理中も、
  # /slots が読めない(時間切れ・HTTP の誤り・形の違う JSON)のも、空いたとはみなさず、期限まで読み直す。
  # 期限を過ぎたらその unit を止めずに止まる。応答しない unit を止めてでも戻すときだけ、--force で待たずに止める。
  if systemctl is-active --quiet "$u"; then
    if [ "$force" = yes ]; then
      echo "--force: $port の処理中の要求を待たずに止める(処理中の要求は失われる)"
    else
      started=$SECONDS
      while :; do
        rc=0; slots_idle "$port" || rc=$?
        if [ "$rc" -eq 0 ]; then break; fi
        if [ $((SECONDS - started)) -ge 900 ]; then
          if [ "$rc" -eq 1 ]; then echo "$port は 15 分たっても処理中。$u は止めずに止まる(静かな時に打ち直す)"; else echo "$port の /slots を 15 分読めない。$u は止めずに止まる(応答しない unit を止めてでも戻すなら --force で打ち直す)"; fi
          exit 1
        fi
        if [ "$rc" -eq 1 ]; then echo "$port は要求を処理中。空くのを待つ"; else echo "$port の /slots を読めない。読み直す"; fi
        sleep 10
      done
    fi
  fi
  if systemctl cat "$u" > /dev/null 2>&1; then systemctl disable --now "$u"; fi
  started=$SECONDS
  already=no
  while :; do
    pid=$(port_owner "$port")
    if [ -z "$pid" ]; then break; fi
    if [ "$(/usr/bin/stat -c %u "/proc/$pid")" = "$hand_uid" ] && [ "$(/usr/bin/readlink "/proc/$pid/exe")" = "$llama" ] && argv_is "$pid" "hand_$port" && ! /usr/bin/grep -q '/system.slice/' "/proc/$pid/cgroup"; then
      echo "$port は既に手の形の版(pid $pid)が持っている。起こさずに、下で確かめ直す"; already=yes; break
    fi
    if [ $((SECONDS - started)) -ge 120 ]; then echo "$port が 120 秒で空かない(pid $pid)。止まる"; exit 1; fi
    sleep 2
  done
  if [ "$already" = no ]; then
    # 口が空いていても、口を閉じたがまだ終わらない前の版(VRAM を持つ)が残っていれば、起こさない。
    wait_no_same_model "$port" "" || { echo "$port は手の形で起こさずに止まる"; exit 1; }
    start_hand "$port"
  fi
  # 起こしたばかりでも、前の回に起こしたものでも、同じ確かめを毎回行う。
  started=$SECONDS
  until /usr/bin/curl -sf --max-time 5 -o /dev/null "http://127.0.0.1:$port/health"; do
    if [ $((SECONDS - started)) -ge "${load_limit[$port]}" ]; then echo "$port が ${load_limit[$port]} 秒で応答しない。${log_of[$port]} の末尾:"; /usr/bin/tail -n 40 "${log_of[$port]}"; exit 1; fi
    sleep 5
  done
  pid=$(port_owner "$port")
  [ -n "$pid" ] || { echo "確認: 失敗。$port を持つプロセスが見えない"; exit 1; }
  argv_is "$pid" "hand_$port" || { echo "確認: 失敗。$port の pid $pid の引数が手で起こした版と違う"; exit 1; }
  wait_no_same_model "$port" "$pid" || { echo "確認: 失敗。$port の pid $pid の他に、同じモデルを読むプロセスが残っている"; exit 1; }
  [ "$(/usr/bin/stat -c %u "/proc/$pid")" = "$hand_uid" ] || { echo "確認: 失敗。$port の pid $pid が hikalium でない"; exit 1; }
  [ "$(/usr/bin/readlink "/proc/$pid/exe")" = "$llama" ] || { echo "確認: 失敗。$port の pid $pid のバイナリが $llama でない"; exit 1; }
  /usr/bin/grep -Eq '^Max open files +1048576 +1048576 ' "/proc/$pid/limits" || { echo "確認: 失敗。$port の pid $pid の開けるファイルの上限が違う"; /usr/bin/cat "/proc/$pid/limits"; exit 1; }
  /usr/bin/grep -Eq '^Max locked memory +67581657088 +67581657088 ' "/proc/$pid/limits" || { echo "確認: 失敗。$port の pid $pid のロックできるメモリの上限が違う"; /usr/bin/cat "/proc/$pid/limits"; exit 1; }
  /usr/bin/grep -Eq '^Max core file size +0 +unlimited ' "/proc/$pid/limits" || { echo "確認: 失敗。$port の pid $pid のコアファイルの上限が違う"; /usr/bin/cat "/proc/$pid/limits"; exit 1; }
  /usr/bin/grep -Eq '^Max stack size +8388608 +unlimited ' "/proc/$pid/limits" || { echo "確認: 失敗。$port の pid $pid のスタックの上限が違う"; /usr/bin/cat "/proc/$pid/limits"; exit 1; }
  /usr/bin/nvidia-smi --query-compute-apps=pid --format=csv,noheader | /usr/bin/grep -qx "$pid" || { echo "確認: 失敗。$port の pid $pid が GPU の利用者の一覧に無い"; exit 1; }
  echo "確認: $port は手の形で戻った(pid $pid、hikalium、引数・上限・GPU は手で起こしていた版と同じ)"
done
echo "unit から手で起こした形に戻し終えた(unit のファイルと nft の表は残る。表を外すのは別の命令)"
EOF
```

済んだかは、/tmp/llama-server-units-rollback.log の最後が「unit から手で起こした形に戻し終えた」と
`exit status: 0` であること。手の形は再起動で戻らない(今と同じ)。戻した版の cgroup は、打った人のセッションの
scope になる。

### 8082 の制限を外す

表 `inet llama_server_gpt_oss_120b` を消す。外す前の表(落とした相手の一覧を含む)を記録に写す。gpt-oss-120b の
unit のファイルが残っていれば、その unit が次に起きるときに表は入り直す(その旨と今の状態を記録に言う)。
vega で打つ命令:

```
( set -o pipefail; sudo bash -s <<'EOF' 2>&1 | /usr/bin/ts '%Y-%m-%dT%H:%M:%S%z' | /usr/bin/tee -a /tmp/llama-server-firewall-rollback.log; echo "exit status: $?" | /usr/bin/tee -a /tmp/llama-server-firewall-rollback.log )
set -euo pipefail
tables=$(/usr/sbin/nft list tables)
if /usr/bin/grep -qxF 'table inet llama_server_gpt_oss_120b' <<<"$tables"; then
  echo "外す前の表(dropped_sources は落とした相手の一覧):"
  /usr/sbin/nft list table inet llama_server_gpt_oss_120b
  /usr/sbin/nft delete table inet llama_server_gpt_oss_120b
  echo "表 inet llama_server_gpt_oss_120b を外した"
else
  echo "表 inet llama_server_gpt_oss_120b は既に無い"
fi
if [ -e /etc/systemd/system/llama-server-gpt-oss-120b.service ]; then
  echo "注意: llama-server-gpt-oss-120b.service は残っている。この unit が次に起きると(手の start・restart、落ちた後の起こし直し、enable なら機械の起動)ExecStartPre= が表を入れ直す。今の状態: $(systemctl is-enabled llama-server-gpt-oss-120b.service || true) / $(systemctl is-active llama-server-gpt-oss-120b.service || true)"
fi
echo "8082 の nft の制限を外し終えた"
EOF
```

済んだかは、/tmp/llama-server-firewall-rollback.log の最後が「8082 の nft の制限を外し終えた」と
`exit status: 0` であること。

unit のファイルまで消すのは、上の 2 つを打った後に `sudo systemctl disable llama-server-bge-reranker-v2-m3.service
llama-server-bge-m3.service llama-server-gpt-oss-120b.service`、/etc/systemd/system/llama-server-*.service と
/etc/systemd/system/llama-server-gpt-oss-120b.service.d を消して `sudo systemctl daemon-reload`。要るときに
命令の形にして渡す。

## 完了の確かめ

据え付けの後:

- `sudo /usr/sbin/nft list table inet llama_server_gpt_oss_120b` に、lo と wg1 の 2 つの accept と 2 つの drop が
  ある。
- crystal から `curl -s -o /dev/null -w '%{http_code}\n' http://10.10.128.1:8082/health` が 200。LAN の他の機械
  (br0 側、例えば 10.10.10.x)からは接続が時間切れになる。
- 手で起こした 3 つの pid が変わっていない(`ss -ltnp` の 8082・8083・8084 が 3995999・2166473・888085)。

切り替えの後(と再起動の後):

- `systemctl is-enabled` と `is-active` が 3 つとも enabled / active。
- `tr '\0' ' ' < /proc/$(systemctl show -p MainPID --value llama-server-gpt-oss-120b.service)/cmdline` が、
  analysis の「命令行」の 8082 の行の末尾に `--cors-origins http://localhost:8082 --no-cors-credentials` を足したもの。
  8083・8084 は analysis の行と同じ。
- `nvidia-smi --query-compute-apps=pid,used_memory --format=csv` に 3 つの MainPID が載り、VRAM がほぼ今と同じ
  (8082 が約 2.8 GiB、他が約 650 MiB)。
- /var/log/llama-server/gpt-oss-120b.log の起動の出力に「CORS is set to allow all origins」の警告が無い
  (llama-server の出力は `StandardOutput=append:` でこのログに出て、journal には出ない)。
- `ls -ld /var/log/llama-server` が root root の drwxr-xr-x で、中の 3 つのログが root の持ち物である。
- `curl -s -D - -o /dev/null -X OPTIONS -H 'Origin: http://example.invalid' http://127.0.0.1:8082/v1/chat/completions`
  が `Access-Control-Allow-Origin: http://localhost:8082` と `Access-Control-Allow-Credentials: false` を返す。
- uniqnode の検索が埋め込みと rerank つきで答える(/v1/search の応答に劣化の理由が無い)。
- lamalium の helpdesk が crystal から答える。

## 操作者への問い

2026-10-01 の統括の決定(正典や前の裁定で決まるもの、どの答えでも作業が同じものは操作者へ聞かない)により、
操作者へ聞くのは 1 だけになった。2〜5 と 7 は統括の決定、6 は crystal 移行のセッションが今の設定を読んで答える。
下の各項の本文は、決めた背景として残す。

統括の決定(2026-10-01):
- 2: A。orion(10.10.128.4)は、crystal 移行のセッションから「外してよい」と来た時に外す。理由: 7441 と同じ扱いの
  前例があり、許す範囲を狭める向きで、据え付けの命令も変わらない。
- 3: A。CORS の生成元は `http://localhost:8082` と `--no-cors-credentials`(計画のまま)。理由: 2026-09-30T23:41Z の
  裁定が CORS を締めると言っており、ブラウザで使う証拠が無い。操作者が 1 に LAN(ブラウザから使う)と答えたら、
  その時に B へ移り、合う生成元を書く(別の問いにはしない)。API キーは足さない(裁定の外)。
- 4: A。8083・8084 の CORS は変えない。理由: 127.0.0.1 にだけ束縛し、立っている指示が llama-server の設定を変え
  ないと言い、A は何も変えない。残る危険: vega の上のブラウザで悪意ある頁を開くと、その頁から 127.0.0.1:8083・
  8084 の応答を読める(埋め込みと rerank の結果だけで、書き込みや停止の口は無い)。
- 5: A。時は聞かない。切り替えと再起動の試しを別々の依頼として出し、各依頼に止まる時間と止まるものを書く
  (切り替えは 8082 が 40 秒〜数分、8083・8084 は数秒。再起動は vega の全部で、uniqnode の 7440・7441・7450 と
  graph の口を含む)。操作者は打つ時で時を選ぶ。
- 6: 事実を読む問いなので、crystal 移行のセッションが今の設定を読んで答えた(2026-10-01)。crystal の呼び手は
  上の「最終目標とのつながり」の 5 つで全部、送信元は 10.10.128.2 だけで、nft の許可は今のままで足りる。
- 7: A。pid の使い回しの隙を受け入れる。理由: 操作者が見ている 1 回きりの切り替えで、区間は身元を確かめた直後の
  数ミリ秒であり、最悪でも行儀よく終わるプロセスへの SIGTERM である。残る危険: その区間で pid が使い回されれば、
  無関係なプロセス(uniqnode の serve を含みうる)が SIGTERM で止まる。

1. 8082 へ届いてよい相手。この計画は loopback と wg1 の 10.10.128.2(crystal)・10.10.128.4(orion)だけを通す。
   他に要る相手はあるか。候補として見えたが、使っている証拠が無いもの: wg1 の他の機械(eevee 10.10.128.5・
   spark 10.10.128.6 など。lamalium の設定では LLM を「配る」側で、8082 の呼び手ではない)、br0 の LAN
   (10.10.10.0/24。操作者のブラウザから Web UI を開くなら要る)、vega の docker のコンテナ(vega-llm-playground-1
   など。172.x の橋から来る)。据え付けの後の `dropped_sources` が答えの材料になる。
2. orion の移行が済んだら 10.10.128.4 を外してよいか(uniqnode の読み口と同じく、lamalium の crystal 移行の
   セッションからの依頼を待つ)。
3. CORS の許す生成元を `http://localhost:8082` にしてよいか。ブラウザで別の生成元(別のホスト名の Web UI や、
   他のサイトからの呼び出し)を使っているなら、その 1 つを書く。API キーを足すか(裁定の外。lamalium の
   呼び手の変更が要る)。
4. 8083・8084 も CORS は既定の全部許しだが、127.0.0.1 にだけ束縛しているので、この計画では変えない(立っている
   指示を優先)。締めるなら別の裁定にする。
5. 切り替えの時(8082 が 40 秒〜数分止まる)と、再起動の試しをいつ行うか。
6. 今の crystal の lamalium で、8082 を使うのは helpdesk と補助の CLI(persona-review・qa-translate・
   prompt-probe)と viewer の keeper の外部レビュー(container/src/keeper.rs:270)だけか(手元の写しは
   2026-08-15 のもの)。他の席や subtask が使うなら、切り替えの止まる時間の
   知らせ先と、許す相手が増えるかを確かめる。
7. pid の使い回しの隙(上の「二重起動の防止」。通常は短いが上限の保証はない競合区間で、別プロセスへ SIGTERM を
   送る可能性が残る)を受け入れてよいか。閉じるなら、pidfd を取ってから身元を確かめ、同じ pidfd へ送る小さな
   Rust の補助プログラムを先に作る(別の作業になる)。

## 済んだら

据え付けと切り替えが済み、再起動の後の確かめも済んだら、「設計」と「戻す」「完了の確かめ」の今も正しい部分を
docs/mop/LLM_UNITS.md(unit の読み方・確かめ方・戻し方)へ移し、この plan は消す(must/0013・must/0010)。
unit と firewall.nft のコメントは `LLM_UNITS (uuid:0ee79be6-f2bd-400e-8340-b9e60da6c9a7)` を指しているので、
mop の文書がこの UUID のアンカーを引き継ぐ(参照を切らないため。新しい UUID にするなら、据えた unit の
コメントも書き換える命令が要る)。

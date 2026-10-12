# vega の 3 つの llama-server と 8082 の利用者(読むだけの実測、2026-10-01)

<a id="0a4b121f-6ba8-4bc2-87ef-d804ff3401fa"></a>

日付: 2026-10-01 08:3x〜09:0x JST。[docs/plan/LLM_UNITS.md](#0ee79be6-f2bd-400e-8340-b9e60da6c9a7) の前提を
集めた観測。`ps`・`/proc/<pid>/{cmdline,cwd,environ,limits,cgroup,stat}`・`ss`・`nvidia-smi`・`readelf`・`ldd`・
`systemctl cat`・/etc の読める範囲と、/work2/llm の起動用の手順ファイルとログを読んだ。root は使っていない。
プロセスへのシグナル・設定の変更・負荷はかけていない。GPU の内訳は
[docs/analysis/20261001-vega-gpu-vram.md](#f24219ab-96b3-47af-9ea7-ef1ebb80fc47)。

## プロセス

3 つとも同じバイナリ /work2/llm/vulkan/llama-b10025/llama-server(`--version` は 10025 (a3e5b96ac))で、
利用者 hikalium の端末の session scope(user.slice/user-1000.slice/session-4056.scope)の下にいる。
systemd の unit ではないので、機械を再起動すると戻らない。

| pid | 口 | モデル | 起動 | 親 | 作業ディレクトリ | RSS | VRAM |
|---|---|---|---|---|---|---|---|
| 888085 | 127.0.0.1:8084 | bge-reranker-v2-m3 FP16 | 2026-08-18 | 1(親が先に終わった) | /work2/llm_playground_host_dir/uniqnode | 2.7 GiB | 650 MiB |
| 2166473 | 127.0.0.1:8083 | bge-m3 FP16 | 2026-08-17 | `bash -c …nohup ./run-bge-m3.sh…`(run-bge-m3.sh は exec で llama-server に替わる) | /work2/llm | 3.0 GiB | 648 MiB |
| 3995999 | 0.0.0.0:8082 | gpt-oss-120b MXFP4(3 分割、63.4 GB) | 2026-07-17 | `bash ./run-gptoss120b.sh`(exec しないので、子が終われば親も終わる) | /work2/llm | 66.5 GiB | 2,844 MiB |

命令行(`/proc/<pid>/cmdline` の NUL を空白に替えたもの。`-ot` の値と JSON はそれぞれ 1 つの引数):

```
/work2/llm/vulkan/llama-b10025/llama-server --model /work2/llm/models/bge-reranker-v2-m3/bge-reranker-v2-m3-FP16.gguf --host 127.0.0.1 --port 8084 --reranking --pooling rank -ngl 99 --ctx-size 8192 --batch-size 2048 --ubatch-size 2048
/work2/llm/vulkan/llama-b10025/llama-server --model /work2/llm/models/bge-m3/bge-m3-FP16.gguf --host 127.0.0.1 --port 8083 --embeddings --pooling cls --embd-normalize 2 -ngl 99 --ctx-size 8192 --parallel 4 --batch-size 2048 --ubatch-size 2048 --threads 8
/work2/llm/vulkan/llama-b10025/llama-server --model /work2/llm/models/gpt-oss-120b/gpt-oss-120b-mxfp4-00001-of-00003.gguf --host 0.0.0.0 --port 8082 -ngl 99 -ot \.ffn_(gate|up|down)_exps\.=CPU --threads 26 --ctx-size 16384 --parallel 1 -fa on --no-mmap --jinja --chat-template-kwargs {"reasoning_effort":"low"}
```

8083 は /work2/llm/run-bge-m3.sh を既定のまま、8082 は /work2/llm/run-gptoss120b.sh を `HOST=0.0.0.0` を
指定して(手順ファイルの既定の HOST は 127.0.0.1)起こしたときの命令行と一字一句同じである。8084 には
手順ファイルが無い。

環境(値は秘密を含みうるので名前だけを見た。LD_LIBRARY_PATH だけ値を読んだ):

- 8083 と 8082 は `LD_LIBRARY_PATH=/work2/llm/vulkan/llama-b10025:`(手順ファイルが足したもの。末尾の `:` は
  空の要素で、作業ディレクトリも探す意味になる)。8084 には無い。バイナリの RUNPATH が `$ORIGIN` なので、
  どちらでも同じ .so(/work2/llm/vulkan/llama-b10025/ の下)を読む(`ldd` で確かめた)。
- 他は端末のセッションと Claude Code の変数(TMUX・SSH_*・CLAUDE_*・PATH・HOME など)で、llama-server が読む
  `LLAMA_ARG_*` は無い。
- 上限: 開けるファイル 1,048,576、ロックできるメモリ 67,581,657,088 バイト(利用者のセッションから継いだ値)。
  system 単位の既定はそれぞれ 524,288(soft 1,024)と 65,536 バイトである(`systemctl show -p DefaultLimit…`)。
  llama-server は `--no-mmap` で `--mlock` も無いので、ロックの上限は今の設定では効かないはずである。
- oom_score_adj は 0、nice は 0、CPU の割り当ては 0-103 の全部。NoNewPrivs は 0(/proc/<pid>/status)。
- 出力: 8082 は /work2/llm/server-gptoss.log、8083 は /work2/llm/server-bge-m3.log、8084 は /tmp/rerank-server.log
  へ標準出力と標準エラーを書いている(/proc/<pid>/fd/1・2)。

GPU: RTX 2080(8 GiB)1 枚、プロプライエタリのドライバ 560.35.05。3 つは /dev/nvidia0・/dev/nvidiactl・
/dev/nvidia-modeset を開いている(/proc/3995999/fd)。これらは 0666 なので、利用者 hikalium は video・render の
組に入っていないが困らない(/dev/dri は使っていない)。~/.cache/nvidia/GLCache にシェーダのキャッシュを書く。
nvidia-persistenced.service は masked。/work2 は別のファイルシステム(/dev/nvme0n1p1、ext4)。

読み込みの時間(ログの起動の刻み): gpt-oss-120b は起動から「model loaded」まで 39 秒
(/work2/llm/server-gptoss.log の先頭。63 GB がページキャッシュに無ければもっとかかる)、bge-m3 は 3 秒。

## llama-server b10025 の振る舞い

タグ b10025 の tools/server/server.cpp・tools/server/server-http.cpp・common/arg.cpp を読み、バイナリの
`--help` と突き合わせた。

- 待ち受けの束縛(`ctx_http.start()`)をモデルの読み込み(`load_model`)より先に行い、束縛に失敗すると
  すぐ `return 1` で終わる。口が塞がっていれば、63 GB を読む前に止まる。読み込みの失敗も exit 1 である。
- 読み込みの間、`/health` などの API は 503 を返し、済むと 200 になる。OPTIONS はこの判定より先に答える。
- SIGTERM と SIGINT を同じ処理で受け、行儀よく終わる。
- `/slots` は既定で有効で、スロットごとに `"is_processing":true|false` を詰めた JSON で返す(8084 で確かめた)。
- 既定の CORS は `--cors-origins *` と `--cors-credentials`(有効)。この組では、要求の Origin をそのまま
  `Access-Control-Allow-Origin` に返し、OPTIONS には `Access-Control-Allow-Credentials: true` を返す
  (server-http.cpp の事前の振り分け)。つまり、どのサイトの頁のスクリプトからでも応答を読める。起動時に
  「CORS is set to allow all origins ('*') and no API key is set」と警告する(server-gptoss.log の先頭に在る。
  条件は `cors_origins == "*"` かつ API キーが無いこと)。
- `--cors-origins` の help は「カンマで並べた一覧」と言うが、実装は値をそのまま 1 つのヘッダに入れる。
  カンマで並べるとブラウザが受け付けない値になるので、使えるのは 1 つの生成元か、特別な値 `localhost`
  (Origin が localhost・127.0.0.1・::1 のときだけ返す)である。`localhost` は Origin の無い要求(ブラウザ
  でない呼び手の全部)でも「skip non-localhost origin」の警告を 1 行ずつ出す。
- 許さない生成元の要求も拒まずに処理する。1 つの生成元を書いた形では、要求の Origin に関わらずその値を
  `Access-Control-Allow-Origin` に入れ、要求元と一致しないのでブラウザが応答を読ませない。`localhost` の特別な
  値で一致しないときは、ヘッダを付けない。どちらも、CORS はブラウザに応答を読ませない仕組みで、口へ届く
  相手を限るものではない。

## 誰が使っているか

- 8083・8084: uniqnode の serve。/etc/systemd/system/uniqnode-serve.service.d/override.conf の
  `--embed http://127.0.0.1:8083/v1/embeddings --rerank http://127.0.0.1:8084/v1/rerank`。15 分の `ss -tn` の
  標本(およそ 1 秒ごと、08:48〜09:03)で、8083・8084 には 127.0.0.1 からの接続だけが見えた。serve は届かない
  とき黙らずに劣化する(埋め込みは BM25 だけ、rerank は元の順位。node/src/embed.rs と node/src/rerank.rs の冒頭)。
- 8082: lamalium。lamalium の host/backend-proxy.toml(手元の古い写し、ca1fdca)は、コンテナから見える
  `10.100.0.1:11436` を `10.10.128.1:8082` へ素朴な TCP で転送する(ホストの server-proxy が張る。
  [docs/plan/LAMALIUM.md](#68571059-94ed-4aa2-8ae0-b2862d1de44e))。helpdesk の役は gpt-oss-120b に pin して
  いる。lamalium のホストは orion(10.10.128.4)から crystal(10.10.128.2)へ 2026-09-29 から移している途中で、
  どちらも wg1 越しに届く。lamalium の CLI の persona-review も既定の URL が `http://10.10.128.1:8082` である
  (container/src/cli.rs)。viewer の keeper の外部レビューも `http://10.100.0.1:11436` の gpt-oss-120b を呼ぶ
  (container/src/keeper.rs:270)。/work2/llm/server-gptoss.log には処理の始まり(`launch_slot_` の行)が
  2,239 回(2026-07-17 からの約 76 日で、1 日平均約 30)あり、この期間に使われていた。行の頭の刻み(起動からの
  分)を 24 時間ごとに区切って awk で数えると、77 の区切りのうち使われたのは 41 で、多い区切りは 300 回を超え、
  1 回も無い区切りが 9〜10 続く期間が 2 度ある。最後の区切り(2026-10-01)にも 7 回ある。毎日使われているとは
  言えない。
- 8082 の他の相手は、root が無いと過去を確かめられない(接続の追跡表 conntrack は読めず、llama-server の
  ログは送信元を出さない)。15 分の標本では 8082 への接続は見えなかった。vega の docker の nginx の設定
  (/home/hikalium/repo/z01.hikalium.com/hosts/vega/templates)に 8082 は無い。
- 0.0.0.0 に束縛しているので、8082 は vega の全部のアドレスで開いている: wg1 の 10.10.128.1、br0 の
  10.10.10.93(LAN)、docker の橋(172.17〜172.23 の各 .1)、loopback。IPv6 には束縛していない。

## ファイアウォール

- nftables.service は disabled。/etc/nftables.conf は空の `inet filter` だけで、読まれていない。
- ufw.service は active だが、/etc/ufw/ufw.conf が `ENABLED=no` なので ufw の規則は入らず、邪魔をしない。
- unit から読める表は uniqnode の 3 つ: `inet uniqnode`(7441)・`inet uniqnode_graph_a`(7445)・
  `inet uniqnode_graph_b`(7443)。どれも serve の drop-in の隣の agent-door.nft を、unit の
  `ExecStartPre=+/usr/sbin/nft -f …` が起動のたびに入れる。各表は自分の口だけを drop し、他の口には触れない。
  実際に入っている表の一覧(`nft list tables`)は root が要るので読んでいない。docker が走っているので、
  docker の規則もあるはずである。

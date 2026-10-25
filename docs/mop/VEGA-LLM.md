# VEGA-LLM — vega の 3 つの llama-server を systemd の user 単位の unit で動かす

<a id="71cc086a-cf00-4bb8-bddb-ef5aa68562fe"></a>

読み手は、vega の LLM のサーバーを据え付け、更新し、止める担当 (vega の Claude のセッションなど)。
unit ファイルは docs/mop/vega-llm/ にある。uniqnode のバイナリには埋め込まない (node/src/install.rs
の unit とは別)。

## 何のためか

vega は次の 3 つの llama-server を動かす。どれも利用者 hikalium のプロセスで、バイナリは
/work2/llm/vulkan/llama-b10025/llama-server である。

| unit | ポート | モデル | 使う側 |
|---|---|---|---|
| llama-server-gpt-oss-120b.service | 0.0.0.0:8082 | gpt-oss-120b (MXFP4、3 分割で 63.4 GB) | lamalium の helpdesk、review、qa_translate (crystal の 10.10.128.2 から) |
| llama-server-bge-m3.service | 127.0.0.1:8083 | bge-m3 (FP16) | uniqnode serve の `--embed` |
| llama-server-bge-reranker-v2-m3.service | 127.0.0.1:8084 | bge-reranker-v2-m3 (FP16) | uniqnode serve の `--rerank` |

2026-10-25 までは 3 つとも端末から手で起動していたので、vega を再起動すると戻らなかった。vega は
2026-10-22 に再起動し、gpt-oss は 10-25 に手で起動し直すまで、bge-m3 と reranker は unit に替える
まで止まっていた。その間、lamalium の review と qa_translate は vega に届かず、uniqnode の検索は
埋め込みと rerank を使えずに BM25 だけと元の順位に落ちていた (node/src/embed.rs と
node/src/rerank.rs の冒頭)。unit にすると、vega の起動で 3 つが自動で起動し、落ちたときも
systemd が起動し直す。

## 決めたこと

- 8082 は今のまま全アドレス (0.0.0.0) で待ち受け、nft の規則も CORS の設定も変えない。
  2026-10-25 の測定 (送信元の IP アドレスは特定できず、設定から分かる呼び手は crystal だけ) を
  材料に、操作者が選択肢 C を選んだ (2026-10-25T15:53Z)。以前の計画 LLM_UNITS 第 8 版の、8082 を
  nft と CORS で絞る部分は採らない。
- system 単位ではなく、hikalium の user 単位の unit にする。hikalium は linger が有効
  (`loginctl show-user hikalium -p Linger` が yes) なので、user 単位の unit も vega の起動で
  起動する。据え付けに root が要らず、プロセスの持ち主も手で起動していたときと同じである。
- gpt-oss と bge-m3 の引数は、/work2/llm の起動のスクリプト (run-gptoss120b.sh と
  run-bge-m3.sh) が決める。unit はスクリプトを呼ぶだけなので、引数を変えるときはスクリプトを
  直して unit を再起動する。gpt-oss は環境変数 HOST=0.0.0.0 だけを与える (スクリプトの既定は
  127.0.0.1)。reranker には起動のスクリプトが無いので、引数を unit に書く。値は 2026-08-18 から
  手で起動していた版と同じである
  ([docs/analysis/20261001-vega-gpu-vram.md](#f24219ab-96b3-47af-9ea7-ef1ebb80fc47))。
- 起動の順序は付けない。VRAM (RTX 2080、8 GiB) は 3 つの合計で約 4.1 GiB で、どの順でも収まる。
- /work2 は fstab で nofail が付いているので、vega の起動で user 単位の unit が /work2 の mount より先に
  起動することがある。その場合も、unit は 30 秒ごとに起動し直す (30 分に 3 回まで試す) ので、mount が約 1 分で
  済めば戻る。
- 落ちたら 30 秒後に起動し直し、30 分に 3 回失敗したら諦める (gpt-oss は 1 回の起動で 63 GB を読むため)。
- 出力は /work2/llm の下のログへ追記する (server-gptoss.log、server-bge-m3.log、server-rerank.log)。

## 据え付けと切り替え

vega で hikalium として、uniqnode のチェックアウトの根から実行する。sudo は要らない。

1. unit を置いて読み込ませる。
   `install -D -m 0644 -t ~/.config/systemd/user/ docs/mop/vega-llm/llama-server-*.service` の後に
   `systemctl --user daemon-reload` を実行する。
2. 手で起動した同じポートのサーバーが残っていれば止める。`ss -tlnp 'sport = :<ポート>'` で
   持ち主を探し、`curl -s http://127.0.0.1:<ポート>/slots` の全スロットの `is_processing` が false の
   ときに、その llama-server へ SIGTERM を送る。gpt-oss の親の `bash ./run-gptoss120b.sh` は子が
   終われば終わる。ポートが塞がっていると、unit の llama-server はモデルを読む前に exit 1 で止まる。
3. `systemctl --user enable --now llama-server-gpt-oss-120b.service llama-server-bge-m3.service llama-server-bge-reranker-v2-m3.service`
   を実行する。
4. 確かめる。`systemctl --user is-active` が 3 つとも active で、
   `curl -s http://127.0.0.1:<ポート>/health` が 200 を返すこと (gpt-oss は読み込みの間 503 を返し、
   約 40 秒で 200 になる)。GPU を使っていることは、
   `nvidia-smi --query-compute-apps=pid,used_memory --format=csv` で 3 つの unit の llama-server の pid が
   VRAM を持っていることで見る (Vulkan の機器が見えないと、llama-server は CPU で動いて 200 を返す。
   b10025 の llama-server はログに Vulkan の機器の行を出さない)。2026-10-25 の切り替えでは 3 つで
   約 4.1 GiB を持っていた。
   `systemctl --user is-enabled` が 3 つとも enabled であれば、次の vega の起動でも起動する。

## 更新と停止

- unit ファイルを変えたら、手順 1 をやり直して `systemctl --user restart <unit>` を実行する。
- 起動のスクリプトの引数だけを変えたら、`systemctl --user restart <unit>` だけでよい。
- 3 回の失敗で諦めた unit や、続けて restart して拒否された unit は、
  `systemctl --user reset-failed <unit>` の後に `systemctl --user start <unit>` を実行する。
- 止めるときは `systemctl --user stop <unit>`、自動の起動もやめるなら `systemctl --user disable <unit>`。
- 起動と終了の記録は `journalctl --user -u <unit>` にあり、サーバーの出力は上のログにある。

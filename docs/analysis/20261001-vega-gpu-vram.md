# vega の GPU の常駐 VRAM(読むだけの実測、2026-10-01)

<a id="f24219ab-96b3-47af-9ea7-ef1ebb80fc47"></a>

日付: 2026-10-01 00:27〜00:28 JST。lamalium の作業グラフの節点 n_gpu_vram_vega の割り当て。
`nvidia-smi`・`ps`・`/proc/<pid>/cmdline` を読んだだけで、プロセスの再起動・設定変更・負荷は
かけていない。

## GPU

| GPU | 名前 | VRAM の総量 | 使用 | 空き | 使用率 | 状態 | 電力 |
|---|---|---|---|---|---|---|---|
| 0 | NVIDIA GeForce RTX 2080(Bus 00000000:AF:00.0) | 8,192 MiB | 4,272 MiB | 3,518 MiB | 0 % | P8 | 1〜5 W / 225 W |

GPU は 1 枚だけである。ドライバ 560.35.05、CUDA 12.6。llama.cpp は Vulkan の版
(/work2/llm/vulkan/llama-b10025/llama-server)で走っている。

## プロセスごとの内訳

3 つとも同じ llama-server のバイナリで、利用者 hikalium の端末の session scope
(user-1000.slice/session-4056.scope)の下で走っている。systemd の unit ではない。

| pid | 役割 | 口 | モデルと量子化 | VRAM | 主な引数 | 起動 |
|---|---|---|---|---|---|---|
| 2166473 | uniqnode の埋め込み(`--embed`) | 127.0.0.1:8083 | bge-m3、FP16 の GGUF(1.16 GB) | 648 MiB | `--embeddings --pooling cls --embd-normalize 2 -ngl 99 --ctx-size 8192 --parallel 4 --batch-size 2048 --ubatch-size 2048` | 2026-08-17 |
| 888085 | uniqnode の rerank(`--rerank`) | 127.0.0.1:8084 | bge-reranker-v2-m3、FP16 の GGUF(1.16 GB) | 650 MiB | `--reranking --pooling rank -ngl 99 --ctx-size 8192 --batch-size 2048 --ubatch-size 2048` | 2026-08-18 |
| 3995999 | 汎用の LLM(lamalium の helpdesk などが使う) | 0.0.0.0:8082 | gpt-oss-120b、MXFP4 の GGUF(3 分割、計 63.4 GB) | 2,844 MiB | `-ngl 99 -ot '\.ffn_(gate\|up\|down)_exps\.=CPU' --ctx-size 16384 --parallel 1 -fa on --no-mmap` | 2026-07-17 |

3 つの合計は 4,142 MiB で、nvidia-smi の使用 4,272 MiB との差の約 130 MiB はドライバと表示の分と
みられる(プロセスの表に出ない)。

gpt-oss-120b は MoE の専門家の重み(ffn の gate・up・down の exps)を `-ot … =CPU` で CPU に置き、
それ以外(注意と共有の層、KV キャッシュ)だけを GPU に載せている。そのため 63 GB のモデルでも
VRAM は 2.8 GiB で、残りはホストのメモリにある(このプロセスの RSS は約 66.5 GiB。機械は 503 GiB)。

埋め込みと rerank はモデル全体を GPU に載せている(`-ngl 99`)。FP16 の重み 1.16 GB に対して
VRAM が 650 MiB 前後なのは、Vulkan の版が重みの一部をホストに置いているか、nvidia-smi が Vulkan の
割り当ての一部しか数えていないためと思われる(この実測では切り分けていない)。

## アイドルと負荷の差

測った時点は 3 つともアイドルだった(使用率 0 %、P8)。3 回続けて読んで、VRAM の数字は 1 MiB も
動かなかった。llama.cpp は重み・KV キャッシュ・計算用のバッファを起動時にまとめて確保するので、
要求を受けても VRAM の使用量はほぼ変わらず、変わるのは使用率と電力である。負荷時の VRAM を直接
測った記録は見つからなかった(/work2/llm/server-*.log には起動時のバッファの大きさが残っていない)。
負荷をかけて確かめることはしていない。

## 空きの見込み

空きは 3.5 GiB ある。同じ形の小さな常駐(bge 級の FP16 モデル、650 MiB 前後)なら、あと数本は
載る。gpt-oss-120b の文脈長を伸ばすと KV キャッシュの分だけ増える。

## 気づいたこと(この割り当ての外)

- 3 つとも利用者の端末の session scope で走っていて、systemd の unit ではない。機械を再起動すると
  戻らない。uniqnode の serve は `--embed`・`--rerank` にこの 2 つを使うので、再起動の後は手で起こす
  必要がある。
- gpt-oss-120b は 0.0.0.0:8082 に束縛していて、llama-server 自身が起動時に「CORS がすべての生成元を
  許し、API キーも無い」と警告している(/work2/llm/server-gptoss.log の先頭)。

# RAG: 検索ストレージの残りの作業項目

<a id="86363f4a-3df6-4aa2-9c64-b99aa5cb4e7b"></a>

現在の状態は design/ が正典である(土台は
[docs/design/OVERVIEW.md](#cfa85cc2-0d72-49f2-b628-b9622589851b)、その上の層は
[INGEST](#47d69a3e-c39a-4e76-9814-e9c24240293b)・
[SEARCH](#19574e78-9bf5-4f87-a4c2-c4a10222c580)・
[EVAL](#1109a04b-923e-4493-8f00-d704047d6a2a)・
[MCP](#dacd474d-424a-45d5-a278-766fc2465dd9)・
[VIEWER](#4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847)・
[DISTRIBUTED_SEARCH](#e577f6db-659e-4eb8-a152-3b7780e4a9d1))。「LLM から常用できる RAG
ストレージ」というこの計画のゴールは、そこで成立している。以下は、その上に積む残りの作業項目を
実行順に並べたものである(先頭は 12 の小さな穴、次が 8 の pack GC。番号は着手順ではなく
起票順で、済んで削除された項目の番号は再利用しない)。

## 12. 常駐で見つかった小さな穴

<a id="6889f68f-ac27-4460-b229-4eba7fa9fc33"></a>

2026-09-05 に本番ノードを systemd に据え付けたとき
([docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2))に見つかった、どれも小さいが
運用者を騙しうる挙動。1 つずつ、欠陥を復元して赤を確かめたテストを添えて直す(should/0137)。

- serve はポートを束縛して `listening on` を出してからストアを開く(node/src/main.rs の
  serve 節)。錠を取れないときはその行の後で exit 1 するので、この行を待つ起動確認は騙される。
  ストアを開いてから束縛する(少なくとも表示する)順にする。
- `--rerank <url>` / `--reranker <id>` は parse_embed_options を共有する mcp と embed も
  受け付けるが、mcp のローカル形は `reranker: None` で embed は使わない。明示された指定の
  黙殺なので、断るか装備する(must/0022 の同型)。
- `uniqnode fsck` を空のディレクトリに向けると、新しいストアを初期化して node_key を作り、
  objects 0 の緑を返す。復元先を先に fsck した写しは別ノードになり backup に断られる
  ([docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2) の罠)。fsck は在るストア
  だけを開き、無ければ断る。
- system 単位のストア(0750、所有者 uniqnode)に人間の権限で mcp を起こすと、既定のログ先
  `<dir>/logs/mcp.log` に書けず標準エラーだけになり、それはクライアントが吸う。書けないとき
  は利用者の書ける場所へ倒す既定にする(SYSTEMD.md の `--log` の回避策を不要にする)。

## 8. pack GC(機会層 evict の物理回収)

<a id="4ed764d8-a570-4809-bd0c-80de0d4b5545"></a>

pack の書き直しで evict 済みオブジェクトのディスクを回収し、その上で min_replicas=0 の
LRU+参照カウント evict(SPEC §5.4)を有効化する。容量の小さいノードを本格運用する前提。
同じ層の残りとして、`renditions/` の輸出除外がある(写しは伝播交換にそのまま載るので、
他ノードへ配りたくない運用では外せる必要がある。
[docs/design/RENDITION.md](#6046eeca-1d95-4d47-87da-13f86c7710dc))。

## 9. 失効文とグループ設定の伝播交換への載せ替え

<a id="63f59b10-f504-45f4-ac41-d80859c49509"></a>

現状は groups.json への手動配置。署名済み文書なので伝播交換に載せる素地はある。

## 10. 分散検索の残り

<a id="05eb44a5-e41b-4bc9-aae3-194c019ad124"></a>

オブジェクト単位の共有ポリシー(現状の share.collections が限るのは検索からの見つけ方だけ
で、チャンクの ID を知る相手は GET /v1/objects/{id} で読める。
[docs/design/DISTRIBUTED_SEARCH.md](#e577f6db-659e-4eb8-a152-3b7780e4a9d1))、kind:search の
クエリハンドル(回答の単調増加集合の観測。SPEC §7.2 の SHOULD)、常用の口(MCP の search
ツールとビューワ)から散布を頼めるようにすること(どちらも 1 台に閉じたままなので、必要に
なってから決める)。

## 11. その先

<a id="0d6e3623-c1f5-4fb5-8579-870390a89f4b"></a>

ウェブページの取り込みは、HTML を渡せば読めて原本のまま返せるところまで来ている
([docs/design/INGEST.md](#47d69a3e-c39a-4e76-9814-e9c24240293b)・
[docs/design/RENDITION.md](#6046eeca-1d95-4d47-87da-13f86c7710dc))。残るのは、URL を渡せば
ノード自身が取りに行き、外部への依存(スクリプト・画像・フォント)を落として自足する 1 枚に
してから収める道である。ほかに、mgcanvas 形式エクスポートによる可視化、Contextual Retrieval
などの検索品質向上(評価ハーネスで測りながら)。

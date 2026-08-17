# RAG: 検索ストレージの残りの作業項目

<a id="86363f4a-3df6-4aa2-9c64-b99aa5cb4e7b"></a>

現在の状態: SPEC の適合レベル L0〜L4(単一ノードストア・レプリケーション・分散クエリ・
レプリカ健全性・グループ鍵)に加えて、取り込み層
([docs/design/INGEST.md](#47d69a3e-c39a-4e76-9814-e9c24240293b))、キーワード・意味・
ハイブリッドの 3 方式の検索
([docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580))、検索方式を差し替えて
数値で比べる評価ハーネス([docs/design/EVAL.md](#1109a04b-923e-4493-8f00-d704047d6a2a))、
search と fetch を LLM エージェントへ出す MCP アダプタ
([docs/design/MCP.md](#dacd474d-424a-45d5-a278-766fc2465dd9))、ブラウザから引く 1 枚の
RAG ビューワ([docs/design/VIEWER.md](#4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847))、署名付き
QUERY を登録ピアへ散布して順位を融合する分散検索
([docs/design/DISTRIBUTED_SEARCH.md](#e577f6db-659e-4eb8-a152-3b7780e4a9d1))まで実装済み。
テスト205本、CLI + HTTP API + MCP + ビューワ。
「LLM から常用できる RAG ストレージ」というこの計画のゴールは、ここで成立している。
以下は、その上に積む残りの作業項目を実行順に並べたものである(先頭は 7 の運用の仕上げ。
済んで削除された項目の番号は再利用しない)。

## 7. 運用の仕上げ

systemd unit の例、バックアップ手順(封印セグメントの rsync + 復元検証)、README の運用節。
完了条件: 常用ノードが systemd で動き、バックアップからの復元手順が一度実証されている。

## 8. pack GC(機会層 evict の物理回収)

<a id="4ed764d8-a570-4809-bd0c-80de0d4b5545"></a>

pack の書き直しで evict 済みオブジェクトのディスクを回収し、その上で min_replicas=0 の
LRU+参照カウント evict(SPEC §5.4)を有効化する。容量の小さいノードを本格運用する前提。

## 9. 失効文とグループ設定の伝播交換への載せ替え

現状は groups.json への手動配置。署名済み文書なので伝播交換に載せる素地はある。

## 10. 分散検索の残り

オブジェクト単位の共有ポリシー(現状の share.collections が限るのは検索からの見つけ方だけ
で、チャンクの ID を知る相手は GET /v1/objects/{id} で読める。
[docs/design/DISTRIBUTED_SEARCH.md](#e577f6db-659e-4eb8-a152-3b7780e4a9d1))、kind:search の
クエリハンドル(回答の単調増加集合の観測。SPEC §7.2 の SHOULD)、MCP の search ツールから
散布を頼めるようにすること(常用の口が 1 台に閉じたままなので、必要になってから決める)。

## 11. その先

Web スナップショット(rag_plan の W トラック)、mgcanvas 形式エクスポートによる可視化、
Contextual Retrieval・リランカーなどの検索品質向上(評価ハーネスで測りながら)。

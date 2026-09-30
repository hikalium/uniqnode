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
実行順に並べたものである(番号は着手順ではなく起票順で、済んで削除された項目の番号は再利用
しない)。順序の根拠は「1 台の今のノードで効くものを先に、2 台目を置くときに効くもの(8・15〜17・9・10)
を後に」である(2026-09-05 の裁定。2 台目はいつか置くが未定)。1 台で効く残りは 11 の検索
品質向上だけで、これは評価ハーネスで測って劣化が見えたときに動く。したがって先頭は 8 の
evict だが、着手は 2 台目の計画が立ってからでよい。

## 14. lamalium につなぐ

<a id="1c8c5b0e-2c0a-4a1e-9d3a-7f2c6e5b4a11"></a>

uniqnode の一番の利用者は lamalium(別の機械 orion で動くマルチエージェント系。uniqnode は
vega にあり、2 台は WireGuard の wg1 で疎通する)になる予定である。
計画は [docs/plan/LAMALIUM.md](#68571059-94ed-4aa2-8ae0-b2862d1de44e)。uniqnode 側の作業は、
wg1 のアドレスに束縛する読み口(`--listen-agent`。許可表の外は 403)と第 2 段の
`--agent-writable`、search の `full` と `GET /v1/collections`、索引の温め、出所の meta、
BENCH の問いによる評価で、lamalium 側の
`uniqnode_search`・`uniqnode_get`・`uniqnode_status` ツールと組になる。方針はエージェントが
自発的に見つけて呼ぶこと(push は測ってから)。着手順は同文書の段階 L0〜L4。1 台の uniqnode で
効き、2 台目の uniqnode を待たないので、8 より前に置く。

## 8. pack GC(機会層 evict の物理回収)

<a id="4ed764d8-a570-4809-bd0c-80de0d4b5545"></a>

孤児(どの ref・pin・保持表明からも辿れないオブジェクト)を含む pack の書き直しは着地して
いる(`uniqnode gc` と `POST /v1/admin/gc`。[docs/design/GC.md](#9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。
残るのは、その上で min_replicas=0 の機会層を容量契機で「参照されない」状態にする evict
(SPEC §5.4)で、容量の小さいノードを本格運用する前提。設計と未決事項は
[docs/plan/PACK_GC.md](#f272eeda-8664-42da-9e5c-ef354bc3f3a7)。
同じ層の `renditions/` の輸出除外は 15 に切り離した。

## 15. `renditions/` の輸出除外

<a id="430e8efe-3785-4006-ae95-606f50d2e9b3"></a>

写しは伝播交換にそのまま載るので、他ノードへ配りたくない運用では外せる必要がある。筋は副鍵
(PACK_GC.md の「`renditions/` の輸出除外」の (c))だが、fsck・同期・署名者一覧に手が入り、
8 の本体とは独立である。写しの tombstone が 8 の主要な入力になるので、8 の段階 2 より前に
置く(PACK_GC.md のセルフレビュー (7))。

## 16. reflog の肥大

<a id="bc12998a-a801-4604-a37b-80a59228bc73"></a>

ref の履歴は全部残り、export_ref_records は毎回全 reflog を走査する。SPEC §4.4 は古い
レコードの忘却を許すが、同期の since カーソルが seq 連続を要求するので、忘却は「ある seq
より前を持たない」と宣言する形でしかできず、新しいピアの初回同期が成り立たなくなる。
pack GC とは別の問題(PACK_GC.md のセルフレビュー (8))。

## 17. 導出データの詰め直し

<a id="a96b6d9c-3607-45ad-8fc0-ec49f51f2b99"></a>

gc で消えたオブジェクトのベクトルが `derived/embeddings/` に残る。写しの「最後に見られた
時刻」も同じ扱いになる。どちらも導出データで小さく、容量が効いてから動く(PACK_GC.md の
セルフレビュー (9))。

## 18. doc_rev の previous が過去の版を gc から守ってしまう

<a id="d75f287e-ce56-4bf0-a85e-6a9ffb3dc571"></a>

取り込みの doc_rev は previous に前の版の ID を `s256:` 付きで持つ(ingest.rs)。gc は値の中の
`s256:` を参照として辿るので(GC.md)、公開中の版から同じ文書の過去の全版とそのチャンクが
辿れ、上書きしても回収されない。GC.md の「同じ ref パスへの上書きで孤児が生まれる」という
記述と食い違い、2026-09-05 の dry-run で孤児が 1 件しか無かったことと整合する。直し方は
previous を参照でない形(`s256:` を付けない 16 進)にすることで、feed の設計
([docs/plan/FEED.md](#fa8de6f9-59f8-4512-a815-9f41d305db15))はそうしている。既存の doc_rev の
ID が変わるので、移し方(再取り込みで新しい形の版を作る)と SPEC §4.3 との関係を決めてから
動く。容量が効くまで急がない。

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

mgcanvas 形式エクスポートによる可視化、Contextual Retrieval などの検索品質向上(評価
ハーネスで測りながら)。

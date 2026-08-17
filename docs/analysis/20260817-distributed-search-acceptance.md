# 分散検索の手動受け入れ: 2 台に分けた知識を 1 本の問いで引く

<a id="08a2c68d-7c50-4aaf-990b-54919ef5a39c"></a>

日付: 2026-08-17。対象: 分散検索(DISTRIBUTED_SEARCH
(uuid:e577f6db-659e-4eb8-a152-3b7780e4a9d1))の完了条件「SPEC §7.1 の kind:search が動き、
2 台に分かれた知識が 1 本のクエリで返る」の手動確認。手段: コミット 917fe53 の release
ビルドで、空のストアを 2 つ(/tmp/dsA を 127.0.0.1:7451、/tmp/dsB を 127.0.0.1:7452)起こし、
生の curl で当てた。自動の対は node/tests/distributed_search.rs にある(8 本)。

## 構成

日本語の文書(「世代の整合」の節)は dsA にだけ、英語の文書(token_estimate の節)は dsB に
だけ置いた。peers.json は双方に 1 件ずつで、dsB の側は `share: {"collections":["notes"]}` を
持つ。埋め込みサーバは使っていないので、両者ともローカルの方式は bm25 である。

## 観測

散布しない要求(これまでどおりの POST /v1/search)は自分の索引だけで答え、応答の形も
変わらなかった: 「世代の整合 token_estimate」に対して memo_ja の 1 件、method bm25、
score_semantics bm25、outcome の欄は無し。

同じ問いに `"peers": true, "budget_ms": 3000` を付けると、1 本の要求で両方の知識が返った。

| 件 | 出所 | 得点 | 引用 |
|---|---|---|---|
| memo_ja 位置 0 | local | 0.01639344262295082 | 分散設計 > 世代の整合 |
| memo_en 位置 0 | 127.0.0.1:7452 | 0.01639344262295082 | Retrieval notes > Chunker internals |

- outcome は found、peers は 1 件で `{"address":"127.0.0.1:7452","node_id":"3f00773d…",
  "hits":1,"state":"answered"}`、score_semantics は rrf だった。
- 得点が両者同じなのは、各DBノードが 1 件ずつしか返さず、どちらも順位 1 位で 1/(60+1) に
  なるためである。順位ベースの融合の素直な帰結であって、異常ではない。

共有していないコレクションは検索から出てこない。dsB に secrets コレクション(「合言葉」の
節)を足して dsA から同じ語で問うと、dsB は 0 件の ANSWER を返し(state empty)、決着は
scope_empty だった。dsB 自身のローカル検索では同じ語でその節が 1 位に出るので、隠したのは
共有ポリシーである。

## 見つけた限界(design 側に記載済み)

チャンクの ID を別の道で知っている相手には、共有していないコレクションのチャンクも読める。
上の secrets の ID(s256:395669c4…)を手で dsA へ渡して POST /v1/query(kind:object)を
投げると、outcome found で本文が取り寄せられた。GET /v1/objects/{id} は content-addressed で
自己認証的なオブジェクトを ID を知る相手へ返す口であり、share が限るのは検索からの見つけ方
だけである。オブジェクト単位の共有ポリシーは残作業として
[docs/plan/RAG.md](#86363f4a-3df6-4aa2-9c64-b99aa5cb4e7b) に置いた。

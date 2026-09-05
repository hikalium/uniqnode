# uniqnode

分散マルチノードのグラフ知識データベース。

各データベースノード(DBノード)は content-addressed で不変なオブジェクト(グラフの頂点と辺)を保持し、
手動設定された暗号化リンクで相互接続される。クエリはネットワークに問いかけられ、答えを持つDBノードが
いれば返事が来る。誰も答えなければ、いつまでも何も来ない。「見つからない」は「存在しない」を意味しない
(開世界仮説)。

- 仕様: [SPEC.md](SPEC.md)
- 計画: docs/plan/
- 運用手順: docs/mop/
- 検証・分析: docs/analysis/ と sim/(プロトコルシミュレータ)
- プロジェクトポリシー: [policy/](policy/README.md)(RFC 2119 レベル別)

## 状態

適合レベル L0〜L4(単一DBノードのストア・伝播交換・分散クエリ・レプリカ健全性・グループ鍵。
SPEC §11)を実装済み。依存クレートなし。

- node/: 実装本体。SHA-256/512、Ed25519(RFC 8032、openssl と相互検証済み)、
  正規化 JSON(c1)、pack セグメント+MANIFEST+署名付き reflog のストア、fsck、
  CLI、HTTP API。その上に、文書の取り込み
  ([docs/design/INGEST.md](#47d69a3e-c39a-4e76-9814-e9c24240293b))、出典付きの検索
  ([docs/design/SEARCH.md](#19574e78-9bf5-4f87-a4c2-c4a10222c580))、LLM エージェント向けの
  MCP アダプタ([docs/design/MCP.md](#dacd474d-424a-45d5-a278-766fc2465dd9))、ブラウザから
  引く 1 枚の RAG ビューワ([docs/design/VIEWER.md](#4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847))、
  ピアへ問いを散布する分散検索
  ([docs/design/DISTRIBUTED_SEARCH.md](#e577f6db-659e-4eb8-a152-3b7780e4a9d1))。
- sim/: レプリカ・健全性モデル(SPEC §8)の離散イベントシミュレータ。

試す:

```
cargo run -p uniqnode -- serve /tmp/uniqnode-data 127.0.0.1:7440
curl -X POST --data-binary '{"v":1,"kind":"node","contents":"hello"}' http://127.0.0.1:7440/v1/objects
curl http://127.0.0.1:7440/v1/status
```

ブラウザから引くには、serve を起こしたままビューワを足す(ストアの錠を取らないので同時に
走る)。`http://127.0.0.1:7450` を開けば、検索・出典・全文が 1 枚の頁で辿れる:

```
cargo run -p uniqnode -- viewer /tmp/uniqnode-data 127.0.0.1:7450
```

知識を分けた 2 台へ 1 本の問いを投げるには、双方の `<データディレクトリ>/peers.json` に
相手のアドレスと DBノードID(`GET /v1/status` の node_id)を書いてから、`peers` を付ける:

```
curl -X POST -d '{"query":"…","peers":true,"budget_ms":3000}' http://127.0.0.1:7440/v1/search
```

serve・mcp・viewer のログは、何も指定しなくても `<データディレクトリ>/logs/` に残る(上の例
なら `/tmp/uniqnode-data/logs/serve.log`)。標準エラーにも同じ行が出る。行の形・回転・保存先の
変え方は [docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004)。

## 運用

常駐は systemd に任せる。serve と viewer の unit の例と、起動・停止・更新・二重起動の手順は
[docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2)。

バックアップは `uniqnode backup <dir> <写し先>` で取る。serve を止めずに取れ(錠を取らない)、
増分で(写し済みの封印済みセグメントは写さない)、写した後に写し先をストアとして開いて fsck
まで通す。写さないのは `derived/`・`logs/`・`tmp/`(作り直せる導出データ・運用ログ・作業場)。
毎日取るなら SYSTEMD.md の timer の unit を使う。復元は、写しをそのまま
`uniqnode serve <写し先> …` に渡すか、空のディレクトリへ逆向きに
`uniqnode backup <写し先> <新しい dir>` してから `uniqnode fsck` で確かめる。何を写す根拠と、
写しが壊れていたときの直し方は [docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2)。

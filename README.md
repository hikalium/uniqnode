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

ブラウザから引くには、serve を起こしたままビューワを足す(ストアのロックを取らないので同時に
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

常駐は systemd に任せる。自分のアカウントで動かすなら `uniqnode install <dir>` の 1 命令で、
serve・viewer・毎日の backup が user 単位の unit として載り、命令は serve と viewer が同じ
node_id を返し backup の写しが fsck で緑であることを見てから戻る(再実行は更新)。unit の
中身と、起動・停止・更新・二重起動の手順は
[docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2)。

バックアップは `uniqnode backup <dir> <写し先>` で取る。serve を止めずに取れ(ロックを取らない)、
増分で(写し済みの封印済みセグメントは写さない)、写した後に写し先をストアとして開いて fsck
まで通す。写さないのは `derived/`・`logs/`・`tmp/`(作り直せる導出データ・運用ログ・作業場)。
毎日取るなら SYSTEMD.md の timer の unit を使う。復元は、写しをそのまま
`uniqnode serve <写し先> …` に渡すか、空のディレクトリへ逆向きに
`uniqnode backup <写し先> <新しい dir>` してから `uniqnode fsck` で確かめる。何を写す根拠と、
写しが壊れていたときの直し方は [docs/mop/BACKUP.md](#e026a5e7-1ece-4f4e-b6b8-ee96c62883a2)。

`uniqnode gc <dir>` で、どの ref・pin・保持表明からも辿れなくなったオブジェクト(孤児)を
含む封印済み pack を、生きているものだけを写した新しい pack で置き換え、孤児のバイト列を
ディスクから取り戻せる。`--dry-run` は数えるだけ(pack ごとの孤児のバイト数と、回収すれば
何バイト戻るか)で、ストアのデータには何も書かない。ロックを取るので serve を止めて(または
写しに対して)打ち、走っている serve には `POST /v1/admin/gc` を打つ(ロックを持つのは短い
3 つの相だけで、その間も他の要求は答える)。定義・手順・出力の読み方は
[docs/design/GC.md](#9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)。消す判断(機会層の evict)は
未実装([docs/plan/PACK_GC.md](#f272eeda-8664-42da-9e5c-ef354bc3f3a7))。

# uniqnode

分散マルチノードのグラフ知識データベース。

各データベースノード(DBノード)は content-addressed で不変なオブジェクト(グラフの頂点と辺)を保持し、
手動設定された暗号化リンクで相互接続される。クエリはネットワークに問いかけられ、答えを持つDBノードが
いれば返事が来る。誰も答えなければ、いつまでも何も来ない。「見つからない」は「存在しない」を意味しない
(開世界仮説)。

- 仕様: [SPEC.md](SPEC.md)
- 計画: docs/plan/
- 検証・分析: docs/analysis/ と sim/(プロトコルシミュレータ)
- プロジェクトポリシー: [policy/](policy/README.md)(RFC 2119 レベル別)

## 状態

適合レベル L0(単一DBノードのストア。SPEC §11)を実装済み。依存クレートなし。

- node/: 実装本体。SHA-256/512、Ed25519(RFC 8032、openssl と相互検証済み)、
  正規化 JSON(c1)、pack セグメント+MANIFEST+署名付き reflog のストア、fsck、
  CLI(`uniqnode init|status|put|get|set-ref|refs|fsck|serve`)、HTTP API。
- sim/: レプリカ・健全性モデル(SPEC §8)の離散イベントシミュレータ。

試す:

```
cargo run -p uniqnode -- serve /tmp/uniqnode-data 127.0.0.1:7440
curl -X POST --data-binary '{"v":1,"kind":"node","contents":"hello"}' http://127.0.0.1:7440/v1/objects
curl http://127.0.0.1:7440/v1/status
```

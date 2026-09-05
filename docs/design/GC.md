# GC — 生きているオブジェクトの定義と、`gc --dry-run` が数えるもの

<a id="9b1ceac3-f3cf-4595-87cb-6e40ce0900e5"></a>

ストアは追記専用で、封印された pack は二度と変わらない。ref を上書きしても tombstone に
しても、指していたオブジェクトのバイト列は pack に残る。この文書は、そのうち何を「生きて
いる」と見なし、何を「孤児」と見なすかの定義と、その定義で pack ごとの孤児量を数える
`uniqnode gc <dir> --dry-run` の読み方を書く。孤児を実際に回収する(pack を書き直す)手順は
まだ無く、その設計は [docs/plan/PACK_GC.md](#f272eeda-8664-42da-9e5c-ef354bc3f3a7) にある。
実装は node/src/gc.rs で、根の集合の組み立てと pack ごとの集計だけを持つ。

## 生きているオブジェクトの定義

到達性で決める。根は次の和集合である。

- 全署名者の ref(自分のも他ノードのも)のうち target が null でないものの target
- 全 pin の root(min_replicas=0 で解除された pin は残っていないので、pin 表の鍵がそのまま)
- held=true の保持表明が 1 つでも残っている root(held=false しか残っていない root は
  含めない)

根から c1 の参照規約(SPEC §4.3: 値の中の `s256:` + 16 進 64 桁の文字列)で辿れるものが
生きている。それ以外は孤児である。ローカルに無い参照先は無視する(開世界)。辿るのは
`Store::reachable_closure_of_roots` で、API の閉包の答え(`Store::reachable_closure`)と
同じ 1 つの実装である(should/0135)。根の側は `Store::list_refs`・`effective_pins`・
`held_attested_roots` から組む。

他ノードの ref が指す先を根に含めるのは、自分がその複製を機会層として持っている意味が
そこにあるからで、外せるのは evict(PACK_GC.md の段階 2)だけである。

孤児が生まれる経路は 3 つ: 同じ ref パスへの上書き(再取り込み)、tombstone、put_object の
後に set_ref に至らなかった残骸。どれも「昔は生きていた」か「一度も名を持たなかった」もの
なので、回収してよいことに疑いはない。逆に、同じ内容の再取り込みは孤児を作らない
(content-addressed なので同じバイト列は同じ ID に落ち、put_object はべき等)。

## `uniqnode gc <dir> --dry-run [--threshold <割合>]`

在るストアだけを開く(空のディレクトリや存在しない道は初期化せず 1 で断る。fsck と同じ
道)。開くので錠を取り、serve が開いているストアには「別プロセスが開いている」と言って 1 で
終わる。backup と違って隣では走れない。

何も書かない。MANIFEST・pack・reflog・tmp のどれにも触れず、終了コードは 0。node/tests/gc.rs
が実プロセスで前後の中身と mtime を突き合わせている。

`--dry-run` を付けないと、回収は未実装であることを言って 2 で終わる。黙って dry-run 扱い
にはしない。

出力は pack ごとに 1 行、最後に集計 2 行。

```
pack 000001 sealed: objects 51668 bytes 269811924, live 269811526, garbage 398 (0.0%) -> keep
pack 000002 active: objects 3765 bytes 107021096, live 107021096, garbage 0 (0.0%) -> keep
gc: packs 2 (sealed 1, compact 0), objects 55433 live 55432 garbage 1, garbage bytes 398 (compact would reclaim 0), roots 202
gc: live set computed in 15157 ms (threshold 0.25, dry-run: nothing written)
```

- `sealed` / `active`: MANIFEST が封印済みと言う pack か、追記中の pack か。封印済みの全部と
  追記中の 1 本が、オブジェクトが無くても行になる。
- `objects`・`bytes`: その pack に索引が置いているオブジェクトの数とペイロードの合計。
  `used_bytes` と同じ会計で、レコードの 8 バイトの頭と、クラッシュ再送で重複追記された
  2 つ目以降は入らない。だからファイルの大きさより少し小さい。
- `live` / `garbage`: 上の定義で生きているバイト数と、そうでないバイト数。括弧は孤児率
  (garbage / bytes。空の pack は 0)。
- `-> compact` / `-> keep`: 封印済みで、孤児率が閾値を超えていれば対象。境界は含めない
  (ちょうど閾値は keep)ので、閾値 0 は「孤児が 1 バイトでもあれば対象」、閾値 1 は
  どの pack も対象にしない。追記中の pack は孤児率がいくらでも keep(書き手と競合する
  ので回収の単位にしない)。判定は `gc::exceeds_threshold` の 1 箇所にある。
- 集計 1 行目: pack の数(封印済み・対象)、オブジェクトの数(全体・生きている・孤児)、
  孤児のバイト数(追記中の pack の分も含む)と、対象の pack だけを書き直したときに戻る
  バイト数、根の数。
- 集計 2 行目: 根を集めて閉包を辿り終えるまでの時間。回収の手順が錠を持つ長さの実測に
  なる(PACK_GC.md のセルフレビュー 6)。ストアを開く時間(索引の再構築)は含まない。

閾値の既定は 0.25(`gc::DEFAULT_THRESHOLD`)。範囲は 0 以上 1 以下で、外れれば usage で
落ちる。既定に実測の根拠はまだ無い。本番ストアの写しに対する最初の実測は
[docs/analysis/20260905-gc-dry-run.md](#491c8c79-620b-47d4-b914-65279e6a599e)。

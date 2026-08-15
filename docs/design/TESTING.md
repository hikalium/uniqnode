# TESTING — テストの層とカバレッジ計測

<a id="267326f7-e919-48f2-9737-fe0c0daec9d5"></a>

uniqnode のテストは5層ある。すべて `cargo test` で走り、決定論的である(乱数は種付き自前
生成器のみ、時刻への依存なし、待ちは条件ポーリング)。

| 層 | 場所 | 検証するもの |
|---|---|---|
| ユニット | node/src/*.rs の `#[cfg(test)]` | 純関数層: 暗号(RFC 8032・FIPS ベクタ、openssl 相互検証)、c1 正規形、CRC、ストアの回復・取り込みの成功系と拒否系 |
| 統合(API) | node/tests/api.rs, sync.rs | 実プロセスの serve に対し、curl が組み立てる形の生 HTTP/1.1 で往復する(should/0138)。レプリケーションの受け入れ基準(複製が読める・停止に耐える・復旧で収束)を含む |
| クラッシュ | node/tests/crash.rs | 書き込み中の実プロセスを位置を変えて5回 SIGKILL し、毎回の回復と fsck 全件パス |
| モデル(実装) | node/tests/sync_model.rs | 実装そのもの(Store + sync の核)を種付き乱数で駆動: 書き込み・tombstone・意図的 dangling・同期・再起動を無作為に交錯させ、全対同期後の収束・閉包完全性・fsck 健全を検証する |
| モデル(仕様) | sim/ | 仕様 §8(レプリカ・健全性)の離散イベントシミュレーション。実装に先行して規則の安全性を検証した([docs/analysis/20260815-replica-model-simulation.md](#31e38823-b783-4dfe-bc7c-3cd268f5e7b4)) |

敵対系も欠かさない: 改竄レコード(署名不正)、seq の飛び、要求 ID と異なるバイト列を返す
不正ピア、封印済みセグメントの破損、二重オープン。これらは「拒否されること」をテストする。

## 方針(lamalium の COVERAGE.md から輸入)

- カバレッジは計測のみで、ゲート条件(閾値)は設けない。行が通ったことと挙動が検証された
  ことは別物であり、数値目標化は薄いテストを誘発する。低下時に差分を見る運用にとどめる。
- 新しいテストは挙動を検査し、失敗時に期待と実際を語る(should/0125)。バグ修正には欠陥を
  復元して赤を確認したテストを添える(should/0137)。

## 計測手順

```
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
cargo llvm-cov --summary-only     # テキストサマリ
cargo llvm-cov --html             # target/llvm-cov/html/index.html
```

計測値は節目ごとに docs/analysis/ に記録する(このファイルには写さない — 古びるため)。

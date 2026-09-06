# LAMALIUM — lamalium のエージェントの記憶装置として uniqnode をつなぐ計画

<a id="68571059-94ed-4aa2-8ae0-b2862d1de44e"></a>

uniqnode の一番の利用者は lamalium(同じ機械で動く階層型のマルチエージェント系。
/work2/llm_playground_host_dir/lamalium)になる予定である。この文書は、両者をつなぐ計画を、
lamalium 側の現物を読んで立てたものである。lamalium 側の変更は lamalium の docs/plan/ に
同名の文書として置き(must/0013 は向こうも同じ)、ここでは uniqnode 側の作業と、両側に
またがる判断を書く。読み手は、両方のリポジトリを触る者。

## lamalium の現物(2026-09-06、lamalium ca1fdca)

計画の根拠になる事実だけを挙げる。行番号は当日のもの。

- エージェントは nspawn コンテナの中の systemd unit(`lamalium-agent@<id>.service`)で、
  ReAct の 1 歩 1 ツールで動く。脳は ollama / llama.cpp(gemma4:31b が標準。
  roles/worker/role.toml)。
- 網: エージェントの unit は `IPAddressAllow=localhost` と `IPAddressAllow=10.100.0.1`
  (container/lamalium-agent@.service:29-31)だけ。10.100.0.1 はホストの veth のアドレスで、
  ollama はそこで待つ。遠い機械の脳は `lamalium-backend-proxy.service`(host.rs:732-)が
  `10.100.0.1:<port>` → `<ip>:<port>` の素朴な TCP 転送で橋を架ける。設定は
  /var/lib/lamalium/etc/backend-proxy.toml で、足すのは sudo 不要。
- brokered `run`(エージェントが打つコマンド)は `PrivateNetwork=yes` で完全にオフライン
  (DESIGN §A10.5)。網に出るのは agentd のハーネス自身(ツールの実装)だけである。
- ツール: `tool::Tool` の登録(toolset.rs `tool_info`)と `execute_tool` の振り分け。読むだけの
  ツールは allowed_tools のゲートの外で常時使える(`knowledge`・`lookup_agent_policy` が先例。
  knowledge.rs:784 `tool_knowledge(args) -> (String, bool)`)。HTTP は ureq(平文のみ、TLS 無し。
  container/Cargo.toml:29)。
- 既存の知識: `knowledge` ツールは RFC の全文をホストが取って RO で bind した `/lamalium/knowledge`
  を grep する(docs/design/KNOWLEDGE.md)。「RFC の要約・埋め込み・ベクトル検索はやらない」と
  明記されている(§8)。理由は「全文 grep + 逐語窓で十分」で、規範文書に対しては正しい。
- 記憶の穴: docs/plan/MEMORY-RECALL.md。セッションはほぼ白紙で始まり、跨いで残るのは失敗の
  lessons だけ。成功例(red→green の修理手順)は残らず、過去 step をセッション横断で検索する
  手段が無い。同文書の重要な実測: 「モデルが自発的に検索を発行する見込みは薄い(拒否文に
  正候補一覧が毎回あっても 374 回反復)。持ち越しが効いた実績はすべてハーネスの push 型」。
- 予算の規律: プロンプトに載せる注入は「条件付きで 1 行だけ」(docs/plan/KNOWLEDGE.md の K4、
  DESIGN §A12.3)。KV キャッシュの接頭辞を壊さない位置に置く(CONTEXT-ECONOMY)。
- リポジトリの規律は uniqnode と同じ系統(policy/ は同じ番号体系、must/0008 Rust のみ、
  must/0009 シェルスクリプト禁止、must/0013 design/plan の分離)。

## 何を提供するか(役割の分担)

uniqnode が lamalium に提供するのは「出典付きで引ける記憶」であり、2 つの使い方がある。

1. 引く(pull): エージェントが問いを投げ、出典付きの断片を得る。lamalium の `knowledge` の
   隣に `recall` ツールを置く形。対象は RFC の grep では届かないもの ― 仕様書 PDF(uniqnode の
   本番コーパス)、lamalium 自身の設計文書とポリシー、過去のタスクの成功手順、ウェブの保存。
2. 渡す(push): 束縛時にハーネスが uniqnode を検索し、タスクの goal に近い知識を 1〜3 行の
   出典付きで注入する。MEMORY-RECALL の実測が言うとおり、効くのはこちらである。pull は
   従で、注入された出典の続きを読むために使う。

書くのは lamalium のエージェントではない。ホストが lamalium の木(git 管理の docs/・policy/・
memory/、タスクの lessons と finish 要約)を取り込み、蒸留された成功手順(MEMORY-RECALL の
作業項目 3。ハーネスが検証器の緑遷移から機械的に作る)を文書として入れる。エージェントに
書かせない理由は lamalium の側にある(A12.4: ハーネスは内容を知らない。0700 の私有記憶は
読まない)。

## 網とロックの形

uniqnode の serve は 127.0.0.1:7440 で待ち、HTTP API に認証は無い(同じ機械の利用者だけ、
という前提)。コンテナへそのまま見せると、エージェントの unit からは届かなくても、`net:` の
grant を得た exec や、将来のツールの誤りで `POST /v1/admin/shutdown`・`PUT documents`・
`POST fetch`(網に出る口。lamalium のオフライン不変条件に触れる)まで届く。

だから uniqnode に「読むだけの口」を足す: `serve … --listen-readonly 10.100.0.1:7441`。
2 本目の listener で、通すのは `POST /v1/search`・`GET /v1/objects/{id}`・
`GET /v1/objects/{id}/rendition[/…]`・`GET /v1/objects/{id}/referrers`・`GET /v1/status` だけ。
それ以外は 404 ではなく 403 で「読むだけの口」と言う。backend-proxy を介す必要は無い
(10.100.0.1 はホスト自身のアドレスなので、serve が直接束縛できる)が、介しても同じ。
`uniqnode install` に `--readonly-listen` を足して drop-in に載せる。

ロックの規律は変わらない。読むだけの口も同じ `Mutex<Store>` を使い、検索の間だけ持つ。

## lamalium 側の作業(向こうの docs/plan/UNIQNODE.md に置く内容の要約)

1. `recall` ツール(read-only、常時利用可。knowledge と同格): `{q}` → uniqnode の
   `POST /v1/search` を `http://10.100.0.1:7441` へ。応答は knowledge_search と同じ規律で
   整形(上限 ~20 件・2400 字、行幅の切り詰め): 1 件 = `document p.N > 見出し` の出典行 +
   snippet + `id`。`{id}` → `GET /v1/objects/{id}` の chunk 本文(逐語窓)。`{collection}` で
   絞れる。uniqnode が落ちていれば「recall: uniqnode に届かない(<url>)」と言って空で返す
   (fail-open。タスクを止めない)。URL は role.toml の `ollama_url` と同じ場所に
   `uniqnode_url`(既定は空 = ツールを載せない)。
2. 束縛時の注入(push): タスクの goal(と gate-fail 文)で 1 回検索し、上位 3 件を
   「related: <document p.N> — <snippet 1 行> (recall id=…)」の形で 1〜3 行、K4 の予算規律の
   位置に置く。効果は adherence と、注入された id の `recall` 呼び出し率で測る。
3. テスト: uniqnode の実物には依存しない。lamalium のテストは固定の JSON を返す小さな
   HTTP サーバ(向こうの http.rs)で `recall` の整形と fail-open を固定する。結合は
   `lamalium host systest` ではなく手順書(向こうの docs/design/DEPLOY の流儀)で 1 回確かめる。
4. policy: 新しい MUST は作らない。向こうの policy 0113(仕様を参照してから実装せよ。uniqnode には無い番号)の「手段」に
   recall を足す 1 文だけ。

## uniqnode 側の作業(実行順)

1. 読むだけの口 `--listen-readonly <addr>`(上の「網とロックの形」)。統合テスト: 読む口で
   PUT・fetch・admin が 403、search と objects が通る。install の `--readonly-listen`。
2. `uniqnode ingest … --serve-url <url>`: serve を止めずにディレクトリを取り込む転送形
   (mcp と同じ形。今の CLI ingest はロックのため serve 停止中しか使えない)。ホストの timer
   `uniqnode-ingest-lamalium.timer` がこれで lamalium の git 管理の文書
   (DESIGN.md・docs/design・docs/plan・policy・memory)をコレクション `lamalium` に入れる
   (同じ内容は同じ ID なので毎日走らせても増えない。変わった文書だけ新しい doc_rev に
   なり、旧チャンクは gc が回収する)。
3. 小さな脳向けの応答: gemma4 級が読む観測は短くなければならない。`POST /v1/search` に
   `snippet_chars`(既定のまま)と `top_k` があれば足りるか、lamalium 側の整形で済むかを
   実測で決める。uniqnode 側に「1 件 1 行」の形を足すのは、向こうの整形で足りないと
   分かってから。
4. 評価: lamalium の実タスク(pianoapp の HTTP 応答修正のような、仕様を引けば直る類)で、
   注入あり/なしの緑到達率を向こうの adherence で比べる。uniqnode 側は、lamalium の問いの
   集合を EVAL の対に足して Recall@k を固定する。

順序の根拠: 1 が無いと何も届かず、2 が無いと引ける知識が仕様書 PDF だけになる。3・4 は
測ってから。

## 段階(両側を合わせた道順)

| 段階 | 内容 | 完了条件 |
|---|---|---|
| L0 今すぐ | lamalium の文書を uniqnode の本番に入れ(手で `uniqnode fetch`/ingest)、Claude Code の MCP から引いて、どんな問いに何が返るかを見る | 10 問の問いと答えの記録が docs/analysis/ にある |
| L1 | uniqnode の読むだけの口 + lamalium の `recall` ツール | コンテナのエージェントが `recall` で仕様書の断片を出典付きで得る(向こうのログで観測) |
| L2 | 束縛時の注入(push) | 注入行がプロンプトに載り、adherence に悪化が無い |
| L3 | ホストの timer による lamalium の木の取り込み + 成功手順の蒸留文書 | 過去タスクの成功手順が別のタスクの注入に現れる |
| L4 | 評価と調整(小さな脳向けの整形、EVAL の対) | 注入あり/なしの比較が数字である |

## 判断が要ること(利用者に聞く)

- 読むだけの口を 10.100.0.1 に直接束縛するか、backend-proxy を介すか。直接のほうが部品が
  少ない。proxy を介すと lamalium 側の一覧(backend-proxy.toml)に uniqnode が並び、
  管理が一か所になる。どちらも sudo は不要。
- コレクションの分け方: 仕様書 PDF(`specs`)、lamalium の文書(`lamalium`)、成功手順
  (`lamalium/recipes`)、ウェブの保存(`web`)。`recall` の既定は全部か、タスクの種類で
  絞るか。
- L3 でエージェントの私有記憶(0700)は決して読まない。読むのは git 管理の木と、ハーネスが
  作る蒸留文書だけ。この線でよいか。

## やらないこと

- コンテナから uniqnode へ書く口。書き込みは常にホストから。
- `fetch_url` をコンテナへ見せること(オフライン不変条件)。
- uniqnode を lamalium のバイナリに埋め込むこと(別プロセスのまま。lamalium は musl の
  静的 1 バイナリで、uniqnode は依存クレート無しの別の木。HTTP でつなぐのが両方の規律に合う)。
- RFC の grep を uniqnode に置き換えること。`knowledge` はそのまま。uniqnode は grep で
  届かないもの(PDF・意味検索・過去の手順)を担う。

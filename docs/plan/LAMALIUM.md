# LAMALIUM — lamalium のエージェントが自発的に引き、書く記憶装置として uniqnode をつなぐ計画

<a id="68571059-94ed-4aa2-8ae0-b2862d1de44e"></a>

uniqnode の一番の利用者は lamalium(同じ機械で動く階層型のマルチエージェント系。
/work2/llm_playground_host_dir/lamalium)になる予定である。この文書は、両者をつなぐ計画を、
lamalium 側の現物を読んで立てたものである。lamalium 側の変更は lamalium の docs/plan/ に
同名の文書として置き(must/0013 は向こうも同じ)、ここでは uniqnode 側の作業と、両側に
またがる判断を書く。読み手は、両方のリポジトリを触る者。

方針(2026-09-06 の裁定): エージェントが自発的にツールを見つけて呼ぶ形を主にする。
「モデルは自発的に検索を発行しない」という向こうの過去の実測(MEMORY-RECALL の 374 回反復)は
特定のモデルとハーネスの組での観測であり、ツールの見せ方・説明文・モデルの選び方で変わる。
だから最初から push(ハーネスによる注入)に逃げず、pull を成立させることを目標に置き、
成立したかどうかを向こうの BENCH で測る。push は測った結果に応じて足す従の手段である。

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
- ツールの作り: `tool::Tool` トレイト(container/src/tool.rs:54。name・describe は
  `&'static str`、`always_listed`・`always_dispatched` で常時ツールになる)を実装して登録し、
  `execute_tool`(tools.rs:13)が振り分ける。エージェントは `tools`・`help` のメタツールで
  自分の使えるツールを見つける(progressive disclosure。docs/design/TOOLS.md)。HTTP は
  ureq(平文のみ、TLS 無し。container/Cargo.toml:29)。MCP のクライアントは無い。
- 先例: `knowledge` ツール(knowledge.rs:784 `tool_knowledge(args) -> (String, bool)`)は
  RFC の全文をホストが取って RO で bind した `/lamalium/knowledge` を grep する常時ツール
  (docs/design/KNOWLEDGE.md)。「RFC の要約・埋め込み・ベクトル検索はやらない」と明記
  (§8)。規範文書に grep で足りる、という判断はそのまま尊重する。
- 記憶の穴: docs/plan/MEMORY-RECALL.md。セッションはほぼ白紙で始まり、跨いで残るのは失敗の
  lessons だけ。成功例は残らず、過去 step をセッション横断で検索する手段が無い。
- 測る道具: BENCH(model × prompt × task の行列と決定論の checker。docs/design/BENCH.md)と
  adherence(指示追従の採点)。ツールが実際に呼ばれたかは events.jsonl から数えられる。
- 最初の利用者に向く席: helpdesk(roles/helpdesk、gpt-oss-120b に pin、OpenAI 互換の本物の
  tools API で駆動。helpdesk.rs:55)。allowed_tools は policy の lookup と `knowledge` の読む
  だけの 4 本で(roles/helpdesk/role.toml:20)、「文書とポリシーだけから出典付きで答える」
  という職能が uniqnode の性質と一致する。`docs` の行動は `tool_knowledge` を直接呼ぶ
  (helpdesk.rs:641)ので、差し替え点が 1 関数に閉じる。
- 常駐の規律: 向こうの policy 0108(uniqnode には無い番号)は、ホストの常駐プロセスを
  user unit ではなく system unit(`User=hikalium`)にせよと言う。ホスト側 3 サービスは
  すべてその形で、rootless podman のために `XDG_RUNTIME_DIR` と linger を組み合わせた先例も
  ある(host/lamalium-device-daemon.service)。uniqnode の本番は user unit で常駐しており、
  この規律と食い違う。
- 明示的な却下: docs/design/KNOWLEDGE.md §8 は「RFC の要約・埋め込み・ベクトル検索は
  やらない」と書き、根拠に「全文 grep + 逐語窓で十分」と「A12.4: ハーネスは内容を知らない」
  を挙げる。uniqnode をつなぐことは、前者には「RFC はそのまま、届かないものを担う」で
  答えられ、後者には「uniqnode は出典(座標)と原文の断片を返すだけで、内容の判断はしない
  (向こうの citation.rs と同型)」で答えられるが、文書の書き換えと操作者の裁定が要る。
- 予算の規律: プロンプトへの注入は「条件付きで 1 行だけ」(docs/plan/KNOWLEDGE.md の K4、
  DESIGN §A12.3)。
- リポジトリの規律は uniqnode と同じ系統(policy/ は同じ番号体系、must/0008 Rust のみ、
  must/0009 シェルスクリプト禁止、must/0013 design/plan の分離)。

## 何を提供するか

uniqnode は lamalium のエージェントにとって「出典付きで引ける記憶」であり、引くのも書くのも
エージェント自身である。

- 引く: 仕様書 PDF(uniqnode の本番コーパス)、lamalium 自身の設計文書とポリシー、過去の
  タスクの手順や決めごと、ウェブの保存。RFC の grep で届くものは `knowledge` のままでよい。
- 書く: 「覚えておいて」に当たるもの ― タスクの中で確かめた事実、効いた手順、決めごと。
  エージェントは uniqnode の `add_document` と同じ意味の口で、許されたコレクションにだけ
  書く。何を書いたかは uniqnode の側に署名付きの記録(doc_rev、出所の meta)として残り、
  消したければ tombstone → gc で消せる。

## 接続の形: どこで何がつながるか

```
コンテナ                                    ホスト
agentd(ハーネス) ── HTTP ──▶ 10.100.0.1:7441 ── uniqnode serve(読む口 + 許した書く口)
   │  recall / remember の 2 ツール                 │
   │  (tool::Tool として登録)                       └─ 127.0.0.1:7440(運用の口。admin・fetch)
   └─ tools / help で発見
```

- uniqnode の serve に 2 本目の listener `--listen-agent 10.100.0.1:7441` を足す。通すのは
  `POST /v1/search`・`GET /v1/objects/{id}`・`GET /v1/objects/{id}/rendition[/…]`・
  `GET /v1/objects/{id}/referrers`・`GET /v1/status` と、`--agent-writable <コレクション>`
  で許したコレクションへの `PUT /v1/collections/{c}/documents/{name}` だけ。それ以外
  (`admin/*`・`sync`・`pins`・`collections/{c}/fetch`)は 403 で「エージェントの口」と言う。
  `fetch` を通さないのは、コンテナが網に出る道になるからで(lamalium のオフライン不変条件)、
  URL の取り込みはホストの操作者の仕事のまま。
- lamalium 側は `recall`(読む)と `remember`(書く)の 2 ツールを `tool::Tool` として足す。
  MCP のクライアントは作らない(向こうのツール登録は静的で、MCP の動的な tools/list を
  受ける形になっていない。uniqnode の MCP アダプタの description は模型向けに書いてあるので、
  向こうのツールの describe に同じ文を写す。文言の出所は uniqnode の mcp.rs とし、写しが
  ずれたら直す)。
- 認証は無い(同じ機械の中、経路はハーネスだけ)。`net:10.100.0.1` の grant を得た exec も
  同じ口に届くが、届く先は上の allowlist に閉じている。

## 自発的に呼ばれるための作り(ここが本体)

ツールを置くだけでは呼ばれない、という向こうの観測は事実として受け取り、呼ばれる条件を
作って測る。

1. 発見: `recall` は `knowledge` と同じ常時ツール(`always_listed`)にする。`tools` の一覧に
   出て、`help` で例が読める。`remember` はロールの allowed_tools に載せる(書く口は
   ロールの判断)。
2. 説明文: 「いつ使うか・使わないか」を 1 行ずつ(uniqnode の MCP の description と同文)。
   例: recall は「仕様・設計・過去の決定を確かめたいとき。ファイルの中身を探すのは grep」。
   remember は「利用者や親タスクから『覚えておいて』と言われた事実・効いた手順」。
3. 呼び出しの費用: 応答は短く決定論的に。1 件 = 出典行(document p.N > 見出し)+ 1 行の
   snippet + id、上限 5 件。続きは `{id}` で逐語。gemma4 級が 1 歩で読める大きさ
   (knowledge_search の上限 2400 字と同じ規律)。
4. 測定: BENCH に「引けば直る」タスクを足す(先例: pianoapp の HTTP 応答が RFC 9112 違反。
   同型で、uniqnode にしか無い知識 ― 仕様書 PDF の 1 節、過去の決めごと ― が要るタスク)。
   計るのは (a) recall が呼ばれた率、(b) 呼ばれたとき緑に至った率、(c) 呼ばれなかったとき
   の率。モデル(gemma4:31b・gpt-oss-120b・Qwen3)× 説明文の版で行列にする。
5. 調整: (a) が低ければ説明文と一覧での位置を変えて測り直す。それでも低いモデルにだけ、
   K4 の規律で 1 行の tip(「spec is one recall away: {…}」)を条件付きで出す。ハーネスの
   注入(束縛時に検索結果を置く push)は、tip でも上がらないと分かってからの最後の手段。

## uniqnode 側の作業(実行順)

1. エージェントの口 `--listen-agent <addr>` と `--agent-writable <コレクション>`(繰り返し可)。
   統合テスト: その口で admin・sync・fetch が 403、search・objects が通る、許したコレクション
   だけ PUT が通る。`uniqnode install` に同じ 2 つの指定を足して drop-in に載せる。
2. 応答の短い形: `POST /v1/search` に `format: "lines"`(1 件 1 行の出典 + snippet + id)を
   足すか、lamalium 側で整形するかを決める。判断の住み処を 1 つにするなら uniqnode 側
   (MCP の search も同じ整形を使う。should/0135)。
3. `uniqnode ingest … --serve-url <url>`: serve を止めずにディレクトリを取り込む転送形。
   ホストの timer で lamalium の git 管理の文書(DESIGN.md・docs/design・docs/plan・policy・
   memory)をコレクション `lamalium` に入れる(同じ内容は同じ ID。変わった文書だけ新しい
   doc_rev になり、旧チャンクは gc が回収する)。
4. 出所の記録: `add_document` 相当の PUT に `meta`(誰が: agent id・task id)を受け付け、
   doc_rev.meta に写す(取り込みの extra_meta は既にあるので、API に欄を足すだけ)。
5. 評価: BENCH の「引けば直る」タスクの問いを EVAL の対に足し、Recall@k を固定する。

## lamalium 側の作業(向こうの docs/plan/UNIQNODE.md に置く内容の要約)

1. `recall`(常時ツール)と `remember`(ロールの allowed_tools)を `tool::Tool` で。HTTP は
   ureq で `http://10.100.0.1:7441`。URL は role.toml の `ollama_url` の隣に `uniqnode_url`
   (既定は空 = ツールを載せない)。uniqnode に届かなければ「recall: uniqnode に届かない」
   と言って空で返す(fail-open。タスクを止めない)。
2. 説明文は uniqnode の mcp.rs の description と同文。写しの出所をコメントで指す。
3. テスト: 固定の JSON を返す小さな HTTP サーバで整形と fail-open を固定する(uniqnode の
   実物には依存しない)。
4. BENCH のタスクと checker(上の「測定」)。
5. policy: 新しい MUST は作らない。向こうの policy 0113(仕様を参照してから実装せよ。
   uniqnode には無い番号)の「手段」に recall を足す 1 文だけ。

## 段階

| 段階 | 内容 | 完了条件 |
|---|---|---|
| L0 | lamalium の文書を uniqnode の本番に入れ、Claude Code の MCP から引いて、どんな問いに何が返るかを 10 問記録 | docs/analysis/ に記録がある |
| L1 | uniqnode のエージェントの口 + lamalium の recall/remember。最初の席は helpdesk(本物の tools API を持つ gpt-oss-120b。読むだけ)、次に pool の worker | helpdesk が recall で仕様書と設計文書の断片を出典付きで答え、worker が remember で notes に書けた(向こうのログで観測) |
| L2 | BENCH で「引けば直る」タスクを回し、呼ばれた率と緑到達率を測る | モデル × 説明文の行列に数字がある |
| L3 | 数字に応じた調整(説明文・位置・tip)。それでも足りないモデルにだけ push | 調整前後の差が数字である |
| L4 | ホスト timer による lamalium の木の取り込み + エージェントが書いた記憶の運用(コレクションの整理、gc) | 過去タスクで remember した事実が別のタスクの recall に出る |

## 判断が要ること(利用者に聞く)

- 向こうの KNOWLEDGE.md §8 の却下(埋め込み・ベクトル検索はやらない)を、上の答え方で
  書き換えてよいか。これは lamalium 側の設計判断の変更なので、操作者の裁定が要る。
- uniqnode の常駐を向こうの規律(system unit、`User=hikalium`)に合わせて移すか。移すなら
  `uniqnode install --system`(今は未実装。docs/mop/systemd/system/ の unit は手順書のみ)を
  先に作る。user unit のままでもつながるが、lamalium の側から見ると規律の例外になる。

- エージェントに書かせるコレクションの粒度: 全エージェント共有の `lamalium-notes` 1 つか、
  タスクごと(`tasks/<id>`)か、エージェントごとか。共有 1 つが単純で、出所は meta に残る。
- エージェントの口を 10.100.0.1 に直接束縛するか、backend-proxy を介すか(直接のほうが
  部品が少ない。proxy なら向こうの一覧で一元管理)。
- BENCH に載せるモデルの集合(gemma4:31b は必須。gpt-oss-120b と Qwen3 を足すか)。

## やらないこと

- `fetch_url` をコンテナへ見せること(オフライン不変条件。URL の取り込みは操作者)。
- admin・sync・pins をコンテナへ見せること。
- lamalium に MCP クライアントを作ること(今回は 2 ツールを直に登録する。MCP サーバが
  複数になったら向こうで改めて考える)。
- uniqnode を lamalium のバイナリに埋め込むこと(別プロセスのまま。HTTP でつなぐ)。
- RFC の grep を uniqnode に置き換えること。`knowledge` はそのまま。

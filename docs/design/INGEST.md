# INGEST — 取り込み層(文書モデル・チャンキング・PDF・HTML・URL・注釈・訂正)

<a id="47d69a3e-c39a-4e76-9814-e9c24240293b"></a>

読み手は、取り込み層のコード(node/src/ingest.rs と、その CLI・API の口)を読む者と、この層の
上に検索・埋め込み・LLM の層を足す者。この文書は取り込み層の現在の実装を記述する。個々の
決定が従う論理設計(言明・検証・訂正の原理)は
[docs/design/ASSERTIONS.md](#c05379e2-2d30-41bc-8342-62103d94bb21) が持ち、実データ投入の
測定は [docs/analysis/20260816-ingest-acceptance.md](#0be862ff-9448-40ab-aa6b-31c796f6482e)
に記録がある。コアの kind(node/edge/blob)と到達閉包の規約(c1 値中の s256: 文字列を参照と
見なす。SPEC §4.3)の上に載る上位層であり、コア・プロトコル・ストレージ形式には手を入れて
いない。

## 文書モデル

blob(原文そのもの)・chunk(検索と引用の単位)・doc_rev(版)・ref(現在の見え)の 4 層。

```jsonc
// chunk: 引用に必要な文脈を自身で持つ。meta.breadcrumbs は Markdown の見出しの入れ子
// パス、meta.page は PDF の物理ページ番号(1 始まり)。空の鍵は書かず、両方無ければ
// meta ごと省く。
{ "v": 1, "kind": "chunk", "text": "…",
  "meta": { "breadcrumbs": ["5 …", "5.2.3.2 …"] } }

// doc_rev: 原文 blob と順序付きチャンク列を束ねる版。media は markdown | text | html | pdf。
// extractor は PDF のときだけ(PDF 抽出の節)。previous は前版の doc_rev(初版はキー
// なし)。
{ "v": 1, "kind": "doc_rev", "source": "s256:<blob>",
  "chunks": ["s256:…", "s256:…"],
  "meta": { "name": "acpi_6_4", "media": "pdf", "extractor": "pdftotext 22.02.0" },
  "previous": "s256:<前版の doc_rev>" }
```

- 文書名は取り込み起点からの相対パスから拡張子を除いたもの(acpi_6_4.pdf なら acpi_6_4)。
  種別は入力ファイル名の拡張子で判定し(.md / .markdown は markdown、.txt は text、
  .html / .htm は html、.pdf は pdf)、拡張子を ref 名には残さない。
- ref `collections/<コレクション名>/<文書名>` が現行の doc_rev を指す。文書一覧は既存の
  refs 一覧(GET /v1/refs)で足りるので、専用の一覧 API は無い。コレクションの一覧と
  各コレクションの文書数だけは GET /v1/collections が返す(応答
  `{"collections":[{"documents":N,"name":"<c>"}]}`、名前順。数えるのはこの形の名前で
  target が null でない ref で、署名者は見ない。ref 名の読み方は
  search::document_ref_parts の一箇所で、索引の走査と同じ。should/0135)。
- 引用は doc_rev 側から組み立てる: 文書名は ref パスから `collections/<コレクション名>/` を
  除いた残り、文書内位置は chunks 列の添字、見出しは meta.breadcrumbs、PDF はさらに
  meta.page。チャンク単体から出発する逆方向(このチャンクはどの文書のものか)は逆引き
  (逆引きの節)が与える。
- チャンクの順序は doc_rev の chunks 列だけが持つ(順序は辺が持つ、という I7 と同じ形)。
  chunk 自身は番号を持たず、順序という一つの事実を二箇所に置かない。
- doc_rev に時刻は入らない。入れると同一内容の再取り込みが別 ID になり、べき等(I1)が
  壊れる。取り込み時刻は ref レコードの at(可変層・署名付き)が持つ(I2)。例外は URL
  からの取り込みの meta.fetched_at(URL からの取り込みの節)で、同一性は下の (source,
  chunks 列) の一致で判定するので、同じ内容の再取得は時刻が違っても no-op のままである。
- 書き込みの順序は blob と chunk 群 → doc_rev → ref(set_ref は存在しない target を拒否する
  ため ref は最後)。
- 再取り込みの同一性は doc_rev の ID ではなく (source, chunks 列) の一致で判定する。previous
  キーがあるため、ID の比較では同じ入力からも毎回別 ID の doc_rev ができて no-op が壊れる。
  現行 ref の指す doc_rev と一致したら、新しい doc_rev を作らず ref も触らない
  (べき等 = I1 の検証)。
- チャンクは本文を自身で持つ(blob へのオフセット参照ではない)。保存は重複するが、引用と
  スニペット返却に blob の取得と切り出しが要らない。同一本文のチャンクは content-addressing
  が重複排除する。重複の実コストの実測は
  [docs/analysis/20260816-ingest-acceptance.md](#0be862ff-9448-40ab-aa6b-31c796f6482e) にある。

## チャンキング規則

- 大きさは上限 480 の近似トークン数(定数 CHUNK_TOKEN_LIMIT)。根拠は検索スニペットとして
  一度に読める長さで、評価ハーネス
  ([docs/design/EVAL.md](#1109a04b-923e-4493-8f00-d704047d6a2a))で測って調整する暫定値である。
- 近似トークン数は「ASCII 文字は 1/4 トークン、非 ASCII 文字は 1 トークン」の重み付き文字数
  (切り上げ)。純関数 token_estimate として一箇所にあり、チャンカーとテストが共用する
  (should/0135)。注釈の照合で数える「語」(注釈の取り込みと照合の節)とは別物である。
- どの形式も、空行区切りの段落を上限まで詰めてチャンクにする。単一の段落が上限を超えるとき
  だけ文字境界で分割する。コードフェンスの内側では空行でも段落を切らず、フェンスの行も本文の
  一部として保つ。
- Markdown: 見出し境界を優先する(見出しごとに詰め直す)。見出しの入れ子パスを各チャンクの
  meta.breadcrumbs に写す。フェンス内の見出し風の行は見出しとして扱わない。
- PDF: pdftotext の form feed をページ境界として保ち、ページをまたぐチャンクを作らない
  (引用のページ番号を一意にするため)。form feed は最終ページの後ろにも付くので、末尾の
  空区画は捨てる(捨てないと末尾に空ページが生まれ、ページ番号が 1 つずれる)。途中の
  空ページはチャンクを生まず、番号だけ進める。
- 正確なトークナイザは使わない。依存ゼロ方針と衝突するうえ、チャンク境界の質は評価ハーネスで
  測って調整するほうが確実である。

## PDF 抽出

- テキスト抽出は pdftotext(poppler)への外部プロセス委譲。ライブラリ依存はゼロのまま
  外部プロセスは可という枠で、既存の先例は openssl 相互検証テスト(node/src/ed25519.rs)。
  シェルは経由せず Command で直接起動する(must/0009 と同じ理由)。
- 実行ファイルは引数 --pdftotext <path> の明示指定を優先し、無指定なら PATH を引く。
  見つからない・起動できないときは、必要なバイナリ名と sudo なしの導入手順を示して明示的に
  失敗する。黙って PDF を飛ばさない(must/0022 の同型)。API では 503 を返す(委譲先が
  無いのは一時的な状態であって要求の誤りではない)。
- doc_rev.meta.extractor に「pdftotext <版>」を記録する。版は pdftotext -v の出力の先頭行
  「pdftotext version <版>」から完全な形で取る(例: pdftotext 22.02.0)。poppler の版が
  変わると抽出テキストが揺れうるが、blob は PDF そのものなので、再抽出の差は新しい doc_rev
  になるだけで、旧 doc_rev は旧抽出についての真のまま残る(I1)。
- blob になるのは PDF バイナリそのもので、チャンクは抽出テキスト(form feed 区切り)から
  作る。

## HTML 抽出

- blob になるのは HTML の原本そのもので、チャンクは札を落とした本文から作る(PDF と同じ
  分け方)。抽出は自前で、家は node/src/html.rs である。pdftotext のような「どこにでもある
  委譲先」が HTML には無い(lynx も pandoc も前提にできない)ので、外部プロセスを増やさずに
  済む範囲だけを引き受ける。
- 引き受けるのは 3 つ。人が読まない部分(script・style・svg・noscript・template の中身と
  コメント)を落とす。見出しと箇条書きを Markdown の記法へ写す。実体参照を文字へ戻す。
  写したあとは Markdown のチャンカーをそのまま通すので、見出しの入れ子が meta.breadcrumbs に
  なる。title 要素は紙面の題として最上位の見出しに写る。
- 落ちるものも決めてある: リンク先の URL、画像(alt も)、表の罫線と列の対応、装飾。表は
  セルを空白 1 つ、行を改行に落とすだけである。落ちたものが要るときは blob を読む
  (原本は無傷で残っている)。
- 見出しの階を飛ばした紙面(h1 の次が h3 で、その h3 が何本も並ぶ形)は HTML では普通に
  ある。チャンカーは見出しを階数つきで積み、同じ階かそれより浅い見出しが来たら積みを
  戻すので、並んだ h3 は兄弟のまま経路に載る。
- ナビゲーションや脚のような、どの紙面にも出る定型は本文に入る。「ここからが本文」を当てる
  規則は生成器ごとに違うので当てない。定型は短く、チャンクの大半は本文になる。
- doc_rev.meta.extractor は付かない。外部へ委譲していないので、記録すべき外部の版が無い
  (抽出の変化はこの木の版が持つ)。
- 原本を返す道は写しの恒等レシピ(source)で、Content-Type は中身の頭で決まる
  ([docs/design/RENDITION.md](#6046eeca-1d95-4d47-87da-13f86c7710dc))。HTML と名乗れる原本は
  text/html で出るので、ブラウザでそのまま開ける。網に取りに行かない紙面を入れておけば、
  ストアの中の 1 枚がそのまま読める紙面になる。

## URL からの取り込み

URL を渡すと、ノード自身が取りに行って上の道(HTML・PDF・素文)へ流す。家は
node/src/fetch.rs で、取る・見分ける・名前を決める・出所を組むまでを持ち、書き込みは
ファイルからの取り込みと同じ ingest_document を通る(should/0135)。

- 取りに行く道具は curl への外部プロセス委譲である。依存クレートを持たないこの木では TLS を
  自前で話せないので、pdftotext と同じ枠(Command で直接起動、シェルは経由しない。
  must/0009)で頼む。渡す指定は `--location --max-redirs 10 --proto =http,https
  --proto-redir =http,https --max-time 30 --max-filesize 67108864 --user-agent uniqnode/<版>`
  で、本文は一時ファイルへ落とし、`--write-out` の 3 行(content_type・url_effective・
  http_code)を標準出力から読む。入力側でも http と https 以外のスキーム(file: や ftp:)は
  curl に渡す前に断る。HTTP の状態が 2xx でなければ理由(状態と転送後の URL)を言って
  失敗し、404 の紙面を黙って取り込まない(must/0022)。curl が無ければ導入手順を示して
  失敗する。
- curl の版は最初の呼び出しで一度だけ `curl --version` の先頭行から取り(PDF 抽出の
  pdftotext と同型)、doc_rev.meta.fetcher に「curl <版>」の形で書く。
- 種別は相手の名乗り(Content-Type の主要部)に従う: text/html と application/xhtml+xml は
  html、application/pdf は pdf、text/plain は text、text/markdown は markdown。ただし中身が
  `%PDF-` で始まれば名乗りによらず pdf(名乗りは嘘をつくが魔法数字はつかない)。名乗りが
  無い・application/octet-stream のときだけ中身の頭で決め、その見方は写しの恒等レシピと同じ
  1 箇所(node/src/rendition.rs の identity_content_type。
  [docs/design/RENDITION.md](#6046eeca-1d95-4d47-87da-13f86c7710dc))である。画像などそれ以外は
  「取り込める種別ではない」と断る(黙って素文にしない)。text/plain で配られる Markdown は
  text として入る(名乗りが正しいと信じる)。
- HTML は blob にする前に自足した 1 枚に書き換える(node/src/web.rs の self_contain。
  スクリプト・外部スタイル・画像・フレーム・フォントなど外部への依存を落とし、相対リンクを
  転送後の URL を基準に絶対にし、出所の meta を入れる)。blob になるのは書き換えた後の
  紙面で、チャンクはそこから HTML 抽出の節の道で作る。`<html>` も DOCTYPE も無い断片は
  書き換えの出力が `<head>` で始まり、原本の Content-Type 判定(RENDITION.md。頭が
  `<!doctype html` か `<html` のときだけ text/html)で素文に落ちるので、その判定に問うて
  HTML と名乗れなければ `<!DOCTYPE html>` を前置してから blob にする。PDF は PDF 抽出の節の道(blob は PDF
  そのもの、meta.extractor に pdftotext の版)。素文と Markdown はそのまま。
- 文書名は指定が無ければ URL から導く: ホスト(小文字)とパスと問い合わせ(? 以降)を、
  英数字と `.` `-` 以外を `_` に潰した 1 語にする(`https://arxiv.org/abs/2401.00001` は
  `arxiv.org_abs_2401.00001`)。スキームと断片(# 以降)は落とし、取り込み対象の拡張子
  (.pdf/.html/.md など。判定は拡張子の表と同じ media_for_extension)は残さない。100 文字を
  超えれば先頭 80 文字に URL の SHA-256 の先頭 8 桁を付ける。同じ URL は同じ名前になるので、
  再取得は同じ文書名への上書き(新しい doc_rev、previous が前版。旧チャンクは孤児になり
  gc が回収する)であり、内容が同じなら no-op である。
- doc_rev.meta には name・media(・extractor)に加えて出所が入る: source_url(要求した
  URL)、final_url(転送後)、fetched_at(UTC の unix 秒)、fetcher(「curl 7.81.0」)、
  content_type(相手の名乗り。名乗らなければ鍵ごと無し)、HTML なら dropped(自足化で
  落としたものの数。scripts・stylesheets・images・frames・fonts・handlers・others の 7 つを
  0 でも書く)。
- 入口は `POST /v1/collections/{collection}/fetch`(ボディ `{"url": "...", "name"?: "..."}`)
  と CLI の `uniqnode fetch`(CLI と API の節)。serve は curl と pdftotext を待つあいだ
  ストアのロックを持たない(取ってから、ロックを取って書く。SPEC §8.2 の同型)。誤りは
  誰が直せるかで分ける: 400 は URL や name の誤り、415 は取り込める種別でない、502 は
  向こうから取れない(繋がらない・期限切れ・2xx でない・上限超え)、503 は道具(curl・
  pdftotext)が無い、500 は取れたのにこちらで処理できない。

## 注釈の取り込みと照合

注釈索引ファイル(data.md)を読み、注釈 1 件を annotates 型の辺として取り込む。

```jsonc
// 節タイトルのノード
{ "v": 1, "kind": "node", "contents": "Generic Address Structure" }

// annotates 型の辺: blob のページ page に節 title がある、という一次言明
{ "v": 1, "kind": "edge", "type": "s256:<annotates 型ノード>",
  "members": ["s256:<節タイトルのノード>", "s256:<PDF blob>"],
  "meta": { "page": 162 } }
```

- data.md のパーサが受け付ける形式(この形だけを受け付ける。must/0020): spec_id の見出し +
  表題・形式・URL の 3 行コードブロック(形式が zip のときだけ書庫内パスの 4 行目)+
  「- p.N: 節タイトル」の箇条書き。タイトルはバッククォート囲みと裸の両方を受け、注釈が
  1 件も無い見出しも正常。形式外の行は行番号を示して失敗する。
- ページ番号 p.N は PDF の物理ページ番号(pdftotext の -f / -l と同じ 1 始まりの通し番号)で
  あり、紙面に刷られたページ番号ではない。meta.page も同じ番号空間で書く。
- 参照先は文書名ではなく PDF blob のハッシュ(I1。原理 6)。spec_id から blob への解決は
  ref `collections/<コレクション名>/<spec_id>` を引いて doc_rev.source を使う。したがって
  注釈の取り込みは PDF の取り込みの後になる。ref が無い spec_id があれば、その注釈を
  飛ばさず、書き込みを始める前に全体を失敗させて先に PDF を取り込むよう促す(must/0022 の
  同型)。
- 照合規則: 小文字化して英数字以外で分割し、数字だけの語を除き、同じ語の繰り返しを 1 語に
  まとめたうえで、タイトル側の語の 6 割以上が当該ページ本文の語の集合に含まれれば一致とする
  (判定は matched * 5 >= total * 3 の整数演算)。ページ本文は meta.page の一致するチャンクの
  text から集める(pdftotext の再実行はしない)。一致に使った本文行は根拠として検証記録に
  残る。照合は一つの関数(match_annotation)にあり、取り込み時の検証と訂正のための再検証が
  共用する(should/0135)。閾値 6 割が内訳の変わらない安全窓の内側にあることの実測は
  [docs/analysis/20260816-ingest-acceptance.md](#0be862ff-9448-40ab-aa6b-31c796f6482e) にある。
- 照合に一致した注釈は method を token-match とした検証記録付きで入る。機械照合に落ちるが
  人が正しいと確認した注釈は、承認リスト(「spec_id ページ番号」の行の並び)に載せて
  --manual で渡すと、method を manual とした検証記録(根拠行なし)付きで入る。どちらでもない
  不一致は取り込まず、一致率とともに報告する。黙った例外は無く、記録が残る形だけがある。
- 型ノード(annotates / corrects / supersedes)の c1 本文は定数として一箇所に定義され、
  生成側と判定側が同じ定数を使う(must/0023)。
- 注釈の辺・検証記録・訂正の辺は、コレクションごとの索引に束ねて ref
  `annotations/<コレクション名>` から指す。索引の ref が無いとこれらは doc_rev の到達閉包に
  入らず、複製や pin の対象にならない(原理 3 帰結 3)。

```jsonc
// 注釈索引: contents.annotations は {annotation, verification} の対の列、
// contents.corrections は corrects 辺 ID の列(空のときはキーごと省く)。
{ "v": 1, "kind": "node",
  "contents": {
    "annotations": [
      { "annotation": "s256:<annotates 辺>", "verification": "s256:<検証記録>" } ],
    "corrections": ["s256:<corrects 辺>"] } }
```

- 索引は取り込みの実行ごとにまとめて作り直し、現行索引の corrections を持ち越す。同一の
  索引に落ちたら ref は触らない(no-op)。辺と検証記録の結びつけはこの対だけが持つ
  (検証記録は言明を指さず、言明も検証記録を持たない。理由は訂正の表現の節)。

## 訂正の表現

```jsonc
// 検証記録: 照合の方法と根拠を残すノード。method は token-match(機械照合)または
// manual(人手確認)。
{ "v": 1, "kind": "node",
  "contents": { "method": "token-match", "evidence": ["CPUID—CPU Identification"] } }

// corrects 型の辺: 誤りの訂正。members は [新しい言明, 誤った言明] の順。
{ "v": 1, "kind": "edge", "type": "s256:<corrects 型ノード>",
  "members": ["s256:<新しい言明>", "s256:<誤った言明>"],
  "meta": { "reason": "…", "verification": "s256:<検証記録>" } }
```

- meta.reason に理由、meta.verification に検証記録の ID を置く。meta 内の s256: 文字列も
  参照なので(SPEC §4.3)、訂正の到達閉包が新旧の言明と根拠ごと運ばれる(原理 3 帰結 1)。
- 検証記録は「何を検証したか」を持たず、「どう検証し何を根拠にしたか」だけを持つ。検証記録
  から言明へ参照を張ると注釈の辺の閉包に根拠が入らず、逆に言明の meta に検証記録を入れると
  根拠行が変わるたびに注釈そのものが別の言明になる。検証は言明についての別の言明であり、
  言明の同一性には混ぜない(原理 2 と原理 4)。
- 訂正の対象は既にストアにある言明だけである(照合で弾かれた注釈はストアに入らないので
  対象にならない)。照合は「そのページにその語が載っている」ことしか確かめず、通っても真とは
  限らないから、照合を通った言明への訂正は異常系ではなく正常系である(原理 4)。
- supersedes 型の辺は、文書の新版に伴う張り替えを誤りの訂正と区別するための型で、members の
  順序は corrects と同じ(旧言明は旧 blob についての真のままであり、誤りだったわけでは
  ない)。現在あるのは型定数の定義までで、発行経路は無い(改版追随が実際に必要になったとき
  に足す)。
- 現在の見えは ref が決める。旧言明と訂正の辺は履歴として辿れる(原理 5)。

## 訂正の発行

CLI `uniqnode correct <dir> <コレクション名> <誤った言明ID> <新しい言明ID> <理由>` が訂正を
発行する。

- 両 ID はストアに存在しなければならず、新しい言明は annotates 型の辺でなければならない
  (再照合はタイトルとページ本文のトークン照合なので、annotates 辺にしか適用できない)。
  言明は自分自身を訂正できない。満たさなければ理由を言って失敗する(must/0022)。
- 新しい言明は取り込み時と同じ照合(注釈の取り込みと照合の節)で再照合される。ページ本文は、
  新しい言明の参照する blob を source に持つ現行 doc_rev を `collections/<コレクション名>/`
  配下の ref から探して得る。doc_rev が無い・照合に落ちる場合は発行せず失敗する。
- 発行は corrects 型ノード・検証記録(method は token-match)・corrects 辺を書き、索引の
  corrections に辺の ID を足して ref `annotations/<コレクション名>` を張り替える(既に載って
  いれば足さない)。すべて content-addressed なので、同じ訂正の再発行は新規オブジェクトを
  生まない(べき等 = I1)。

## 逆引き

あるオブジェクト ID を参照している既知オブジェクトの一覧を、導出データ(I4)として持つ
(node/src/store.rs の ReferrerIndex)。逆向きの知識は「いま自分が知っている言明の集合」に
相対的な事実であり、言明にはできない(原理 3 帰結 2)。これにより「この言明への訂正は
あるか」が、旧言明しか持たない DBノードでも問える。

- 遅延構築である。Store::open では作らず、最初の要求で全オブジェクトを一度走査して作る。
  open はオブジェクトをパースせずハッシュだけを見る唯一の共有経路であり、そこに全件パースを
  足すと、壊れたオブジェクト 1 個でストアが開かなくなる。
- 世代は構築時点の object_count。オブジェクトは追記専用で消えないため、これが現在値と一致
  する限り索引は最新で、書き込みが挟まれば次の要求で作り直す。
- c1 としてパースできないオブジェクト(生 blob 等)は参照ゼロとして飛ばす(参照の規約
  SPEC §4.3 は c1 値の中の s256: 文字列だけを参照と見なすため、パースできない内容は定義上
  参照を持たない)。
- 知らない ID には空の一覧を返す。逆引きは「自分の知る範囲」の導出データであり、空は不在の
  言明ではない(SPEC §7.2 の開世界と同型)。

## git の木からの取り込み

`uniqnode ingest-git` は、手元の git の木の、ある ref が指すコミットに追跡されている
ファイルのうち、指定した道(`--paths`。カンマ区切り、木の根からの相対パス、繰り返せる)の下の
ものだけを取り込む。定期に回す形は
[docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2) の「git の木を定期に取り込む」。

- 読むのはコミットの木だけ: `git ls-tree -r` で道の下の項目を並べ、通常のファイル(mode
  100644・100755)の中身を `git cat-file blob` で一時の置き場(TMPDIR の下、
  `uniqnode-ingest-git-<pid>-<時刻>`)へ書き出し、そこを取り込み起点にして ingest と同じ道
  (main.rs の plan_ingest と、--serve-url の有無で決まる 2 つの形)を通す。作業ディレクトリは
  読まないので、追跡していないファイル(secrets/ など)もコミット前の書きかけも入らない。
  裸の木(push を受ける bare repository)でも同じに読める。置き場は成功でも失敗でも消す。
- 文書名は木の根からの相対パスから拡張子を除いたもの(`docs/design/STORE.md` は
  `docs/design/STORE`)。ingest と同じく、対象外の拡張子は一覧で言う。
- シンボリックリンク(120000)とサブモジュール(160000)は書き出さず、名前を言う。リンクを
  書き出さないのは、取り込み起点を歩くときにリンクを辿るので、指定した道の外(追跡して
  いないファイルを含む)の中身が入りうるからである。
- 道の指定は字面どおり(`--literal-pathspecs`。`:(exclude)` のような魔法は効かない)。
  絶対パス・`..`・空の要素は断る。既定の道の一覧は持たない(何を入れるかは呼び手が決める。
  lamalium の木の一覧は unit の既定に置いた)。
- git の外へは取りに行かない(fetch も clone もしない)。木を置き、最新にするのは前の段の仕事。
- 断るもの(何も送らず非 0 で終わる。0 件で成功したことにしない。must/0022): 木が無い、
  git の木でない(木の親を GIT_CEILING_DIRECTORIES に置くので、別の木の中のディレクトリを
  指しても外の木は読まない)、ref がコミットを指さない、`--paths` の道の 1 つでもそのコミットに
  無い(ls-tree は無い道を黙って飛ばすので、道ごとに 1 項目以上あることを確かめる)、書き出せる
  通常のファイルが 1 つも無い。git の誤り(所有者の違う木への「dubious ownership」など)は
  git の文をそのまま載せる。
- 限り: git から消えた文書(ファイルの削除・改名の旧名)はコレクションから消えない。
  取り込みは足すか書き換えるだけで、ref を tombstone しない。消すのは ref の tombstone
  (REST の PUT /v1/refs/{path} に `{"target":null}`)で、旧版と合わせて gc が回収する。

## CLI と API

CLI はストアの排他ロックを取るので serve 停止中のストア用であり、serve 中は HTTP API を使う
(既存の sync サブコマンドと同じ扱い)。例外は ingest の --serve-url で、ストアを開かずに
走っている serve の HTTP API へ送る。形の正典は node/src/main.rs の usage と node/src/api.rs。

- `uniqnode ingest <dir> <コレクション名> <パス> [--pdftotext <exe>]`: 文書の取り込み。
  ディレクトリは再帰(名前順)。対象の拡張子は .md / .markdown / .txt / .html / .htm / .pdf のみで、
  それ以外は取り込まず、対象外の一覧を最後に印字する(黙って捨てない)。1 件ごとに
  `<c>/<文書名>: updated|no-op chunks=N new_objects=N doc_rev=<id>` の行を出し、最後に
  `取り込み: N 件(updated N、no-op N)、対象外 N 件` の 1 行で締める。
- `uniqnode ingest <dir> <コレクション名> <パス> --serve-url <url>`: 転送する形の取り込み。
  ストアを開かず(ロックは serve が持っている)、選んだ文書を 1 件ずつ走っている serve の
  `PUT /v1/collections/{collection}/documents/{name}` へ送る(url は serve の主の口の根。
  例 `http://127.0.0.1:7440`)。ファイルの選び方と文書名は上の形と同じ 1 箇所(main.rs の
  plan_ingest)で決まり、{name} には文書名に元の拡張子を付け直して送る。種別の判定・PDF の
  抽出・チャンク分け・同一内容の判定は serve 側の put_document が持つので、同じ内容は
  上の形と同じ doc_rev の ID になり、変わらない文書は no-op である。行は上の形から chunks を
  除いたもの(PUT の応答がチャンク数を持たない)で、締めの 1 行は同じ。serve が 2xx 以外を
  返したら、ファイル名と serve の理由(応答の error、無ければ本文の頭)を言って非 0 で
  止まる(上の形が最初の誤りで止まるのと同じ。先に送った文書は入ったまま)。届かなければ
  serve の起こし方を添えて失敗する(MCP の転送する形と同じ ServeClient を使う)。
  断るもの: --pdftotext との併用(PDF を抽出するのは serve で、serve が PATH から引く。
  効かせる先が無い指定を黙って捨てない)と、空白・制御文字・`?` を含むコレクション名・文書名
  (serve は道のパーセント符号を解かないので、要求行に載らないか query と読まれる)。名前は
  1 件も送る前に全部を確かめる。PUT 1 件を待つ期限は 600 秒(千ページ級の PDF の抽出を
  serve が終えるまで答えないため)。
- `uniqnode ingest-git <dir> <コレクション名> <木> --paths <道,道,...> [--ref <ref>] [--serve-url <url> | --pdftotext <exe>]`:
  git の木からの取り込み(git の木からの取り込みの節)。ref の既定は main。--serve-url の有無で
  上の 2 つの形のどちらかになり、1 件ごとの行と締めの 1 行も同じ。その前に
  `木: <木> の <ref> = <コミット ID>(書き出し N 件、書き出さなかったもの N 件)` の 1 行と、
  書き出さなかったものごとの `書き出さない(シンボリックリンク: <道>)` の行を出す。
- `uniqnode fetch <dir> <コレクション名> <url> [--name <名>] [--pdftotext <exe>]`: URL からの
  取り込み(URL からの取り込みの節)。ストアを開くのは取ってからで、取れない URL では
  ストアを作らない。1 行目は ingest と同じ形に media と final_url を足したもの、HTML なら
  2 行目に dropped の内訳。
- `uniqnode ingest-annotations <dir> <コレクション名> <data.md> [--manual <承認リスト>]`:
  注釈索引の取り込み(注釈の取り込みと照合の節)。
- `uniqnode correct <dir> <コレクション名> <誤った言明ID> <新しい言明ID> <理由>`: 訂正の
  発行(訂正の発行の節)。
- `PUT /v1/collections/{collection}/documents/{name}[?meta.<key>=<value>&...]`: 文書 1 件の
  取り込み。本文は生バイト列で、種別は name の拡張子で判定し、ref 名には拡張子を残さない。
  応答は doc_rev の ID・new_objects・ref_updated・previous(上書きなら前版の doc_rev、新規なら
  null。同じ内容で ref_updated が false のときは現行の doc_rev)。本体は node/src/api.rs の
  put_document で、MCP の add_document([docs/design/MCP.md](#dacd474d-424a-45d5-a278-766fc2465dd9))
  も同じ関数を通る。query の `meta.<key>=<value>` は出所を doc_rev.meta に足す欄(取り込みの
  extra_meta。URL からの取り込みが source_url などを足すのと同じ場所): key は `[a-z0-9_]{1,32}`、
  value は %XX をデコードして 1..=200 字(`+` は空白に読み替えない)。name・media・extractor は
  取り込みが決めるので query で名乗れず、同じ鍵の繰り返し・`meta.` 以外の鍵・壊れた % も 400 で
  理由を言い、何も書かれない(読み方は api.rs の parse_meta_query)。meta は doc_rev にだけ写り、
  検索の citation の形は変えない。同じ本文の再 PUT は meta が違っても no-op(同一内容の判定は
  source と chunks 列で、meta を見ない)。serve の読み口からのこの PUT は
  [docs/design/AGENT_DOOR.md](#02f79aec-2f12-41e6-bede-1557d4719e4d) の「書く口」。
- `POST /v1/collections/{collection}/fetch`: URL からの取り込み(URL からの取り込みの節)。
  ボディは `{"url": "...", "name": "..."}`(name は省略可)。応答は PUT documents と同じ
  doc_rev・new_objects・ref_updated・previous に final_url・name・media を足し、HTML なら
  dropped(落としたものの数)も載る。本体は api.rs の fetch_into で、MCP の fetch_url も同じ
  関数を通る。
- `GET /v1/objects/{id}/referrers`: 逆引き(逆引きの節)。応答は referrers(ID の列)。
- 引用を組み立てる専用 API は無い(ref → doc_rev → chunks の添字と、既存のオブジェクト取得
  で組める)。

## 既知の癖

- .txt が対象拡張子なので、取り込み起点の下にある SHA-1 台帳のような index.txt もプレーン
  テキストの文書として取り込まれる。コレクションを狭めたければ、取り込み起点を絞るか ref を
  tombstone する。
- PDF のチャンクの meta.breadcrumbs は、節見出しの経路である(しおりか語の高さから復元する。
  RENDITION と同じ道具を使う抽出で、node/src/outline.rs が家)。全 34 本の 95.9% のページに
  経路が付く。しおりも高さも拾えない文書(組版が節番号の体を成さないもの)は空のままで、
  そのときは索引語が本文だけから出る。
- 注釈索引は 1 オブジェクトで、注釈数に比例して大きくなる。100 件規模を超えて常用するなら
  分割が要る(現在は分割していない)。
- URL からの取り込みは、serve が受けた URL をノード自身が取りに行く形なので、serve に
  届く者はノードの居る網から見える先(内側のアドレスを含む)を取らせられる(SSRF の形)。
  serve は 127.0.0.1 に束縛される前提で、第一版の守りは「http と https のみ・転送 10 回
  まで・30 秒と 64 MB の上限」にとどめ、内側のアドレスの拒否はしていない。robots.txt は
  見ない(取りに行くのは利用者が 1 枚ずつ指した URL で、這うわけではない)。
- 同じ URL の再取得が同じ内容だったとき、fetched_at は最初にその内容を取った時刻のまま
  残る(doc_rev も ref も動かさないため)。「いつ最後に確かめたか」は記録に無く、ログの
  fetch の行だけが持つ。

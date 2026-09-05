//! 評価ハーネスの回帰テスト(EVAL (uuid:1109a04b-923e-4493-8f00-d704047d6a2a))。
//! 固定の小コーパスと「クエリ → 正解チャンク」対で、現在の検索方式の数値を基準線として
//! 固定する。コーパスはソースとは別ファイル(should/0112)。
//!
//! 検索層(Store + SearchIndex)を直に駆動する。ハーネスが測るのは順位付けであって
//! HTTP の往復ではないので、serve を起こさずに済ませている(API そのものの検証は
//! node/tests/search.rs の担当)。

mod common;

use std::path::PathBuf;
use uniqnode::embed::{
    fill_cache, Embedder, HybridSearch, SearchMethod, VectorCache, VectorIndex,
};
use uniqnode::eval::{
    chunk_name, evaluate, per_mille, Bm25Retrieval, EvalCase, EvalReport, HybridRetrieval,
    LowInformationFiltered, Retrieval,
};
use uniqnode::ingest::{chunk_markdown, ingest_document, DocumentInput};
use uniqnode::search::{chunk_terms_of, terms_of, SearchIndex};
use uniqnode::store::{Store, StoreConfig};

/// 固定の小コーパス(文書名, 本文)。文書名は資材のファイル名から拡張子を除いたもので、
/// 正解チャンクの名前 `<文書>#<chunks 列の添字>` の前半になる。前の 3 文書はそれぞれ
/// 日本語・英語・識別子の場を持ち、日本語と英語の文書は先頭に列挙(目次)の節を持つ。
/// eval_gap は語彙の隔たりの場で、クエリと正解が索引語を共有しない対のためにある
/// (VOCABULARY_GAP_CASES)。
///
/// 文書を足すときは末尾に足す。対照方式(CorpusOrder)はコーパス順に返すので、前に
/// 割り込ませると既存の対の順位が動く。
const CORPUS: &[(&str, &str)] = &[
    ("eval_ja", include_str!("assets/eval_ja.md")),
    ("eval_en", include_str!("assets/eval_en.md")),
    ("eval_api", include_str!("assets/eval_api.md")),
    ("eval_gap", include_str!("assets/eval_gap.md")),
];

/// 固定コーパスには入れない資材(目次の節と本文の節を 1 つずつ持つ文書)。低情報の
/// 後処理が効くことを見るためだけに、別の索引へ入れて使う。API の試験(node/tests/search.rs)
/// と同じ資材である。
const LOW_INFO: &str = include_str!("assets/search_lowinfo.md");

/// 語彙が一致する対(クエリの語が正解チャンクの本文か見出しに現れる)。正解はチャンクの
/// 名前(`<文書>#<添字>`)で書く。
/// チャンクのオブジェクト ID は内容ハッシュなので、コーパスの本文を一字直すだけで
/// 変わり、固定資材に書けないためである。添字は chunk_markdown が見出しごとに切った
/// 順で、各文書の 0 番は列挙(目次)の節である。
///
/// [目次] と書いた対は、実データで観察された難所(SEARCH
/// (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580) の「既知の癖」)の再現である: クエリの句が
/// 目次(列挙)側と定義(本文)側の両方に現れ、正解は定義側にある。
///
/// [見出しのみ] と書いた対は、正解チャンクの本文にはクエリの語が一つも無く、見出し
/// (meta.breadcrumbs)にしかない場合である。索引が本文だけを見ていたころ、この 2 対は
/// 正解に届かなかった(監査証跡の保全は目次の節だけが返り、token_quarters は空振り)。
const CASES: &[(&str, &str)] = &[
    // 日本語。
    ("世代の整合", "eval_ja#1"),                 // [目次] 同じ句が目次にもある
    ("保持表明の書式", "eval_ja#2"),             // [目次] 同じ句が目次にもある
    ("破損した区間の取り直し", "eval_ja#3"),     // 見出しとは違う言い回し
    ("機会層の退避", "eval_ja#4"),               // [目次] 同じ句が目次にもある
    ("索引はいつ作るのか", "eval_ja#5"),         // 見出しとは違う言い回し
    ("監査証跡の保全", "eval_ja#6"),             // [見出しのみ] 本文に句が無い
    // 英語。
    ("saturates repeated term frequency", "eval_en#1"),
    ("cosine similarity between vectors", "eval_en#2"),
    ("reciprocal rank fusion", "eval_en#3"),     // [目次] 同じ句が Contents にもある
    // 識別子(1 語まるごとの完全一致)と、日本語に識別子が混じる場合。
    ("CHUNK_TOKEN_LIMIT", "eval_api#0"),
    ("token_estimate", "eval_api#1"),
    ("pdftotext の版", "eval_api#3"),
    ("token_quarters", "eval_api#4"),            // [見出しのみ] 本文に識別子が無い
];

/// 語彙が隔たる対(問いと正解が同じことを言っているのに、索引語を一つも共有しない)。
/// キーワード一致では原理的に届かない場であり、意味で近さを測る方式
/// (埋め込み。SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))が効くはずの
/// 領域である。共有語が空であることは
/// a_vocabulary_gap_pair_shares_no_index_term_with_its_answer が機械的に確かめ、現在の
/// BM25 が実際に届かないことは bm25_cannot_reach_any_vocabulary_gap_pair が確かめる。
///
/// 将来この対のどれかが上位に来たら、それは語の一致ではない何か(意味検索)が効いた
/// 証拠である。
const VOCABULARY_GAP_CASES: &[(&str, &str)] = &[
    // [言い換え] 本文は「電源断・復帰」、問いは「電気が消えた・立ち上げ」。同じ出来事を
    // 漢語と和語で言い分けているだけなので、bigram は一つも重ならない。
    ("急に電気が消えたときの立ち上げ", "eval_gap#0"),
    // [言い換え] 本文は「帯域の抑制・上限で抑える」、問いは「混まないように・速さを
    // ゆるめる」。共有する索引語は無い。
    ("ネットワークが混まないように送る速さをゆるめたい", "eval_gap#1"),
    // [上位語と下位語] 本文は具体名(ed25519・x25519・chacha20)だけを並べ、総称の
    // 「暗号方式」を一度も書いていない。総称で引くと索引語が無い。
    ("どの暗号方式を採っているか", "eval_gap#2"),
    // [英語の問いで日本語の本文] 本文は日本語だけで ASCII の語を持たず、問いは英語だけ。
    // 語の集合が文字種の段階で交わらない。
    ("how long are request logs kept", "eval_gap#3"),
    // [質問文の形] 本文は名詞句と手順(計画停止・保持表明)、問いは「どうすれば〜できるか」。
    // 問いの側の語(安全・落とす)は本文に無い。
    ("どうすればノードを安全に落とせるか", "eval_gap#4"),
    // [日本語の問いで英語の本文] 逆向きの多言語。本文は英語、問いは日本語で、こちらも
    // 文字種の段階で交わらない。
    ("時計がずれても大丈夫か", "eval_gap#5"),
];

/// 評価する打ち切り k。
const CUTOFFS: &[usize] = &[1, 5, 10];

/// 評価する対の全体(語彙が一致する対のあとに、語彙が隔たる対)。
fn cases() -> Vec<EvalCase> {
    CASES
        .iter()
        .chain(VOCABULARY_GAP_CASES)
        .map(|(query, relevant)| EvalCase::new(query, &[*relevant]))
        .collect()
}

/// 索引の組(BM25 と、埋め込みを与えたときだけ作るベクトル)。
struct IndexedCorpus {
    lexical: SearchIndex,
    vectors: Option<VectorIndex>,
}

/// コーパスを一時ストアへ取り込んで索引を作る。索引は Store を借りない導出データ
/// なので、作った後はストアもディレクトリも畳んでよい。
///
/// 埋め込みを与えると、同じ走査から作ったベクトルの索引も返る。ベクトルはメモリだけの
/// キャッシュに置く: 評価は毎回コーパスを取り込み直すので、残しても次回の鍵
/// (チャンクのオブジェクト ID)は同じだが、測るたびに実際に埋め込みサーバを通す方が、
/// 測定の前提(サーバが今も同じ模型で応じること)を確かめられる。
fn indexed_corpus_with(
    name: &str,
    embedder: Option<&Embedder>,
    documents: &[(&str, &str)],
) -> IndexedCorpus {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("uniqnode-eval-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
    let mut store = Store::open(StoreConfig::new(&dir)).expect("open store");
    for (document, text) in documents {
        let chunks = chunk_markdown(text);
        ingest_document(
            &mut store,
            &DocumentInput {
                collection: "eval",
                name: document,
                source: text.as_bytes(),
                media: "markdown",
                chunks: &chunks,
                extractor: None,
                extra_meta: &[],
            },
        )
        .expect("ingest");
    }
    let lexical = SearchIndex::build(&store).expect("build index");
    let vectors = embedder.map(|embedder| {
        let mut cache = VectorCache::in_memory(embedder.embedder_id(), embedder.dimension());
        fill_cache(&store, embedder, &mut cache, &mut |_| {}).expect("埋め込み");
        VectorIndex::from_cache(&store, &cache).expect("ベクトルの索引")
    });
    drop(store);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    IndexedCorpus { lexical, vectors }
}

fn indexed_corpus(name: &str) -> SearchIndex {
    indexed_corpus_with(name, None, CORPUS).lexical
}

fn bm25_report(index: &SearchIndex) -> EvalReport {
    evaluate(&Bm25Retrieval::new(index, Some("eval")), &cases(), CUTOFFS)
}

/// 対照の方式が持つチャンク 1 件(名前と索引語)。
struct CorpusChunk {
    name: String,
    terms: Vec<String>,
}

/// 対照の方式(順位付けの下限): クエリ語を一つでも含むチャンクを、順位を付けずに
/// コーパス順で返す。差し替え点が本当に方式を差し替えられること、指標が方式の違いを
/// 数値にできることを見るための対照であり、生産経路では使わない。BM25 の数値が
/// これを上回らなければ、順位付けは何も稼いでいないことになる。
///
/// 索引語だけは索引と同じ chunk_terms_of(本文と見出し)を呼ぶ(should/0135。語の
/// 採り方まで別実装にすると、方式の差なのか語の採り方の差なのかが読めなくなる)。
/// 順位付けは持たないので、BM25 の得点計算を写した第二実装にはなっていない。
struct CorpusOrder {
    chunks: Vec<CorpusChunk>,
}

impl CorpusOrder {
    /// コーパスを索引と同じ切り方(chunk_markdown → chunk_terms_of)で読み込む。
    fn over_corpus() -> CorpusOrder {
        let mut chunks = Vec::new();
        for (document, text) in CORPUS {
            for (position, chunk) in chunk_markdown(text).iter().enumerate() {
                chunks.push(CorpusChunk {
                    name: chunk_name(document, position),
                    terms: chunk_terms_of(&chunk.text, &chunk.breadcrumbs),
                });
            }
        }
        CorpusOrder { chunks }
    }
}

impl Retrieval for CorpusOrder {
    fn name(&self) -> &str {
        "corpus-order"
    }

    fn ranked(&self, query: &str, top_k: usize) -> Vec<String> {
        let mut query_terms = terms_of(query);
        query_terms.sort();
        query_terms.dedup();
        self.chunks
            .iter()
            .filter(|chunk| query_terms.iter().any(|term| chunk.terms.contains(term)))
            .map(|chunk| chunk.name.clone())
            .take(top_k)
            .collect()
    }
}

/// BM25 の現在の実測値を基準線として固定する。
///
/// 数値は「あるべき値」ではなく「今の値」である。このテストの仕事は目標達成の判定では
/// なく、順位が黙って悪化しないことの検出である(カバレッジを計測のみとしてゲートに
/// しない TESTING (uuid:267326f7-e919-48f2-9737-fe0c0daec9d5) の方針と同じ扱い)。
/// 語彙が一致する 13 対は全部 1 位、語彙が隔たる 6 対は全部圏外という内訳なので、
/// 数値は上限に達しておらず、上げる改良も下げる悪化も見える。
///
/// 基準線を意図して動かすときの手順: (1) このテストを走らせ、失敗メッセージの内訳で
/// クエリごとの順位が意図どおりに動いたことを目で確かめる、(2) その実測値でリテラルを
/// 置き換える、(3) 何を変えたから動いたのかをコミットメッセージに書く。先に数値を
/// 書いてから実装を合わせにいかない。
#[test]
fn bm25_holds_the_recorded_baseline_on_the_fixed_corpus() {
    let index = indexed_corpus("baseline");
    let report = bm25_report(&index);
    assert_eq!(
        report.queries.len(),
        CASES.len() + VOCABULARY_GAP_CASES.len(),
        "全対が評価されるべき"
    );
    // 実測 2026-08-17(千分率)。19 対のうち、語彙が一致する 13 対が 1 位、語彙が隔たる
    // 6 対が圏外なので、どの打ち切りでも 13/19 = 0.684 で並ぶ。語彙が隔たる対を足す前は
    // 13 対すべてが 1 位で、4 つの指標がいずれも 1.000 に飽和していた。
    assert_eq!(per_mille(report.mean_recall(1)), 684, "Recall@1 の基準線\n{}", report.detail());
    assert_eq!(per_mille(report.mean_recall(5)), 684, "Recall@5 の基準線\n{}", report.detail());
    assert_eq!(per_mille(report.mean_recall(10)), 684, "Recall@10 の基準線\n{}", report.detail());
    assert_eq!(per_mille(report.mean_reciprocal_rank), 684, "MRR の基準線\n{}", report.detail());
}

/// 検索方式を切り替えて数値が比較できることを実際の検索で示す:
/// 同じコーパスと同じ対のまま方式だけを差し替え、両方の数値を並べて比べる。対照は
/// 順位を付けない下限なので、この差が BM25 の順位付けが稼いでいるぶんである。
/// 対照の数値も基準線として固定する(更新手順は BM25 の基準線と同じ)。
#[test]
fn switching_the_method_changes_the_numbers_on_the_same_pairs() {
    let index = indexed_corpus("switch");
    let bm25 = bm25_report(&index);
    let floor = evaluate(&CorpusOrder::over_corpus(), &cases(), CUTOFFS);
    assert_eq!(floor.method, "corpus-order");
    // 実測 2026-08-17(千分率)。同じ 19 対で、順位を付けないだけで Recall@1 は 0.684 から
    // 0.263 へ、MRR は 0.684 から 0.465 へ落ちる。対照の Recall@5 と Recall@10 が BM25 と
    // 並ぶのは、語彙が一致する対では一致するチャンクが 5 件に収まるためで(取りこぼしでは
    // なく規模の話)、語彙が隔たる 6 対は両方式とも圏外である。語彙が隔たる対を足す前の
    // 対照は Recall@1 0.385・Recall@10 1.000・MRR 0.679 だった。
    assert_eq!(
        per_mille(floor.mean_recall(1)),
        263,
        "対照の Recall@1 の基準線\n{}",
        floor.detail()
    );
    assert_eq!(
        per_mille(floor.mean_recall(5)),
        684,
        "対照の Recall@5 の基準線\n{}",
        floor.detail()
    );
    assert_eq!(
        per_mille(floor.mean_recall(10)),
        684,
        "対照の Recall@10 の基準線\n{}",
        floor.detail()
    );
    assert_eq!(
        per_mille(floor.mean_reciprocal_rank),
        465,
        "対照の MRR の基準線\n{}",
        floor.detail()
    );
    assert!(
        bm25.mean_reciprocal_rank > floor.mean_reciprocal_rank,
        "BM25 の順位付けは順位なしの下限を上回るはず\n{}\n{}",
        bm25.detail(),
        floor.detail()
    );
}

/// かつての難所(実データで観察された癖)の現状の記録: 同じ句が目次と定義の両方に
/// あるとき、どちらが上に来るか。見出しを索引語に入れる前は、目次の節が短いぶん文書長
/// 正規化で得をして 1 位を取り、定義の節は 2 位だった。見出しに重みを付けて数える今は、
/// 定義の節が自分の題を持つぶん重く、目次の節を上回る。これも達成すべき目標ではなく
/// 現状の記録であり、順位が入れ替わったらこの期待値も実測に合わせて更新する。
#[test]
fn the_definition_outranks_the_table_of_contents_today() {
    let index = indexed_corpus("contents");
    let report = bm25_report(&index);
    let outcome = report.outcome_of("世代の整合");
    assert_eq!(outcome.first_relevant_rank, Some(1), "定義の節が来る順位\n{}", report.detail());
    // 実測 2026-08-16: 目次の節は 2 位。句を持つ点では同じなので、消えるのではなく下がる。
    assert_eq!(
        outcome.ranked.get(1).map(String::as_str),
        Some("eval_ja#0"),
        "目次の節が来る順位\n{}",
        report.detail()
    );
}

/// 見出しにしかない語で本文の節に届く。索引が chunk の text だけを索引語にしていた
/// ころ、この 2 対は正解に届かなかった: 日本語の対は目次の節だけが返り(節そのものは
/// 圏外)、識別子の対は空振りだった。基準線の平均が動いた理由をここで名指しする。
#[test]
fn a_phrase_that_lives_only_in_a_heading_finds_its_section() {
    let index = indexed_corpus("heading-only");
    let report = bm25_report(&index);
    let japanese = report.outcome_of("監査証跡の保全");
    assert_eq!(
        japanese.ranked.first().map(String::as_str),
        Some("eval_ja#6"),
        "見出しにしかない句で節が 1 位のはず\n{}",
        report.detail()
    );
    let identifier = report.outcome_of("token_quarters");
    assert_eq!(
        identifier.ranked.first().map(String::as_str),
        Some("eval_api#4"),
        "見出しにしかない識別子で節が 1 位のはず\n{}",
        report.detail()
    );
}

/// 語彙が隔たる対が届かない理由を、順位ではなく語で名指しする: 問いの索引語と正解
/// チャンクの索引語(本文と見出し)の共通部分が空である。BM25 の得点は共有する語から
/// しか生まれない(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580) の BM25)ので、
/// 共有語が無いことは「順位が低い」ではなく「原理的に返らない」を意味する。
///
/// この対を足すときの受け入れ条件でもある。共有語が一つでもあれば、その語が偶然
/// 効いて当たることがあり、意味検索が効いた証拠にならなくなる。
#[test]
fn a_vocabulary_gap_pair_shares_no_index_term_with_its_answer() {
    let corpus = CorpusOrder::over_corpus();
    for (query, relevant) in VOCABULARY_GAP_CASES {
        let answer = corpus
            .chunks
            .iter()
            .find(|chunk| chunk.name == *relevant)
            .unwrap_or_else(|| panic!("正解チャンク {relevant} がコーパスに無い"));
        let mut shared: Vec<String> = terms_of(query)
            .into_iter()
            .filter(|term| answer.terms.contains(term))
            .collect();
        shared.sort();
        shared.dedup();
        assert!(
            shared.is_empty(),
            "問い {query:?} と正解 {relevant} は索引語 {} を共有している(共有語があると\
             キーワード一致で当たりうるので、語彙の隔たりの対にならない)",
            shared.join("、")
        );
    }
}

/// 低情報チャンクを落とす後処理(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580) の
/// 「低情報チャンクの後処理」)を、評価方式の 1 つとして測る。生産経路の POST /v1/search が
/// 既定でしていることと同じ判定である。
///
/// この固定コーパスには低情報チャンクが 1 件も無いので、数値は素の方式と 1 つも変わら
/// ない。実データ(仕様書 PDF 25 本)では 25,098 チャンクのうち 1,960 件(7.8%)がこの
/// 判定に落ち、純日本語の問いの上位 10 件では 10.7% を占めていた(実測 2026-08-17。
/// 20260817-real-corpus-search-quality
/// (uuid:faeda9ac-5e9e-4091-8122-2fba9f80c8db))。合成の小コーパスがその癖を
/// 持たないことの記録であって、後処理が効かないことの証拠ではない。
#[test]
fn the_low_information_filter_finds_nothing_to_drop_on_the_fixed_corpus() {
    let index = indexed_corpus("low-information");
    let bm25 = Bm25Retrieval::new(&index, Some("eval"));
    let filtered = LowInformationFiltered::new(&bm25, &index);
    assert_eq!(filtered.name(), "bm25-filtered", "方式名で数値の出どころが読めるべき");
    // 実測 2026-08-17: 固定コーパスの全チャンクのうち、低情報と判定されるのは 0 件。
    assert_eq!(
        filtered.low_information_count(),
        0,
        "固定コーパスに低情報チャンクは無いはず(あるなら判定か資材のどちらかが動いた)"
    );
    let report = evaluate(&filtered, &cases(), CUTOFFS);
    // したがって基準線は BM25 と同一である(実測 2026-08-17。千分率)。
    assert_eq!(
        per_mille(report.mean_recall(1)),
        684,
        "後処理つき BM25 の Recall@1 の基準線\n{}",
        report.detail()
    );
    assert_eq!(
        per_mille(report.mean_recall(5)),
        684,
        "後処理つき BM25 の Recall@5 の基準線\n{}",
        report.detail()
    );
    assert_eq!(
        per_mille(report.mean_recall(10)),
        684,
        "後処理つき BM25 の Recall@10 の基準線\n{}",
        report.detail()
    );
    assert_eq!(
        per_mille(report.mean_reciprocal_rank),
        684,
        "後処理つき BM25 の MRR の基準線\n{}",
        report.detail()
    );
    let plain = bm25_report(&index);
    for outcome in &report.queries {
        assert_eq!(
            outcome.ranked,
            plain.outcome_of(&outcome.query).ranked,
            "落とすものが無いのに順位が動いた: {:?}",
            outcome.query
        );
    }

    // 後処理が本当に効くことは、低情報チャンクを持つ別の索引で見る(固定コーパスで
    // 0 件だったのが「判定が何も落とさない」ためでないことの確認。should/0137)。
    // 資材は API の試験と共用する(目次の節と本文の節を 1 つずつ持つ文書)。
    let with_contents =
        indexed_corpus_with("low-information-asset", None, &[("search_lowinfo", LOW_INFO)]).lexical;
    let bm25 = Bm25Retrieval::new(&with_contents, Some("eval"));
    let filtered = LowInformationFiltered::new(&bm25, &with_contents);
    assert_eq!(filtered.low_information_count(), 1, "目次の節が低情報のはず");
    let query = "索引の構築";
    assert_eq!(
        bm25.ranked(query, 10),
        vec!["search_lowinfo#1".to_string(), "search_lowinfo#0".to_string()],
        "素の BM25 は目次の節も返す(実測 2026-08-17: 本文の節が 1 位、目次の節が 2 位)"
    );
    assert_eq!(
        filtered.ranked(query, 10),
        vec!["search_lowinfo#1".to_string()],
        "後処理は目次の節だけを落とす"
    );
}

/// 埋め込み(意味検索)と RRF 融合の実測値。埋め込みサーバを要求するテストはこれと
/// the_vocabulary_gap_pairs_that_semantics_reaches の 2 つで、無い環境では黙って飛ばさず
/// 起動手順を示して落ちる(TESTING (uuid:267326f7-e919-48f2-9737-fe0c0daec9d5))。純粋な
/// 部品(RRF の融合・コサイン・応答の読み取り・キャッシュの往復)の検査はサーバを要求
/// しない単体テスト(node/src/embed.rs)の側にある。
#[test]
fn embedding_and_fusion_hold_the_recorded_baseline_on_the_fixed_corpus() {
    let embedder = common::require_embedding_server();
    let indexed = indexed_corpus_with("embedding", Some(&embedder), CORPUS);
    let vectors = indexed.vectors.as_ref().expect("ベクトルの索引");
    // 二つの索引が同じチャンクを同じ並びで見ていること(位置で順位を融合する前提)。
    assert_eq!(vectors.chunk_count(), indexed.lexical.chunk_count());
    assert!(vectors.aligned_with(&indexed.lexical), "ベクトルの索引が BM25 側と揃うべき");
    assert_eq!(
        vectors.embedded_count(),
        vectors.chunk_count(),
        "固定コーパスは全チャンクにベクトルがあるべき"
    );
    let search = || HybridSearch { lexical: &indexed.lexical, vectors: Some(vectors) };
    let bm25 = bm25_report(&indexed.lexical);
    let embedding = evaluate(
        &HybridRetrieval::new(search(), Some(&embedder), SearchMethod::Embedding, Some("eval")),
        &cases(),
        CUTOFFS,
    );
    let hybrid = evaluate(
        &HybridRetrieval::new(search(), Some(&embedder), SearchMethod::Hybrid, Some("eval")),
        &cases(),
        CUTOFFS,
    );

    // 判定の第一の基準は取りこぼさないことである(EVAL
    // (uuid:72106edc-2c58-44d3-91c6-7b2a3dd7dfda) の「何を良いとするか」)。消費者は LLM で
    // 上位 10 件をまとめて読むので、10 件に入っていれば読める。ここは平均ではなく件数で
    // 縛る: 平均だと「1 問が圏外に落ちた」と「5 問が 2 位から 3 位へ下がった」が同じ大きさに
    // 見え、前者だけを止めたいのに後者でも落ちる(あるいはその逆になる)。
    //
    // 意味を見る 2 方式は、この固定コーパスの 19 対を 1 つも取りこぼさない。ここが 0 でなく
    // なる変更は、他の指標がどれだけ上がっても採らない。
    assert_eq!(
        embedding.misses(10),
        0,
        "埋め込みの取りこぼし(@10)\n{}",
        embedding.detail()
    );
    assert_eq!(hybrid.misses(10), 0, "融合の取りこぼし(@10)\n{}", hybrid.detail());
    // BM25 単独は語彙が隔たる 6 対に原理的に届かない(共有語が無い)。取りこぼしの件数として
    // それを固定しておくと、意味を見る方式が何を埋めているのかが件数で読める。
    assert_eq!(bm25.misses(10), 6, "BM25 単独の取りこぼし(@10)\n{}", bm25.detail());

    // 実測 2026-08-17(千分率)。基準線は「あるべき値」ではなく「今の値」であり、更新の
    // 手順は BM25 の基準線と同じ(失敗メッセージの内訳で順位の動きを目で確かめてから
    // リテラルを置き換える)。Recall@1 と MRR は記録するが、採否の材料にはしない
    // (基準の第 3 項)。
    //
    // 埋め込み単独: 語彙が隔たる 6 対すべてが上位 10 件に入り、5 対は 1 位。取りこぼすのは
    // 順位であって到達ではない(Recall@5 が 1.000)。落ちたのは、見出しにしかない句の対
    // (「監査証跡の保全」が 2 位)と識別子の対(「token_quarters」が 3 位)で、どちらも
    // 語の完全一致が効く場である。
    assert_eq!(
        per_mille(embedding.mean_recall(1)),
        842,
        "埋め込みの Recall@1 の基準線\n{}",
        embedding.detail()
    );
    assert_eq!(
        per_mille(embedding.mean_recall(5)),
        1000,
        "埋め込みの Recall@5 の基準線\n{}",
        embedding.detail()
    );
    assert_eq!(
        per_mille(embedding.mean_recall(10)),
        1000,
        "埋め込みの Recall@10 の基準線\n{}",
        embedding.detail()
    );
    assert_eq!(
        per_mille(embedding.mean_reciprocal_rank),
        896,
        "埋め込みの MRR の基準線\n{}",
        embedding.detail()
    );

    // 融合: 19 対のうち 17 対を 1 位に、残りも 5 件以内に入れる。
    //
    // この数値は、語の一致に被覆率を効かせた(crate::embed::MIN_FUSION_COVERAGE)ときに
    // 上がったものである。前の基準線は Recall@1 0.684・Recall@5 0.947・MRR 0.791 で、
    // 融合が落としていたのは、クエリ語の一部にしか当たっていない文書が BM25 側の上位に
    // 入り、意味検索の 1 位と同じ重みで先頭を争っていた対である。日本語のクエリは文字
    // bigram に切れるので、複合語の断片が別の語に当たる(「割り込み」と「取り込み」)。
    // 被覆の足りない順位を融合の入力から外すと、その混入が消えて 1 位が戻る。
    //
    // この 4 つの数値は融合の深さと定数(crate::embed::RRF_DEPTH と RRF_K)では動かない。
    // 深さ 10/30/50/100 と k 10/20/60 の 12 組すべてで同じ値だった(実測 2026-08-18。
    // RRF_DEPTH を 10 から 30 に、RRF_K を 60 から 20 に替えた変更でも 1 件も動いていない)。
    // 固定コーパスは 23 チャンクしかなく、BM25 が返す順位も 10 件に届かないので、深さを
    // 広げても融合の入力が増えないためである。深さと k の選定はこのハーネスではできず、
    // 実データ(仕様書 PDF 25 本・25,884 チャンク)の側で測った。ここが固定するのは
    // 「深さと k を替えても固定コーパスの順位は動かない」という現状の記録である。
    assert_eq!(
        per_mille(hybrid.mean_recall(1)),
        895,
        "融合の Recall@1 の基準線\n{}",
        hybrid.detail()
    );
    assert_eq!(
        per_mille(hybrid.mean_recall(5)),
        1000,
        "融合の Recall@5 の基準線\n{}",
        hybrid.detail()
    );
    assert_eq!(
        per_mille(hybrid.mean_recall(10)),
        1000,
        "融合の Recall@10 の基準線\n{}",
        hybrid.detail()
    );
    assert_eq!(
        per_mille(hybrid.mean_reciprocal_rank),
        932,
        "融合の MRR の基準線\n{}",
        hybrid.detail()
    );

    // 新しい二方式を足す意味は、BM25 単独より Recall@k が上がることにある(基準線を置く
    // 理由そのもの。EVAL (uuid:1109a04b-923e-4493-8f00-d704047d6a2a))。打ち切りのどこで
    // 比べても下回らず、少なくとも一つで上回ることを確かめる。
    for cutoff in CUTOFFS {
        for report in [&embedding, &hybrid] {
            assert!(
                report.mean_recall(*cutoff) >= bm25.mean_recall(*cutoff),
                "方式 {} の Recall@{cutoff} が BM25 を下回った\n{}\n{}",
                report.method,
                report.detail(),
                bm25.detail()
            );
        }
    }
    assert!(
        embedding.mean_recall(10) > bm25.mean_recall(10)
            && hybrid.mean_recall(10) > bm25.mean_recall(10),
        "意味を見る方式は BM25 単独の Recall@10 を上回るはず\n{}\n{}\n{}",
        bm25.detail(),
        embedding.detail(),
        hybrid.detail()
    );
    // 融合がこの固定コーパスでは埋め込み単独を上回る。被覆率を効かせる前は逆で
    // (埋め込み MRR 0.896 対 融合 0.791)、融合を押し下げていたのは、クエリ語の一部に
    // しか当たっていない文書が BM25 側の上位に混ざることだった。被覆の足りない順位を
    // 融合の入力から外すと、語の一致が効く対の 1 位を保ったまま、その混入だけが消える。
    //
    // それでも、実データの分布を代表する数値ではない(19 対のうち 6 対が語彙の隔たりの
    // 対で、語の一致が原理的に効かない場を多めに含む)。既定の方式をこの比較だけで
    // 決めない(EVAL (uuid:1109a04b-923e-4493-8f00-d704047d6a2a) の「既知の制約」)。
    assert!(
        hybrid.mean_reciprocal_rank > embedding.mean_reciprocal_rank,
        "被覆を効かせた後は融合の MRR が埋め込み単独を上回る記録\n{}\n{}",
        embedding.detail(),
        hybrid.detail()
    );
}

/// 完了条件の中身を対ごとに名指しする: BM25 が原理的に届かない 6 対
/// (VOCABULARY_GAP_CASES)が、意味を見る方式では上位 10 件に入る。基準線の平均が動いた
/// 理由はここにある。
///
/// 順位そのものはリテラルで固定しない。同じ入力でもバッチの構成が変わると埋め込みは
/// 1e-4 級でずれる(GPU の畳み込み順序)ので、僅差の順位は揺れうるためである。固定
/// するのは「10 件以内に入る」という到達の事実で、実測の順位はコメントに残す。
#[test]
fn the_vocabulary_gap_pairs_that_semantics_reaches() {
    let embedder = common::require_embedding_server();
    let indexed = indexed_corpus_with("gap-semantics", Some(&embedder), CORPUS);
    let vectors = indexed.vectors.as_ref().expect("ベクトルの索引");
    let search = || HybridSearch { lexical: &indexed.lexical, vectors: Some(vectors) };
    let embedding = evaluate(
        &HybridRetrieval::new(search(), Some(&embedder), SearchMethod::Embedding, Some("eval")),
        &cases(),
        CUTOFFS,
    );
    let hybrid = evaluate(
        &HybridRetrieval::new(search(), Some(&embedder), SearchMethod::Hybrid, Some("eval")),
        &cases(),
        CUTOFFS,
    );
    // 実測 2026-08-17 の順位(埋め込み単独 / 融合):
    //   言い換え「急に電気が消えた…」 1 位 / 3 位
    //   言い換え「ネットワークが混まない…」 1 位 / 4 位
    //   上位語と下位語「どの暗号方式を…」 1 位 / 3 位
    //   多言語(英語の問いで日本語の本文)「how long are request logs kept」 1 位 / 2 位
    //   質問文の形「どうすればノードを安全に落とせるか」 5 位 / 9 位
    //   多言語(日本語の問いで英語の本文)「時計がずれても大丈夫か」 1 位 / 1 位
    // BM25 は 6 対とも圏外で、うち「時計がずれても大丈夫か」は空振りだった。
    for (query, relevant) in VOCABULARY_GAP_CASES {
        for report in [&embedding, &hybrid] {
            let outcome = report.outcome_of(query);
            assert!(
                outcome.first_relevant_rank.is_some(),
                "方式 {} で、問い {query:?} の正解 {relevant} が上位 10 件に入らなかった\n{}",
                report.method,
                report.detail()
            );
        }
    }
}

/// 語彙が隔たる 6 対は、現在の BM25 では一つも上位 10 件に入らない(圏外)。基準線の
/// 平均が飽和から下がった理由をここで名指しする。
///
/// 取りこぼしの形は 2 通りある。実測 2026-08-17 では、6 対のうち 5 対は別の節が返り
/// (語の一致だけを見ると別の節の方が近く見える)、日本語の問いで英語の本文を引く
/// 1 対(「時計がずれても大丈夫か」)はコーパス全体と語を共有せず空振りになる。どちらも
/// クエリ自体は索引語を持つので、生産経路の POST /v1/search が 400 で撥ねる形
/// (索引語が 1 語も無いクエリ)ではない。それも下でいっしょに確かめる。
#[test]
fn bm25_cannot_reach_any_vocabulary_gap_pair() {
    let index = indexed_corpus("gap");
    let report = bm25_report(&index);
    for (query, relevant) in VOCABULARY_GAP_CASES {
        let outcome = report.outcome_of(query);
        assert_eq!(
            outcome.first_relevant_rank, None,
            "問い {query:?} の正解 {relevant} は現在の BM25 では圏外のはず\n{}",
            report.detail()
        );
        assert!(
            !terms_of(query).is_empty(),
            "問い {query:?} は索引語を持つはず(語の無いクエリは POST /v1/search が 400 で\
             撥ねるので、生産経路の呼び手が投げられない問いを測ることになる)"
        );
    }
}

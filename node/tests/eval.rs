//! 評価ハーネスの回帰テスト(RAG (uuid:8912f7c0-05fd-464d-9cba-4db8a5d30527) の項 3)。
//! 固定の小コーパスと「クエリ → 正解チャンク」対で、現在の検索方式の数値を基準線として
//! 固定する。コーパスはソースとは別ファイル(should/0112)。
//!
//! 検索層(Store + SearchIndex)を直に駆動する。ハーネスが測るのは順位付けであって
//! HTTP の往復ではないので、serve を起こさずに済ませている(API そのものの検証は
//! node/tests/search.rs の担当)。

use std::path::PathBuf;
use uniqnode::eval::{
    chunk_name, evaluate, per_mille, Bm25Retrieval, EvalCase, EvalReport, Retrieval,
};
use uniqnode::ingest::{chunk_markdown, ingest_document, DocumentInput};
use uniqnode::search::{terms_of, SearchIndex};
use uniqnode::store::{Store, StoreConfig};

/// 固定の小コーパス(文書名, 本文)。文書名は資材のファイル名から拡張子を除いたもので、
/// 正解チャンクの名前 `<文書>#<chunks 列の添字>` の前半になる。3 文書はそれぞれ
/// 日本語・英語・識別子の場を持ち、日本語と英語の文書は先頭に列挙(目次)の節を持つ。
const CORPUS: &[(&str, &str)] = &[
    ("eval_ja", include_str!("assets/eval_ja.md")),
    ("eval_en", include_str!("assets/eval_en.md")),
    ("eval_api", include_str!("assets/eval_api.md")),
];

/// 「クエリ → 正解チャンク」対。正解はチャンクの名前(`<文書>#<添字>`)で書く。
/// チャンクのオブジェクト ID は内容ハッシュなので、コーパスの本文を一字直すだけで
/// 変わり、固定資材に書けないためである。添字は chunk_markdown が見出しごとに切った
/// 順で、各文書の 0 番は列挙(目次)の節である。
///
/// [目次] と書いた対は、実データで観察された難所(SEARCH
/// (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580) の「既知の癖」)の再現である: クエリの句が
/// 目次(列挙)側と定義(本文)側の両方に現れ、正解は定義側にある。
const CASES: &[(&str, &str)] = &[
    // 日本語。
    ("世代の整合", "eval_ja#1"),                 // [目次] 同じ句が目次にもある
    ("保持表明の書式", "eval_ja#2"),             // [目次] 同じ句が目次にもある
    ("破損した区間の取り直し", "eval_ja#3"),     // 見出しとは違う言い回し
    ("機会層の退避", "eval_ja#4"),               // [目次] 同じ句が目次にもある
    ("索引はいつ作るのか", "eval_ja#5"),         // 見出しとは違う言い回し
    // 英語。
    ("saturates repeated term frequency", "eval_en#1"),
    ("cosine similarity between vectors", "eval_en#2"),
    ("reciprocal rank fusion", "eval_en#3"),     // [目次] 同じ句が Contents にもある
    // 識別子(1 語まるごとの完全一致)と、日本語に識別子が混じる場合。
    ("CHUNK_TOKEN_LIMIT", "eval_api#0"),
    ("token_estimate", "eval_api#1"),
    ("pdftotext の版", "eval_api#3"),
];

/// 評価する打ち切り k。
const CUTOFFS: &[usize] = &[1, 5, 10];

fn cases() -> Vec<EvalCase> {
    CASES.iter().map(|(query, relevant)| EvalCase::new(query, &[*relevant])).collect()
}

/// コーパスを一時ストアへ取り込んで索引を作る。索引は Store を借りない導出データ
/// なので、作った後はストアもディレクトリも畳んでよい。
fn indexed_corpus(name: &str) -> SearchIndex {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("uniqnode-eval-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
    let mut store = Store::open(StoreConfig::new(&dir)).expect("open store");
    for (document, text) in CORPUS {
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
            },
        )
        .expect("ingest");
    }
    let index = SearchIndex::build(&store).expect("build index");
    drop(store);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    index
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
/// 語の切り方だけは索引と同じ terms_of を呼ぶ(should/0135。切り方まで別実装に
/// すると、方式の差なのか語の切り方の差なのかが読めなくなる)。順位付けは持たない
/// ので、BM25 の得点計算を写した第二実装にはなっていない。
struct CorpusOrder {
    chunks: Vec<CorpusChunk>,
}

impl CorpusOrder {
    /// コーパスを索引と同じ切り方(chunk_markdown → terms_of)で読み込む。
    fn over_corpus() -> CorpusOrder {
        let mut chunks = Vec::new();
        for (document, text) in CORPUS {
            for (position, chunk) in chunk_markdown(text).iter().enumerate() {
                chunks.push(CorpusChunk {
                    name: chunk_name(document, position),
                    terms: terms_of(&chunk.text),
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
/// 数値は「あるべき値」ではなく「今の値」である。目次が定義より上位に来る難所
/// (SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580) の「既知の癖」)はここでも
/// 再現しており、Recall@1 はそのぶん低い。このテストの仕事は目標達成の判定ではなく、
/// 順位が黙って悪化しないことの検出である(カバレッジを計測のみとしてゲートにしない
/// TESTING (uuid:267326f7-e919-48f2-9737-fe0c0daec9d5) の方針と同じ扱い)。
///
/// 基準線を意図して動かすときの手順: (1) このテストを走らせ、失敗メッセージの内訳で
/// クエリごとの順位が意図どおりに動いたことを目で確かめる、(2) その実測値でリテラルを
/// 置き換える、(3) 何を変えたから動いたのかをコミットメッセージに書く。先に数値を
/// 書いてから実装を合わせにいかない。
#[test]
fn bm25_holds_the_recorded_baseline_on_the_fixed_corpus() {
    let index = indexed_corpus("baseline");
    let report = bm25_report(&index);
    assert_eq!(report.queries.len(), CASES.len(), "全対が評価されるべき");
    // 実測 2026-08-16(千分率)。11 対中 7 対が 1 位、残る 4 対は 2 位で、その 4 対は
    // すべて目次の節に先を越されている。5 位までには全対が入る。
    assert_eq!(per_mille(report.mean_recall(1)), 636, "Recall@1 の基準線\n{}", report.detail());
    assert_eq!(per_mille(report.mean_recall(5)), 1000, "Recall@5 の基準線\n{}", report.detail());
    assert_eq!(per_mille(report.mean_recall(10)), 1000, "Recall@10 の基準線\n{}", report.detail());
    assert_eq!(per_mille(report.mean_reciprocal_rank), 818, "MRR の基準線\n{}", report.detail());
}

/// 完了条件(RAG の項 3「検索方式を切り替えて数値が比較できる」)を実際の検索で示す:
/// 同じコーパスと同じ対のまま方式だけを差し替え、両方の数値を並べて比べる。対照は
/// 順位を付けない下限なので、この差が BM25 の順位付けが稼いでいるぶんである。
/// 対照の数値も基準線として固定する(更新手順は BM25 の基準線と同じ)。
#[test]
fn switching_the_method_changes_the_numbers_on_the_same_pairs() {
    let index = indexed_corpus("switch");
    let bm25 = bm25_report(&index);
    let floor = evaluate(&CorpusOrder::over_corpus(), &cases(), CUTOFFS);
    assert_eq!(floor.method, "corpus-order");
    // 実測 2026-08-16(千分率)。順位を付けないだけで Recall@1 は 0.636 から 0.364 へ、
    // MRR は 0.818 から 0.667 へ落ちる。Recall@10 が下がらないのは、このコーパスでは
    // 一致するチャンクが 10 件に収まるため(取りこぼしではなく規模の話)。
    assert_eq!(
        per_mille(floor.mean_recall(1)),
        364,
        "対照の Recall@1 の基準線\n{}",
        floor.detail()
    );
    assert_eq!(
        per_mille(floor.mean_recall(10)),
        1000,
        "対照の Recall@10 の基準線\n{}",
        floor.detail()
    );
    assert_eq!(
        per_mille(floor.mean_reciprocal_rank),
        667,
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

/// 難所の再現(実データで観察された癖): 同じ句が目次と定義の両方にあるとき、今は
/// 目次の節が定義の節より上位に来る。これは達成すべき目標ではなく現状の記録であり、
/// 検索が改良されて定義が 1 位になったら、この期待値も実測に合わせて更新する
/// (そのとき Recall@1 の基準線も一緒に上がる)。
#[test]
fn the_table_of_contents_still_outranks_the_definition_today() {
    let index = indexed_corpus("contents");
    let report = bm25_report(&index);
    let outcome = report.outcome_of("世代の整合");
    assert_eq!(
        outcome.ranked.first().map(String::as_str),
        Some("eval_ja#0"),
        "目次の節が 1 位のはず\n{}",
        report.detail()
    );
    // 実測 2026-08-16: 定義の節は 2 位。目次の節は短いぶん文書長正規化で得をし、
    // 同じ句を 1 度ずつしか含まない両者のうち短い方が上に来る。
    assert_eq!(outcome.first_relevant_rank, Some(2), "定義の節が来る順位\n{}", report.detail());
}

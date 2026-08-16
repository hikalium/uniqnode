//! 評価ハーネス: 検索方式を差し替えて Recall@k と MRR を比べる
//! (RAG (uuid:8912f7c0-05fd-464d-9cba-4db8a5d30527) の項 3)。
//!
//! この層が持つのは指標の計算(純関数)と方式の差し替え点だけで、固定の小コーパスと
//! 「クエリ → 正解チャンク」対は資材の側にある(node/tests/assets/eval_*.md と
//! node/tests/eval.rs)。現時点で載っている方式は BM25 だけだが、埋め込みと RRF 融合
//! (RAG の項 4)は Retrieval を実装して同じ evaluate に載り、同じ対で数値を比べられる。
//!
//! 指標は順位だけを見る(得点は見ない)。SEARCH
//! (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580)の score_semantics のとおり、BM25 の得点は
//! 同一応答内の順位付けにしか意味を持たず、方式をまたいで比べられないためである。

use crate::search::SearchIndex;

/// 評価の中でチャンクを名指す名前(この作り方の家はここだけ。should/0135)。引用
/// (INGEST (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の「文書モデル」節)の document と
/// position から `<文書>#<chunks 列の添字>` を組む。
///
/// チャンクのオブジェクト ID(内容ハッシュ)を使わないのは、コーパスの本文を一字直す
/// だけで ID が変わり、固定資材に正解として書けなくなるからである。位置で名指せば、
/// どの方式が返した結果も同じ名前で突き合わせられる。
pub fn chunk_name(document: &str, position: usize) -> String {
    format!("{document}#{position}")
}

/// 検索方式(差し替え点)。クエリを受け取り、順位付きのチャンク名の列(先頭が 1 位、
/// 重複なし)を返す。ハーネスはこの形しか要求しないので、BM25 でも、埋め込みでも、
/// 両者を RRF で融合したものでも、同じ対と同じ指標で比べられる。
pub trait Retrieval {
    /// 報告に出る方式名(どの数値がどの方式のものかを失敗時に言うため。should/0125)。
    fn name(&self) -> &str;
    /// 上位 top_k 件のチャンク名を順位順に返す。
    fn ranked(&self, query: &str, top_k: usize) -> Vec<String>;
}

/// BM25(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))を差し替え点に載せる被せ物。
/// 得点は捨てて順位だけを渡す。
pub struct Bm25Retrieval<'a> {
    index: &'a SearchIndex,
    /// 絞り込むコレクション(None なら全コレクション)。
    collection: Option<&'a str>,
}

impl<'a> Bm25Retrieval<'a> {
    pub fn new(index: &'a SearchIndex, collection: Option<&'a str>) -> Bm25Retrieval<'a> {
        Bm25Retrieval { index, collection }
    }
}

impl Retrieval for Bm25Retrieval<'_> {
    fn name(&self) -> &str {
        "bm25"
    }

    fn ranked(&self, query: &str, top_k: usize) -> Vec<String> {
        self.index
            .search(query, self.collection, top_k)
            .into_iter()
            .map(|hit| chunk_name(&hit.chunk.document, hit.chunk.position))
            .collect()
    }
}

/// 評価 1 件: クエリと、その正解チャンクの名前(複数可)。
pub struct EvalCase {
    pub query: String,
    pub relevant: Vec<String>,
}

impl EvalCase {
    /// 資材の表(クエリと正解の名前)から対を組む。
    pub fn new(query: &str, relevant: &[&str]) -> EvalCase {
        EvalCase {
            query: query.to_string(),
            relevant: relevant.iter().map(|name| (*name).to_string()).collect(),
        }
    }
}

/// Recall@k(指標の家はここだけ。should/0135): 上位 k 件に入った正解の割合。正解を
/// 持たないクエリは 0(評価の対象にならない)。
pub fn recall_at_k(ranked: &[String], relevant: &[String], k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    let found = relevant
        .iter()
        .filter(|name| ranked.iter().take(k).any(|candidate| candidate == *name))
        .count();
    found as f64 / relevant.len() as f64
}

/// 最初の正解の順位(1 始まり)。返り値の中に一つも無ければ None。
pub fn first_relevant_rank(ranked: &[String], relevant: &[String]) -> Option<usize> {
    ranked
        .iter()
        .position(|candidate| relevant.iter().any(|name| name == candidate))
        .map(|index| index + 1)
}

/// 逆順位(MRR の 1 クエリぶん): 最初の正解の順位の逆数。一つも無ければ 0。
pub fn reciprocal_rank(ranked: &[String], relevant: &[String]) -> f64 {
    match first_relevant_rank(ranked, relevant) {
        Some(rank) => 1.0 / rank as f64,
        None => 0.0,
    }
}

/// 指標を千分率の整数に丸める。基準線を f64 の等値比較ではなく整数のリテラルで固定
/// するためで、丸め方の家もここだけにする(should/0135)。
pub fn per_mille(value: f64) -> i64 {
    (value * 1000.0).round() as i64
}

/// クエリ 1 件の内訳(失敗時に読む。should/0125)。
pub struct QueryOutcome {
    pub query: String,
    pub relevant: Vec<String>,
    /// 方式が返した順位付きのチャンク名(先頭が 1 位)。
    pub ranked: Vec<String>,
    /// 最初の正解の順位(1 始まり)。圏外なら None。
    pub first_relevant_rank: Option<usize>,
    pub reciprocal_rank: f64,
    /// 打ち切り k ごとの Recall@k(EvalReport の cutoffs と同じ並び)。
    pub recalls: Vec<f64>,
}

/// 方式 1 つぶんの評価結果。
pub struct EvalReport {
    pub method: String,
    /// 評価した打ち切り k(与えられた並びのまま)。
    pub cutoffs: Vec<usize>,
    pub queries: Vec<QueryOutcome>,
    /// cutoffs と同じ並びの平均 Recall@k(クエリ 0 件なら 0)。
    pub mean_recalls: Vec<f64>,
    /// 平均逆順位(クエリ 0 件なら 0)。
    pub mean_reciprocal_rank: f64,
}

impl EvalReport {
    /// 平均 Recall@k。評価していない k を尋ねられたら黙って 0 を返さず、何を評価した
    /// のかを言って落ちる(must/0022)。
    pub fn mean_recall(&self, k: usize) -> f64 {
        let position = self
            .cutoffs
            .iter()
            .position(|cutoff| *cutoff == k)
            .unwrap_or_else(|| panic!("Recall@{k} は評価していない(評価した k: {:?})", self.cutoffs));
        self.mean_recalls[position]
    }

    /// クエリ 1 件の内訳を引く(基準線が動いたときに、どのクエリが動いたのかを名指し
    /// で調べるため)。
    pub fn outcome_of(&self, query: &str) -> &QueryOutcome {
        self.queries
            .iter()
            .find(|outcome| outcome.query == query)
            .unwrap_or_else(|| panic!("クエリ {query:?} は評価対象にない"))
    }

    /// 失敗時に読む内訳(should/0125: 失敗が修理の物語を語る)。平均のあとに、クエリ
    /// ごとの「正解が何位に来たか・正解は何か・方式が実際に返した並び」を 1 行ずつ出す。
    /// これだけ読めば、どのクエリのどの順位が動いて数値が変わったのかが分かる。
    pub fn detail(&self) -> String {
        let averages: Vec<String> = self
            .cutoffs
            .iter()
            .zip(&self.mean_recalls)
            .map(|(k, value)| format!("Recall@{k} {value:.3}"))
            .collect();
        let mut lines = vec![format!(
            "方式 {}: {} クエリ、MRR {:.3}、{}",
            self.method,
            self.queries.len(),
            self.mean_reciprocal_rank,
            averages.join("、")
        )];
        for outcome in &self.queries {
            let place = match outcome.first_relevant_rank {
                Some(rank) => format!("{rank} 位"),
                None => "圏外".to_string(),
            };
            let ranked = if outcome.ranked.is_empty() {
                "(空振り)".to_string()
            } else {
                outcome.ranked.join(" > ")
            };
            lines.push(format!(
                "  [{place}] {:?} 正解 {} / 返り値 {ranked}",
                outcome.query,
                outcome.relevant.join(", ")
            ));
        }
        lines.join("\n")
    }
}

/// 方式 1 つを対の集合で評価する。各クエリには打ち切りの最大値ぶんだけ返してもらい
/// (それより下の順位はどの指標にも効かない)、Recall@k と逆順位を計算して平均する。
pub fn evaluate(method: &dyn Retrieval, cases: &[EvalCase], cutoffs: &[usize]) -> EvalReport {
    let top_k = cutoffs.iter().copied().max().unwrap_or(0);
    let mut queries = Vec::with_capacity(cases.len());
    for case in cases {
        let ranked = method.ranked(&case.query, top_k);
        queries.push(QueryOutcome {
            query: case.query.clone(),
            relevant: case.relevant.clone(),
            first_relevant_rank: first_relevant_rank(&ranked, &case.relevant),
            reciprocal_rank: reciprocal_rank(&ranked, &case.relevant),
            recalls: cutoffs.iter().map(|k| recall_at_k(&ranked, &case.relevant, *k)).collect(),
            ranked,
        });
    }
    let count = queries.len() as f64;
    let mean = |total: f64| if queries.is_empty() { 0.0 } else { total / count };
    let mean_recalls = cutoffs
        .iter()
        .enumerate()
        .map(|(index, _)| mean(queries.iter().map(|outcome| outcome.recalls[index]).sum()))
        .collect();
    EvalReport {
        method: method.name().to_string(),
        cutoffs: cutoffs.to_vec(),
        mean_reciprocal_rank: mean(queries.iter().map(|outcome| outcome.reciprocal_rank).sum()),
        mean_recalls,
        queries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|name| (*name).to_string()).collect()
    }

    /// 順位付きの返り値を表で決め打ちする試験用の方式。検索の実装には触れないので、
    /// 指標とハーネスの側だけを検査できる。
    struct FixedRanking {
        label: &'static str,
        ranking: Vec<String>,
    }

    impl Retrieval for FixedRanking {
        fn name(&self) -> &str {
            self.label
        }
        fn ranked(&self, _query: &str, top_k: usize) -> Vec<String> {
            self.ranking.iter().take(top_k).cloned().collect()
        }
    }

    /// 指標の期待値はリテラルで書く(検査対象から導出しない。should/0137)。
    #[test]
    fn recall_and_reciprocal_rank_match_hand_computed_literals() {
        let ranked = names(&["a", "b", "c", "d", "e"]);
        // 正解 1 件が 3 位: 上位 1・2 件には入らず、3 件目で入る。
        let one = names(&["c"]);
        assert_eq!(recall_at_k(&ranked, &one, 1), 0.0);
        assert_eq!(recall_at_k(&ranked, &one, 2), 0.0);
        assert_eq!(recall_at_k(&ranked, &one, 3), 1.0);
        assert_eq!(first_relevant_rank(&ranked, &one), Some(3));
        assert!((reciprocal_rank(&ranked, &one) - 1.0 / 3.0).abs() < 1e-12);
        // 正解 2 件(3 位と 5 位): Recall は「上位 k に入った正解の割合」なので
        // k=3 では半分、k=5 で全部。逆順位は最初の正解だけを見る。
        let two = names(&["c", "e"]);
        assert_eq!(recall_at_k(&ranked, &two, 1), 0.0);
        assert_eq!(recall_at_k(&ranked, &two, 3), 0.5);
        assert_eq!(recall_at_k(&ranked, &two, 5), 1.0);
        assert_eq!(first_relevant_rank(&ranked, &two), Some(3));
        assert!((reciprocal_rank(&ranked, &two) - 1.0 / 3.0).abs() < 1e-12);
        // 圏外の正解: Recall は 0、逆順位も 0(1/∞ ではなく 0 と決める)。
        let missing = names(&["z"]);
        assert_eq!(recall_at_k(&ranked, &missing, 5), 0.0);
        assert_eq!(first_relevant_rank(&ranked, &missing), None);
        assert_eq!(reciprocal_rank(&ranked, &missing), 0.0);
        // k が返り値より長くても、無い順位を数えない。
        assert_eq!(recall_at_k(&ranked, &one, 100), 1.0);
        // 空振り(返り値なし)は 0。
        assert_eq!(recall_at_k(&[], &one, 10), 0.0);
        assert_eq!(reciprocal_rank(&[], &one), 0.0);
        // 千分率の丸め(基準線をリテラルの整数で書くための丸め方)。
        assert_eq!(per_mille(1.0), 1000);
        assert_eq!(per_mille(0.0), 0);
        assert_eq!(per_mille(1.0 / 3.0), 333);
        assert_eq!(per_mille(5.0 / 12.0), 417);
    }

    /// 完了条件(RAG の項 3)の形: 同じ対のまま方式を差し替えると、数値が方式ごとに
    /// 出て比べられる。期待値はリテラル(should/0137)。
    #[test]
    fn switching_the_method_produces_comparable_numbers() {
        let cases = [EvalCase::new("q1", &["a"]), EvalCase::new("q2", &["b"])];
        let cutoffs = [1usize, 2, 3];
        // 前寄せの方式: q1 の正解が 1 位、q2 の正解が 2 位。
        let front = FixedRanking { label: "front", ranking: names(&["a", "b", "c"]) };
        let front = evaluate(&front, &cases, &cutoffs);
        assert_eq!(front.method, "front");
        // MRR = (1/1 + 1/2)/2 = 0.75、Recall@1 = (1+0)/2 = 0.5、Recall@2 = 1。
        assert_eq!(per_mille(front.mean_reciprocal_rank), 750);
        assert_eq!(per_mille(front.mean_recall(1)), 500);
        assert_eq!(per_mille(front.mean_recall(2)), 1000);
        assert_eq!(per_mille(front.mean_recall(3)), 1000);
        // 後ろ寄せの方式: 同じ対で q1 の正解が 3 位、q2 の正解が 2 位。
        let back = FixedRanking { label: "back", ranking: names(&["c", "b", "a"]) };
        let back = evaluate(&back, &cases, &cutoffs);
        // MRR = (1/3 + 1/2)/2 = 5/12 ≒ 0.417、Recall@1 = 0、Recall@2 = 0.5。
        assert_eq!(per_mille(back.mean_reciprocal_rank), 417);
        assert_eq!(per_mille(back.mean_recall(1)), 0);
        assert_eq!(per_mille(back.mean_recall(2)), 500);
        assert_eq!(per_mille(back.mean_recall(3)), 1000);
        assert!(
            front.mean_reciprocal_rank > back.mean_reciprocal_rank,
            "正解を上位に置く方式の MRR が高いはず"
        );
    }

    /// 失敗時に読む内訳が、クエリ・正解・実際の並び・正解の順位を名指しで語る
    /// (should/0125)。これが無いと、数値が動いたときにどのクエリが動いたのかを
    /// 読み手が再発見しなければならない。
    #[test]
    fn the_detail_names_the_query_the_answer_and_where_it_landed() {
        let cases = [EvalCase::new("問い", &["doc#2"]), EvalCase::new("圏外の問い", &["doc#9"])];
        let method = FixedRanking { label: "fixed", ranking: names(&["doc#0", "doc#2"]) };
        let report = evaluate(&method, &cases, &[1usize, 2]);
        let detail = report.detail();
        assert!(detail.contains("方式 fixed"), "{detail}");
        assert!(detail.contains("MRR 0.250"), "平均が読めるべき: {detail}");
        assert!(detail.contains("Recall@1 0.000"), "{detail}");
        assert!(detail.contains("Recall@2 0.500"), "{detail}");
        assert!(detail.contains("[2 位] \"問い\" 正解 doc#2"), "{detail}");
        assert!(detail.contains("返り値 doc#0 > doc#2"), "実際の並びが読めるべき: {detail}");
        assert!(detail.contains("[圏外] \"圏外の問い\" 正解 doc#9"), "{detail}");
        // 内訳の引き当ても名前で行える。
        assert_eq!(report.outcome_of("問い").first_relevant_rank, Some(2));
        assert_eq!(report.outcome_of("圏外の問い").first_relevant_rank, None);
    }
}

//! 文字 bigram の転置索引と BM25 検索(SEARCH
//! (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))。
//!
//! 索引は導出データ(I4)であり、ReferrerIndex(node/src/store.rs)と同じ遅延構築と
//! する。Store::open には触れない(open はオブジェクトをパースせずハッシュだけを見る
//! 唯一の共有経路であり、そこに全件パースを足すと壊れたオブジェクト 1 個でストアが
//! 開かなくなる)。
//!
//! 索引対象は「見え」だけ: collections/ 配下の各 ref が指す現行 doc_rev の chunks のみ
//! (ASSERTIONS (uuid:c05379e2-2d30-41bc-8342-62103d94bb21) の原理 5。既定の読み手は
//! 見えだけを読む。旧版・訂正済みの言明は ref から辿れないので索引に入らない)。

use crate::c1::{self, Value};
use crate::store::{Result, Store};
use std::collections::BTreeMap;

/// BM25 の tf 飽和の強さ(Okapi の慣用値)。
const BM25_K1: f64 = 1.2;
/// BM25 の文書長正規化の強さ(Okapi の慣用値)。
const BM25_B: f64 = 0.75;

/// 応答スニペットの長さ上限(文字数)。チャンク全文は GET /v1/objects/{id} で取れる
/// ため、応答には先頭だけを載せる。
const SNIPPET_CHAR_LIMIT: usize = 200;

/// 本文を索引語の列に切る(索引の構築と問い合わせが共用する唯一の実装。should/0135)。
///
/// - 全体を小文字化した上で、ASCII 英数字と `_` の連なりは 1 語まるごと 1 語にする
///   (token_estimate のような識別子・英単語の完全一致のため。`_` を語に含めるので、
///   識別子は途中で割れない)。
/// - 非 ASCII の英数字(仮名・漢字など。判定は char::is_alphanumeric)の連なりは文字
///   bigram にする。連なりが 1 文字だけなら、その 1 文字を語にする。
/// - それ以外(空白・記号・句読点)は区切りで、語にならない。
pub fn terms_of(text: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Class {
        Word,
        Wide,
        Separator,
    }
    fn class_of(character: char) -> Class {
        if character.is_ascii_alphanumeric() || character == '_' {
            Class::Word
        } else if character.is_alphanumeric() {
            Class::Wide
        } else {
            Class::Separator
        }
    }
    fn flush(run: &mut Vec<char>, class: Class, terms: &mut Vec<String>) {
        match class {
            Class::Word => terms.push(run.iter().collect()),
            Class::Wide => {
                if run.len() == 1 {
                    terms.push(run[0].to_string());
                } else {
                    for pair in run.windows(2) {
                        terms.push(pair.iter().collect());
                    }
                }
            }
            Class::Separator => {}
        }
        run.clear();
    }
    let mut terms = Vec::new();
    let mut run: Vec<char> = Vec::new();
    let mut run_class = Class::Separator;
    for character in text.to_lowercase().chars() {
        let class = class_of(character);
        if class != run_class {
            flush(&mut run, run_class, &mut terms);
            run_class = class;
        }
        if class != Class::Separator {
            run.push(character);
        }
    }
    flush(&mut run, run_class, &mut terms);
    terms
}

/// BM25 の 1 語 1 チャンクぶんの得点(純関数。数式の家はここだけ。should/0135)。
/// idf は負にならない +1 の形 ln(1 + (N - df + 0.5)/(df + 0.5))、全体は
/// idf × tf(k1+1) / (tf + k1(1 - b + b·len/avg))。文書長が平均と等しく tf=1 のとき
/// 飽和と正規化が打ち消し合い、得点は idf に一致する。
fn bm25_term_score(
    term_frequency: u32,
    document_frequency: usize,
    chunk_count: usize,
    length: usize,
    average_length: f64,
) -> f64 {
    let total = chunk_count as f64;
    let with_term = document_frequency as f64;
    let idf = (1.0 + (total - with_term + 0.5) / (with_term + 0.5)).ln();
    let tf = f64::from(term_frequency);
    let normalizer = BM25_K1 * (1.0 - BM25_B + BM25_B * length as f64 / average_length);
    idf * tf * (BM25_K1 + 1.0) / (tf + normalizer)
}

/// チャンク本文の先頭適量(文字境界で切る)。
fn snippet_of(text: &str) -> String {
    match text.char_indices().nth(SNIPPET_CHAR_LIMIT) {
        Some((offset, _)) => text[..offset].to_string(),
        None => text.to_string(),
    }
}

/// 索引済みチャンク 1 件。引用は doc_rev 側から組む(INGEST
/// (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の「文書モデル」節): document は
/// ref パスから collections/<コレクション名>/ を除いた残り、position は chunks 列の
/// 添字、見出しは meta.breadcrumbs、PDF はさらに meta.page。
pub struct IndexedChunk {
    /// チャンクのオブジェクト ID(全文の取得は既存の GET /v1/objects/{id})。
    pub id: String,
    pub collection: String,
    pub document: String,
    pub position: usize,
    pub snippet: String,
    pub breadcrumbs: Vec<String>,
    pub page: Option<u32>,
    /// このチャンクの語数(BM25 の文書長)。
    term_count: usize,
}

/// 転置索引の 1 項目: チャンク番号(chunks 列の添字)と、そのチャンク内の出現数。
#[derive(Clone)]
struct Posting {
    chunk: usize,
    term_frequency: u32,
}

/// 構築時点の世代。オブジェクトは追記専用、reflog は署名者ごとに seq 単調なので、
/// 両方が現在値と一致することが「構築後に書き込みも ref の変化も無い」ことと同値。
/// ReferrerIndex の object_count だけでは足りない: 文書の張り替え(既存 doc_rev への
/// 巻き戻し)や tombstone は ref レコードしか増やさず、旧版のチャンクが索引に残る。
struct Generation {
    object_count: usize,
    signers: Vec<(String, u64)>,
}

/// 検索結果 1 件(得点と索引済みチャンクへの参照)。
pub struct SearchHit<'a> {
    pub score: f64,
    pub chunk: &'a IndexedChunk,
}

pub struct SearchIndex {
    generation: Generation,
    chunks: Vec<IndexedChunk>,
    /// 索引語 → 位置表(チャンク番号昇順)。
    postings: BTreeMap<String, Vec<Posting>>,
    /// 索引済みチャンクの平均語数(BM25 の文書長正規化の基準)。
    average_length: f64,
}

/// ストア上のオブジェクトを c1 として読む。無い(他ノードの ref の複製前)・UTF-8 で
/// ない・c1 でないは None(索引は導出データであり、壊れた 1 個で構築全体を失敗させ
/// ない。ReferrerIndex と同じ扱い)。入出力の失敗だけは伝える。
fn read_c1(store: &Store, id: &str) -> Result<Option<Value>> {
    let Some(bytes) = store.get_object(id)? else { return Ok(None) };
    let Ok(text) = String::from_utf8(bytes) else { return Ok(None) };
    Ok(c1::parse(&text).ok())
}

impl SearchIndex {
    /// 見えの全チャンクを一度走査して構築する。ref は完全名 <署名者>/<パス> で並ぶが、
    /// 索引対象はパスが collections/ 配下のものだけ(annotations/ などの ref は文書の
    /// 見えではない)。tombstone と、doc_rev や chunk の形が崩れているものは飛ばす。
    pub fn build(store: &Store) -> Result<SearchIndex> {
        let generation =
            Generation { object_count: store.object_count(), signers: store.signers() };
        let mut chunks: Vec<IndexedChunk> = Vec::new();
        let mut postings: BTreeMap<String, Vec<Posting>> = BTreeMap::new();
        let mut total_terms = 0usize;
        for (name, state) in store.list_refs() {
            // tombstone は現在の見えに無い(原理 5)。
            let Some(target) = &state.target else { continue };
            let Some((_signer, path)) = name.split_once('/') else { continue };
            let Some(rest) = path.strip_prefix("collections/") else { continue };
            let Some((collection, document)) = rest.split_once('/') else { continue };
            let Some(Value::Object(doc_rev)) = read_c1(store, target)? else { continue };
            let Some(Value::Array(chunk_ids)) = doc_rev.get("chunks") else { continue };
            for (position, chunk_ref) in chunk_ids.iter().enumerate() {
                let Value::Text(chunk_id) = chunk_ref else { continue };
                let Some(Value::Object(chunk)) = read_c1(store, chunk_id)? else { continue };
                let Some(Value::Text(text)) = chunk.get("text") else { continue };
                let terms = terms_of(text);
                if terms.is_empty() {
                    // 語の無いチャンクはどのクエリにも一致しない。
                    continue;
                }
                let (breadcrumbs, page) = match chunk.get("meta") {
                    Some(Value::Object(meta)) => {
                        let breadcrumbs = match meta.get("breadcrumbs") {
                            Some(Value::Array(items)) => items
                                .iter()
                                .filter_map(|item| match item {
                                    Value::Text(t) => Some(t.clone()),
                                    _ => None,
                                })
                                .collect(),
                            _ => Vec::new(),
                        };
                        let page = match meta.get("page") {
                            Some(Value::Integer(n)) => u32::try_from(*n).ok(),
                            _ => None,
                        };
                        (breadcrumbs, page)
                    }
                    _ => (Vec::new(), None),
                };
                let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
                for term in &terms {
                    *counts.entry(term).or_insert(0) += 1;
                }
                let index = chunks.len();
                for (term, term_frequency) in counts {
                    // 走査はチャンク番号の昇順なので、各語の位置表も昇順で積み上がる。
                    postings
                        .entry(term.to_string())
                        .or_default()
                        .push(Posting { chunk: index, term_frequency });
                }
                total_terms += terms.len();
                chunks.push(IndexedChunk {
                    id: chunk_id.clone(),
                    collection: collection.to_string(),
                    document: document.to_string(),
                    position,
                    snippet: snippet_of(text),
                    breadcrumbs,
                    page,
                    term_count: terms.len(),
                });
            }
        }
        let average_length = if chunks.is_empty() {
            0.0
        } else {
            total_terms as f64 / chunks.len() as f64
        };
        Ok(SearchIndex { generation, chunks, postings, average_length })
    }

    /// この索引が store の現状について最新か。オブジェクト数と署名者ごとの最終 seq の
    /// 両方が一致する限り、書き込みも ref の変化も挟まっていない。
    pub fn is_current(&self, store: &Store) -> bool {
        self.generation.object_count == store.object_count()
            && self.generation.signers == store.signers()
    }

    /// 語 term の位置表。1 文字の非 ASCII 語は bigram の索引にそのままでは載らない
    /// (連なりの途中の文字は bigram の一部としてだけ現れる)ため、その文字を含む
    /// 全索引語の表を統合した擬似語として扱う。これが 1 文字クエリの意味である。
    /// 同じチャンク内で文字が複数の bigram に現れるぶん出現数は重複して数えるが、
    /// 順位付けにしか使わないので許す。
    fn postings_of(&self, term: &str) -> Vec<Posting> {
        let mut characters = term.chars();
        let (first, second) = (characters.next(), characters.next());
        let single_wide = second.is_none() && first.is_some_and(|c| !c.is_ascii());
        if !single_wide {
            return self.postings.get(term).cloned().unwrap_or_default();
        }
        let character = first.expect("1 文字ある");
        let mut merged: BTreeMap<usize, u32> = BTreeMap::new();
        for (indexed_term, postings) in &self.postings {
            if !indexed_term.contains(character) {
                continue;
            }
            for posting in postings {
                *merged.entry(posting.chunk).or_insert(0) += posting.term_frequency;
            }
        }
        merged
            .into_iter()
            .map(|(chunk, term_frequency)| Posting { chunk, term_frequency })
            .collect()
    }

    /// BM25 で top_k 件を返す。collection を指定するとそのコレクションだけに絞る
    /// (df は索引全体で数える。順位はどちらでも同一応答内でのみ意味を持つ)。
    /// 同点は索引順(ref 名の昇順 → chunks 列の順)で安定に決める(should/0125 の
    /// 決定性)。
    pub fn search(
        &self,
        query: &str,
        collection: Option<&str>,
        top_k: usize,
    ) -> Vec<SearchHit<'_>> {
        let mut query_terms = terms_of(query);
        query_terms.sort();
        query_terms.dedup();
        let mut scores: BTreeMap<usize, f64> = BTreeMap::new();
        for term in &query_terms {
            let postings = self.postings_of(term);
            if postings.is_empty() {
                continue;
            }
            let document_frequency = postings.len();
            for posting in &postings {
                let chunk = &self.chunks[posting.chunk];
                if collection.is_some_and(|wanted| wanted != chunk.collection.as_str()) {
                    continue;
                }
                *scores.entry(posting.chunk).or_insert(0.0) += bm25_term_score(
                    posting.term_frequency,
                    document_frequency,
                    self.chunks.len(),
                    chunk.term_count,
                    self.average_length,
                );
            }
        }
        let mut ranked: Vec<(usize, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(top_k);
        ranked
            .into_iter()
            .map(|(index, score)| SearchHit { score, chunk: &self.chunks[index] })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{chunk_markdown, ingest_document, DocumentInput, IngestOutcome};
    use crate::store::StoreConfig;
    use std::path::PathBuf;

    /// 索引語の切り方の期待値はリテラルで書く(検査対象から導出しない。should/0137)。
    #[test]
    fn terms_split_ascii_words_whole_and_wide_runs_into_bigrams() {
        // ASCII の語は 1 語まるごと(`_` を含むので識別子は割れない)。記号は区切り。
        assert_eq!(
            terms_of("pub fn token_estimate(text: &str)"),
            vec!["pub", "fn", "token_estimate", "text", "str"]
        );
        // 大文字は小文字化される。
        assert_eq!(terms_of("BM25 Search"), vec!["bm25", "search"]);
        // 非 ASCII の連なりは文字 bigram。
        assert_eq!(terms_of("世代整合の検証"), vec!["世代", "代整", "整合", "合の", "の検", "検証"]);
        // 句読点は区切り。1 文字だけの連なりはその 1 文字。
        assert_eq!(terms_of("です。次"), vec!["です", "次"]);
        assert_eq!(terms_of("鍵"), vec!["鍵"]);
        // ASCII と非 ASCII の境界でも連なりは切れる。
        assert_eq!(terms_of("第2章"), vec!["第", "2", "章"]);
        // 語が一つも無い入力は空。
        assert_eq!(terms_of("--- 、。"), Vec::<String>::new());
    }

    /// BM25 の期待値はリテラルで書く(検査対象から導出しない。should/0137)。文書長が
    /// 平均と等しく tf=1 のとき、飽和と正規化が打ち消し合って得点は idf に一致する。
    #[test]
    fn bm25_matches_hand_computed_literals() {
        // N=2, df=1, tf=1, len=avg: idf = ln((2-1+0.5)/(1+0.5)+1) = ln 2(数学定数。
        // clippy の approx_constant に従い std の定数で書くが、検査対象からの導出では
        // ない)。
        assert!((bm25_term_score(1, 1, 2, 10, 10.0) - std::f64::consts::LN_2).abs() < 1e-9);
        // N=3, df=1: idf = ln((3-1+0.5)/1.5+1) = ln(8/3)。
        assert!((bm25_term_score(1, 1, 3, 10, 10.0) - 0.9808292530117261).abs() < 1e-9);
        // 全チャンクに現れる語(df=N=2)でも idf は正: ln(1 + 0.5/2.5) = ln 1.2。
        assert!((bm25_term_score(1, 2, 2, 10, 10.0) - 0.1823215567939546).abs() < 1e-9);
        // tf=2, len=avg: ln 2 × (2×2.2)/(2+1.2) = ln 2 × 1.375(k1 を噛む)。
        assert!((bm25_term_score(2, 1, 2, 10, 10.0) - 0.9530773732699248).abs() < 1e-9);
        // tf=1, len=2×avg: ln 2 × 2.2/(1+1.2×(0.25+1.5)) = ln 2 × 2.2/3.1(b を噛む。
        // len=avg のケースは正規化項が k1 に退化して b の値に反応しない)。
        assert!((bm25_term_score(1, 1, 2, 20, 10.0) - 0.4919109023328644).abs() < 1e-9);
    }

    /// 数式の性質: tf の伸びは線形未満(飽和)で、平均より長いチャンクは割り引かれる。
    #[test]
    fn bm25_saturates_tf_and_penalizes_long_chunks() {
        let base = bm25_term_score(1, 1, 2, 10, 10.0);
        let doubled = bm25_term_score(2, 1, 2, 10, 10.0);
        assert!(doubled > base, "出現数が増えれば得点も増える");
        assert!(doubled < base * 2.0, "tf の伸びは線形未満(飽和)のはず");
        let long = bm25_term_score(1, 1, 2, 20, 10.0);
        assert!(long < base, "平均より長いチャンクは割り引かれるはず");
    }

    #[test]
    fn snippets_stop_at_the_character_limit_on_char_boundaries() {
        let text = "あ".repeat(SNIPPET_CHAR_LIMIT + 50);
        assert_eq!(snippet_of(&text).chars().count(), SNIPPET_CHAR_LIMIT);
        assert_eq!(snippet_of("short"), "short");
    }

    fn temp_store(name: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-search-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        let store = Store::open(StoreConfig::new(&dir)).expect("open store");
        (dir, store)
    }

    fn ingest_markdown(
        store: &mut Store,
        collection: &str,
        name: &str,
        text: &str,
    ) -> IngestOutcome {
        let chunks = chunk_markdown(text);
        ingest_document(
            store,
            &DocumentInput {
                collection,
                name,
                source: text.as_bytes(),
                media: "markdown",
                chunks: &chunks,
                extractor: None,
            },
        )
        .expect("ingest")
    }

    /// 見え(原理 5)と世代整合: 索引は collections/ 配下の ref が指す現行 doc_rev の
    /// チャンクだけを持ち、改版でも、オブジェクトの増えない ref だけの変化
    /// (tombstone)でも、世代がずれて作り直しになる。
    #[test]
    fn the_index_follows_the_visibility_and_detects_ref_only_changes() {
        let (dir, mut store) = temp_store("visibility");
        let outcome =
            ingest_markdown(&mut store, "notes", "memo", "# 章\n\n初版だけの合言葉。\n");
        // collections/ 配下でない ref は、同じ doc_rev を指していても索引対象ではない。
        store.set_ref("scratch/memo", Some(&outcome.doc_rev_id)).expect("set_ref");
        let index = SearchIndex::build(&store).expect("build");
        assert!(index.is_current(&store));
        let hits = index.search("合言葉", None, 10);
        assert_eq!(hits.len(), 1, "合言葉を含む現行チャンクだけが返るべき");
        assert_eq!(hits[0].chunk.document, "memo");
        assert_eq!(hits[0].chunk.collection, "notes");
        assert_eq!(hits[0].chunk.position, 0);
        assert_eq!(hits[0].chunk.breadcrumbs, vec!["章".to_string()]);

        // 改版: 旧版のチャンクは見えから消える。
        ingest_markdown(&mut store, "notes", "memo", "# 章\n\n改訂で言い換えた本文。\n");
        assert!(!index.is_current(&store), "改版後の索引を最新と誤認してはならない");
        let rebuilt = SearchIndex::build(&store).expect("rebuild");
        assert!(
            rebuilt.search("合言葉", None, 10).is_empty(),
            "旧版のチャンクが索引に残っている"
        );
        assert_eq!(rebuilt.search("改訂", None, 10).len(), 1);

        // ref の張り替えだけの変化(tombstone)はオブジェクトを増やさないが、世代
        // (署名者ごとの最終 seq)がずれて作り直しになる。
        let objects = store.object_count();
        store.set_ref("collections/notes/memo", None).expect("tombstone");
        assert_eq!(store.object_count(), objects, "tombstone はオブジェクトを増やさない");
        assert!(!rebuilt.is_current(&store), "ref だけの変化を世代が検出できていない");
        let after = SearchIndex::build(&store).expect("rebuild");
        assert!(after.search("改訂", None, 10).is_empty(), "tombstone 後も見えに残っている");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 1 文字の非 ASCII クエリは、その文字を含む索引語(bigram)の統合として検索でき、
    /// 文字が連なりの途中にあっても一致する。
    #[test]
    fn a_single_character_query_matches_occurrences_inside_runs() {
        let (dir, mut store) = temp_store("single-char");
        ingest_markdown(&mut store, "notes", "keys", "# 鍵\n\n暗号鍵を保管する。\n");
        ingest_markdown(&mut store, "notes", "other", "# 別\n\n関係の無い本文。\n");
        let index = SearchIndex::build(&store).expect("build");
        let hits = index.search("鍵", None, 10);
        assert_eq!(hits.len(), 1, "鍵を含むチャンクだけが返るべき");
        assert_eq!(hits[0].chunk.document, "keys");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 順位付け: ありふれた語と希少語の両方を含むチャンクが、ありふれた語だけの
    /// チャンクより上に来る(idf の効き)。
    #[test]
    fn a_chunk_with_the_rare_term_ranks_first() {
        let (dir, mut store) = temp_store("ranking");
        ingest_markdown(&mut store, "notes", "a", "共通の語だけの本文。\n");
        ingest_markdown(&mut store, "notes", "b", "共通の語だけの別の本文。\n");
        ingest_markdown(&mut store, "notes", "c", "共通の語に加えて特有の合図がある本文。\n");
        let index = SearchIndex::build(&store).expect("build");
        let hits = index.search("共通 特有", None, 10);
        assert_eq!(hits.len(), 3, "共通の語で全チャンクが候補になる");
        assert_eq!(hits[0].chunk.document, "c", "希少語を含むチャンクが先頭に来るべき");
        assert!(hits[0].score > hits[1].score);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

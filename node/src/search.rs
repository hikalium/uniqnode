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

/// 文字列を索引語の列に切る(索引の構築と問い合わせが共用する唯一の実装。should/0135。
/// チャンクのどの文字列をここに通すかは chunk_terms_of が決める)。
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

/// 目次の行とみなす点線の長さ(連続する `.` の数)。見出しとページ番号を点でつなぐ
/// 組版がこの形になる。
const DOT_LEADER_RUN: usize = 4;
/// 本文に占める `.` の割合がこれを超えたら目次の紙面とみなす。実データ(仕様書 PDF
/// 25 本)の目次ページは 1 行のほとんどが点で埋まる。
const DOT_LEADER_RATIO: f64 = 0.15;
/// 点線の行がこの本数以上あれば、割合が低くても目次の紙面とみなす(点線の行と本文が
/// 同じチャンクに混じる紙面のため)。
const DOT_LEADER_LINES: usize = 3;
/// 「薄い」チャンクの長さの上限(文字数)。これ以上長ければ、語が少なくても本文と
/// みなす。
const THIN_CHAR_LIMIT: usize = 300;
/// 「薄い」チャンクの異なり索引語の数の上限。ページ番号だけ・柱(ページヘッダ)だけの
/// チャンクはここに落ちる。
const THIN_TERM_LIMIT: usize = 8;

/// このチャンクが低情報(目次の紙面・柱だけ・ページ番号だけ)かどうか(純関数。
/// 判定の家はここだけ。should/0135)。判定は本文だけを見る。
///
/// 実データ(仕様書 PDF 25 本・索引対象 25,098 チャンク)で観察された二つの型を、
/// そのまま条件にしてある。この判定に当たるのは 1,960 件(7.8%)で、標本 20 件の目視に
/// 本文のチャンクは無く、検証済みの正解ページ 54 チャンクのうち巻き込みは 1 件だった
/// (実測 2026-08-17。20260817-real-corpus-search-quality
/// (uuid:faeda9ac-5e9e-4091-8122-2fba9f80c8db))。
/// - 目次の紙面: `.` の割合が DOT_LEADER_RATIO を超えるか、点線の行が
///   DOT_LEADER_LINES 本以上ある。
/// - 薄いチャンク: 長さが THIN_CHAR_LIMIT 未満で、異なり索引語が THIN_TERM_LIMIT 未満。
///
/// 語の数え方は索引と同じ terms_of を通す(語の採り方を二重に実装しない。should/0135)。
/// 日本語は文字 bigram になるので、同じ長さでも仮名漢字の本文は語数が多く、ここには
/// 落ちない。
///
/// 判定するだけで索引からは外さない。索引に無いチャンクは GET /v1/objects/{id} でも
/// 引用を組めなくなり、pdftotext が柱しか採れなかった図版のページが見えから消える。
/// 落とすのは応答を組む側(node/src/api.rs の run_search)の後処理である。
pub fn is_low_information(text: &str) -> bool {
    let characters = text.chars().count();
    if characters == 0 {
        return true;
    }
    let dots = text.chars().filter(|character| *character == '.').count();
    if dots as f64 / characters as f64 > DOT_LEADER_RATIO {
        return true;
    }
    let leader_lines = text
        .lines()
        .filter(|line| {
            let mut run = 0usize;
            for character in line.chars() {
                run = if character == '.' { run + 1 } else { 0 };
                if run >= DOT_LEADER_RUN {
                    return true;
                }
            }
            false
        })
        .count();
    if leader_lines >= DOT_LEADER_LINES {
        return true;
    }
    if characters < THIN_CHAR_LIMIT {
        let mut terms = terms_of(text);
        terms.sort();
        terms.dedup();
        if terms.len() < THIN_TERM_LIMIT {
            return true;
        }
    }
    false
}

/// 見出しの語を本文の何回ぶんとして数えるか(重み)。見出しは本文より短く、同じ語を
/// 繰り返さないので、重み 1(素直な連結)のままだと節の題を並べただけの目次の節に
/// 定義の節が負ける。評価ハーネス(node/src/eval.rs)の固定コーパスで測ると、重み 1 は
/// 目次の対を取りこぼし、重み 2 と重み 3 は同じ数値になった。同じ数値なら小さい方を
/// 採る(重みを上げるほど見出しが文書長を占め、本文の語が薄まる)。
const BREADCRUMB_WEIGHT: usize = 2;

/// チャンク 1 件が索引に出す語の列(本文の語に、見出しの語を重みの回数だけ続けたもの)。
/// 索引の構築と評価ハーネスの対照方式が共用する、「このチャンクの索引語は何か」の唯一の
/// 家(should/0135)。切り方そのものは terms_of の一箇所のままで、ここは切る対象と、
/// 各語を何回数えるかを決める。
///
/// 見出し(meta.breadcrumbs)を含めるのは、見出しにしかない語で本文へ届くためである。
/// チャンカーは見出し行を本文に残さず meta.breadcrumbs へ写す(INGEST
/// (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の「チャンク分割」)ので、含めなければ
/// 節の題だけにある語はどのチャンクからも消え、検索で到達できない。
///
/// 重みは出現数を増やす形で与える。BM25 の tf と文書長の両方が同じ 1 本の語の列から
/// 出るので、フィールドごとに別の長さを持たせる仕掛けが要らない(その形も測ったが、
/// 数値は同じだった)。
///
/// 見出しは 1 段ずつ切る(段をつないでから切らない)。つなぐと段の境をまたぐ bigram が
/// 生まれ、どの見出しにも無い語が索引に入ってしまう。
pub fn chunk_terms_of(text: &str, breadcrumbs: &[String]) -> Vec<String> {
    let mut terms = terms_of(text);
    for crumb in breadcrumbs {
        let crumb_terms = terms_of(crumb);
        for _ in 0..BREADCRUMB_WEIGHT {
            terms.extend(crumb_terms.iter().cloned());
        }
    }
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
/// 添字、見出しは meta.breadcrumbs、PDF はさらに meta.page。取得日時は、そのチャンクを
/// 見えに置いている ref レコードの at である。
pub struct IndexedChunk {
    /// チャンクのオブジェクト ID(全文の取得は既存の GET /v1/objects/{id})。
    pub id: String,
    pub collection: String,
    pub document: String,
    pub position: usize,
    pub snippet: String,
    pub breadcrumbs: Vec<String>,
    pub page: Option<u32>,
    /// 取得日時(このチャンクを見えに置いている ref レコードの at。unix 秒)。
    pub at: i64,
    /// 低情報(目次の紙面・柱だけ・ページ番号だけ)かどうか(判定は is_low_information)。
    /// 索引には入れたままで、応答から落とすかどうかは要求の側が決める。
    pub low_information: bool,
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
///
/// 導出データの索引はどれもこの世代で最新かを判定する(BM25 の転置索引と、ベクトルの
/// 索引 node/src/embed.rs)。判定の家はここだけである(should/0135)。
#[derive(Clone, PartialEq, Eq)]
pub struct Generation {
    object_count: usize,
    signers: Vec<(String, u64)>,
}

impl Generation {
    /// store の現在の世代。
    pub fn current(store: &Store) -> Generation {
        Generation { object_count: store.object_count(), signers: store.signers() }
    }

    /// この世代が store の現状と一致するか(= 構築後に書き込みも ref の変化も無い)。
    pub fn matches(&self, store: &Store) -> bool {
        *self == Generation::current(store)
    }
}

/// 検索結果 1 件(得点と索引済みチャンクへの参照)。
pub struct SearchHit<'a> {
    pub score: f64,
    pub chunk: &'a IndexedChunk,
}

/// 順位付けの途中の 1 件(索引内の位置と得点)。方式をまたいで順位を融合する側は、
/// 引用を組む前のこの形で受け取る。
#[derive(Clone, Copy)]
pub struct ScoredChunk {
    /// 索引内の位置(visit_indexable_chunks の走査順)。
    pub position: usize,
    pub score: f64,
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

/// 索引の対象になるチャンク 1 件(走査が呼び手へ渡す形)。
pub struct IndexableChunk {
    /// チャンクのオブジェクト ID。ベクトルの導出層はこれを鍵の半分に使う。
    pub id: String,
    pub collection: String,
    pub document: String,
    pub position: usize,
    /// チャンクの本文全体(スニペットではない)。埋め込みに掛けるのはこちらである。
    pub text: String,
    pub breadcrumbs: Vec<String>,
    pub page: Option<u32>,
    /// 取得日時(このチャンクを見えに置いている ref レコードの at。unix 秒)。ref は
    /// 署名済みの可変層のレコードであり、at は署名者がその版を書いた時刻である
    /// (SPEC §4.4)。文書を取り込んだ時刻であって、原文が書かれた時刻ではない。
    pub at: i64,
    /// このチャンクの索引語(chunk_terms_of の結果。呼び手が数え直さずに済むように渡す)。
    pub terms: Vec<String>,
}

/// 「何が索引の対象か」の唯一の家(should/0135)。見えのチャンクのうち索引語を 1 語以上
/// 持つものを、決定的な順序(ref 名の昇順 → chunks 列の順)で渡す。
///
/// ref は完全名 <署名者>/<パス> で並ぶが、対象はパスが collections/ 配下のものだけ
/// (annotations/ などの ref は文書の見えではない。ASSERTIONS
/// (uuid:c05379e2-2d30-41bc-8342-62103d94bb21) の原理 5)。tombstone と、doc_rev や
/// chunk の形が崩れているものは飛ばす(索引は導出データであり、壊れた 1 個で構築全体を
/// 失敗させない)。語の無いチャンクはどのクエリにも一致しないので渡さない。
///
/// BM25 の転置索引(SearchIndex)とベクトルの索引(node/src/embed.rs の VectorIndex)が
/// これを共用する。同じ store の同じ世代から作る限り、両者は同じチャンクを同じ並びで
/// 見る。ハイブリッド検索が二つの索引の位置を突き合わせられるのはこのためである。
pub fn visit_indexable_chunks(
    store: &Store,
    visit: &mut dyn FnMut(IndexableChunk),
) -> Result<()> {
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
            // 索引語は本文と見出しの両方から採る(chunk_terms_of)。
            let terms = chunk_terms_of(text, &breadcrumbs);
            if terms.is_empty() {
                // 語の無いチャンクはどのクエリにも一致しない。
                continue;
            }
            visit(IndexableChunk {
                id: chunk_id.clone(),
                collection: collection.to_string(),
                document: document.to_string(),
                position,
                text: text.clone(),
                breadcrumbs,
                page,
                at: state.at,
                terms,
            });
        }
    }
    Ok(())
}

impl SearchIndex {
    /// 見えの全チャンクを一度走査して構築する(対象の決め方は visit_indexable_chunks)。
    pub fn build(store: &Store) -> Result<SearchIndex> {
        let generation = Generation::current(store);
        let mut chunks: Vec<IndexedChunk> = Vec::new();
        let mut postings: BTreeMap<String, Vec<Posting>> = BTreeMap::new();
        let mut total_terms = 0usize;
        visit_indexable_chunks(store, &mut |chunk| {
            let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
            for term in &chunk.terms {
                *counts.entry(term.as_str()).or_insert(0) += 1;
            }
            let index = chunks.len();
            for (term, term_frequency) in counts {
                // 走査はチャンク番号の昇順なので、各語の位置表も昇順で積み上がる。
                postings
                    .entry(term.to_string())
                    .or_default()
                    .push(Posting { chunk: index, term_frequency });
            }
            total_terms += chunk.terms.len();
            chunks.push(IndexedChunk {
                id: chunk.id,
                collection: chunk.collection,
                document: chunk.document,
                position: chunk.position,
                snippet: snippet_of(&chunk.text),
                breadcrumbs: chunk.breadcrumbs,
                page: chunk.page,
                at: chunk.at,
                // 判定は構築時に一度だけ行う(応答のたびに全文を持ち歩かないため。
                // IndexedChunk が持つのは先頭 200 文字のスニペットだけである)。
                low_information: is_low_information(&chunk.text),
                term_count: chunk.terms.len(),
            });
        })?;
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
        self.generation.matches(store)
    }

    /// 構築時点の世代(ベクトルの索引と同じ世代から作られたことを確かめる側が使う)。
    pub fn generation(&self) -> &Generation {
        &self.generation
    }

    /// 索引済みチャンクの件数。
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// 索引内の位置からチャンクを引く(位置は visit_indexable_chunks の走査順)。
    pub fn chunk(&self, position: usize) -> &IndexedChunk {
        &self.chunks[position]
    }

    /// オブジェクト ID から索引済みチャンクを引く(見えに無ければ None)。全文の取得
    /// (MCP の fetch)が引用を組むために使う。走査は線形だが、引くのは 1 要求につき
    /// 1 回であり、順位付けの内側ではない。
    pub fn chunk_by_id(&self, id: &str) -> Option<&IndexedChunk> {
        self.chunks.iter().find(|chunk| chunk.id == id)
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

    /// BM25 で top_k 件を返す(search_positions の被せ物。順位付けの家はそちら)。
    pub fn search(
        &self,
        query: &str,
        collection: Option<&str>,
        top_k: usize,
    ) -> Vec<SearchHit<'_>> {
        self.search_positions(query, collection, top_k)
            .into_iter()
            .map(|scored| SearchHit { score: scored.score, chunk: &self.chunks[scored.position] })
            .collect()
    }

    /// BM25 で top_k 件を、索引内の位置と得点で返す。collection を指定するとその
    /// コレクションだけに絞る(df は索引全体で数える。順位はどちらでも同一応答内でのみ
    /// 意味を持つ)。同点は索引順(ref 名の昇順 → chunks 列の順)で安定に決める
    /// (should/0125 の決定性)。
    ///
    /// 位置で返す形を持つのは、融合(node/src/embed.rs の RRF)がベクトル側の順位と
    /// 突き合わせるためである。引用を組むのは応答を作る側の仕事で、順位付けはここで
    /// 完結する。
    pub fn search_positions(
        &self,
        query: &str,
        collection: Option<&str>,
        top_k: usize,
    ) -> Vec<ScoredChunk> {
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
            .map(|(position, score)| ScoredChunk { position, score })
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

    /// チャンクの索引語は本文の語に見出しの語が重みの回数だけ続いたもの。期待値は
    /// リテラルで書く(検査対象から導出しない。should/0137)。
    #[test]
    fn chunk_terms_repeat_the_heading_terms_by_their_weight() {
        // 見出しの語は BREADCRUMB_WEIGHT(2)回ぶん数える。本文の語は 1 回のまま。
        assert_eq!(chunk_terms_of("本文", &["節".to_string()]), vec!["本文", "節", "節"]);
        // 見出しは 1 段ずつ切る: 段をつなぐと生まれる「代整」は索引語にならない。
        assert_eq!(
            chunk_terms_of("", &["世代".to_string(), "整合".to_string()]),
            vec!["世代", "世代", "整合", "整合"]
        );
        // 見出しを持たないチャンク(PDF はこの形)は本文の語だけ。
        assert_eq!(
            chunk_terms_of("token_estimate は 近似", &[]),
            vec!["token_estimate", "は", "近似"]
        );
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

    /// 低情報チャンクの判定。期待値はリテラルで書く(検査対象から導出しない。
    /// should/0137)。落とすのは目次の紙面と、柱やページ番号だけの薄いチャンクで、
    /// 本文はどの言語でも残る。
    #[test]
    fn low_information_catches_contents_pages_and_thin_chunks() {
        // 目次の紙面: 見出しとページ番号を点でつないだ行が並ぶ(点の割合で落ちる)。
        let contents = "IA-PC HPET ................................................ 4\n\
                        1.1 Revision History ...................................... 5\n\
                        1.2 Scope ................................................. 6\n";
        assert!(is_low_information(contents));
        // 本文に点線の行が混じる紙面。点の割合は低いが、点線の行が 3 本ある。
        let mixed = format!(
            "{}\n第 1 章 導入 .... 1\n第 2 章 索引 .... 2\n第 3 章 検索 .... 3\n",
            "この章では索引の構築と検索の順位付けについて述べる。".repeat(12)
        );
        assert!(mixed.chars().filter(|c| *c == '.').count() * 100 < mixed.chars().count() * 15);
        assert!(is_low_information(&mixed));
        // 柱(ページヘッダ)だけ・ページ番号だけのチャンク。
        assert!(is_low_information("RTL8139D(L)"));
        assert!(is_low_information("142"));
        assert!(is_low_information(""));
        // 本文は残る。英語の一節は語が足りている。
        let english = "The GetMemoryMap() function returns a copy of the current memory map. \
                       The map is an array of memory descriptors, each of which describes a \
                       contiguous block of memory.";
        assert_eq!(terms_of(english).len(), 28);
        assert!(!is_low_information(english));
        // 日本語の短い一節も残る(文字 bigram なので語数が足りる)。
        let japanese = "転置索引は導出データであり、世代の整合はオブジェクト数で確かめる。";
        assert!(japanese.chars().count() < THIN_CHAR_LIMIT);
        assert!(!is_low_information(japanese));
        // 節番号の点は目次の点線ではない(割合が閾値に届かない)。落ちるとすれば、
        // 見出しだけで本文の無い薄いチャンクだからである。
        assert!(!is_low_information(&format!(
            "5.2.3.2 Generic Address Structure\n{english}"
        )));
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
        // 取得日時は ref レコードの at(取り込みの時刻)であって、既定値の 0 ではない。
        // 下限はリテラル(2023-11-14T22:13:20Z)で書く(検査対象から導出しない。
        // should/0137)。
        assert!(
            hits[0].chunk.at > 1_700_000_000,
            "取得日時が ref レコードの at を運んでいない: {}",
            hits[0].chunk.at
        );

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

    /// 見出しにしかない語でも、その見出しの下のチャンクに届く。チャンカーは見出し行を
    /// 本文に残さず meta.breadcrumbs へ写すので、索引が本文だけを見ていたころ、この
    /// クエリはどのチャンクにも一致しなかった(空振り)。
    #[test]
    fn a_term_that_appears_only_in_the_heading_reaches_its_chunk() {
        let (dir, mut store) = temp_store("heading-only");
        // 本文には「監査証跡」の bigram(監査・査証・証跡)が一つも無い。
        ingest_markdown(
            &mut store,
            "notes",
            "manual",
            "# 監査証跡\n\nだれがいつ何を書いたのかの記録を残す。\n",
        );
        ingest_markdown(&mut store, "notes", "other", "# 別\n\n関係の無い本文。\n");
        let index = SearchIndex::build(&store).expect("build");
        let hits = index.search("監査証跡", None, 10);
        assert_eq!(hits.len(), 1, "見出しにしかない語で本文のチャンクに届くべき");
        assert_eq!(hits[0].chunk.document, "manual");
        assert_eq!(hits[0].chunk.breadcrumbs, vec!["監査証跡".to_string()]);
        // 索引語に足すだけで、応答のスニペットは本文のまま(見出しは citation が持つ)。
        assert!(
            !hits[0].chunk.snippet.contains("監査証跡"),
            "スニペットは本文のはず: {}",
            hits[0].chunk.snippet
        );
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

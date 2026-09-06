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

/// 索引語を「まとまり」に分けて返す(被覆率を数える単位。切り方そのものは terms_of の
/// 一箇所のまま)。
///
/// ASCII の語は 1 語で 1 まとまりである。非 ASCII の連なりは、その連なりから出た文字
/// bigram 全部で 1 まとまりになる。この区別が被覆率の意味を決める:
/// - 「scratchpad」のような 1 語は、当たれば意図が当たったと言ってよい。
/// - 「スクラッチパッド」の bigram は 1 語の断片であり、2 つ当たっただけでは意図が当たった
///   ことにならない(「取り込み」が「割り込み」に当たる型。20260817-japanese-partial-match
///   (uuid:ff9299d5-3dfb-4ad0-9b3b-f5d597271cdc))。
///
/// まとまりごとの当たり具合(当たった語 / まとまりの語数)の平均を被覆率とすると、
/// 両方が同じ物差しで測れる。カタカナの問いを英語へ広げたとき(node/src/translit.rs)、
/// 広げた語が当たれば被覆はまとまり単位で 0.5 に届き、断片だけの一致は届かない。
pub fn term_groups_of(text: &str) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    for run in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if run.is_empty() {
            continue;
        }
        // 1 つの連なりから出た語をまとめる。ASCII と非 ASCII が地続きの連なり
        // (identifier1 と 日本語 が空白なしで続く形)は terms_of が境で切るので、
        // ここでも同じ切り方に従い、出た語のうち ASCII の 1 語だけは独立させる。
        let terms = terms_of(run);
        let (ascii, wide): (Vec<String>, Vec<String>) = terms
            .into_iter()
            .partition(|term| term.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        for word in ascii {
            groups.push(vec![word]);
        }
        if !wide.is_empty() {
            groups.push(wide);
        }
    }
    groups
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
    /// 原本(このチャンクの出た文書の doc_rev.source = 原文 blob のオブジェクト ID)。
    /// PDF なら PDF そのもの、markdown なら原文テキストである。ページ画像の遅延生成は
    /// これと page から「どの PDF の何ページか」を組み立てる。doc_rev に source が
    /// 無い(壊れた・他実装が書いた)ときだけ None。
    pub source: Option<String>,
    /// 種別(doc_rev の meta.media。"pdf" / "markdown" / "text")。source をどう扱うか
    /// (ページ画像に描けるのは "pdf" だけ)は呼び手がこれで判断する。
    pub media: Option<String>,
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

/// 構築時点の世代(導出データの鮮度札)。索引の中身は次の二つだけの純関数である:
/// (a) collections/ 配下の非 tombstone ref の束縛(名前 → target。visit_indexable_chunks が
/// 読む範囲そのもの)、(b) そこから辿れるオブジェクトの内容(content-addressed で不変)。
/// したがって索引が古くなる原因は二つしかない: 束縛が変わったか、構築時に「まだ無い」で
/// 飛ばしたオブジェクト(未複製の doc_rev やチャンク)が後から届いたか。札はこの二つだけを
/// 見る。
///
/// もとは (ストア全体のオブジェクト数, 署名者ごとの最終 seq) を見ていた。それはストアへの
/// 書き込みすべてに反応する札で、索引が読まないもの — collections/ の外の ref、索引の対象で
/// ないオブジェクト — を足しただけでも「古い」になり、次の検索が全再構築を引いた。実データ
/// (25,754 オブジェクト)での再構築は BM25 索引の構築 8.80 秒、ベクトル索引の読み込み
/// 8.31 秒である。たとえば PDF のページ画像をストアに 1 枚足すたびに次の検索が 17 秒に
/// なり、その機能自体が成り立たない。札が見る範囲を索引が読む範囲に一致させれば、索引と
/// 無関係な書き込みは索引を落とさない。
///
/// 導出データの索引はどれもこの世代で最新かを判定する(BM25 の転置索引と、ベクトルの
/// 索引 node/src/embed.rs)。判定の家はここだけである(should/0135)。
///
/// 等値は導出しない(以前は matches が `*self == current(store)` だった)。判定は札と
/// 現状の非対称な比較 — missing が 0 なら object_count を見ない — であり、札同士の
/// 等値とは別物である。両方があると、うっかり `==` で書いた側が黙って別の規則になる。
#[derive(Clone)]
pub struct Generation {
    /// collections/ 配下の非 tombstone ref の束縛(ref の完全名 → target)。
    bindings: Vec<(String, String)>,
    /// 構築時に「ストアに無い」で飛ばしたオブジェクトの数。0 なら、後から何が届いても
    /// 索引の中身は変わらない。
    missing: usize,
    /// missing が 0 でないときだけ意味を持つ(取りこぼしが埋まったかを見るため)。
    object_count: usize,
}

impl Generation {
    /// 走査を始める前の store の世代。取りこぼしの数は走査してみるまで判らないので 0 で
    /// 置き、走査が終わったら note_missing で入れる。束縛とオブジェクト数を走査より先に
    /// 採るのは、走査中に届いたものを「構築時に見えていた」と誤って記録しないためである
    /// (先に採れば、取りこぼしたぶんは次の判定で古いに倒れる。安全側)。
    pub fn current(store: &Store) -> Generation {
        Generation {
            bindings: Generation::bindings_of(store),
            missing: 0,
            object_count: store.object_count(),
        }
    }

    /// 構築の走査が「ストアに無い」で飛ばしたオブジェクトの数を記録する
    /// (visit_indexable_chunks の返り値をそのまま渡す)。
    pub fn note_missing(&mut self, missing: usize) {
        self.missing = missing;
    }

    /// collections/ 配下の非 tombstone ref の束縛。RAM 上の ref 表(Store::list_refs)を
    /// 一巡するだけで、ストアの読み込みは無い(O(#refs))。走査(visit_indexable_chunks)と
    /// 同じ条件で絞るが、あちらは 1 件ずつ doc_rev を開く走査であり、こちらは要求ごとに
    /// 回る鮮度札である。「見えの文書を指す ref か」の読み方は document_ref_parts の
    /// 一箇所を共用する(should/0135)。
    fn bindings_of(store: &Store) -> Vec<(String, String)> {
        store
            .list_refs()
            .filter_map(|(name, state)| {
                // tombstone は見えに無い = 索引が読まない。
                let target = state.target.as_ref()?;
                document_ref_parts(name)?;
                Some((name.clone(), target.clone()))
            })
            .collect()
    }

    /// この世代が store の現状について最新か。
    /// - 束縛が違えば古い(文書の改版・巻き戻し・tombstone・新しい文書)。
    /// - 束縛が同じでも、構築時に取りこぼしがあったならオブジェクト数も見る(取りこぼしが
    ///   埋まったかもしれない)。取りこぼしの無かった索引は、束縛が同じである限り最新で
    ///   ある: 読む範囲のオブジェクトは全部読めていて、その中身は不変だからである。
    pub fn matches(&self, store: &Store) -> bool {
        if self.missing != 0 && self.object_count != store.object_count() {
            return false;
        }
        self.bindings == Generation::bindings_of(store)
    }
}

/// 検索結果 1 件(得点と索引済みチャンクへの参照)。
pub struct SearchHit<'a> {
    pub score: f64,
    pub chunk: &'a IndexedChunk,
}

/// 検索が見てよいコレクションの範囲。要求側の絞り込み(collection を 1 つ指定する)と、
/// 応答側の共有ポリシー(このピアへ出してよいコレクションの一覧。SPEC §6.3 の share)は
/// どちらもこの型で表し、重ねるときは intersect で交差を取る。範囲の判定の家はここだけ
/// である(should/0135)。
///
/// Only(空) は「見てよいコレクションが 1 つも無い」であり、All とは違う: どのチャンクも
/// 通さない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionScope {
    All,
    Only(Vec<String>),
}

impl CollectionScope {
    /// 要求の collection(省略時は全体)から作る。
    pub fn of(collection: Option<&str>) -> CollectionScope {
        match collection {
            None => CollectionScope::All,
            Some(name) => CollectionScope::Only(vec![name.to_string()]),
        }
    }

    pub fn allows(&self, collection: &str) -> bool {
        match self {
            CollectionScope::All => true,
            CollectionScope::Only(names) => names.iter().any(|name| name == collection),
        }
    }

    /// 交差。要求の絞り込みと共有ポリシーを重ねると、両方が許すコレクションだけが残る。
    pub fn intersect(&self, other: &CollectionScope) -> CollectionScope {
        match (self, other) {
            (CollectionScope::All, scope) | (scope, CollectionScope::All) => scope.clone(),
            (CollectionScope::Only(mine), CollectionScope::Only(theirs)) => CollectionScope::Only(
                mine.iter().filter(|name| theirs.contains(name)).cloned().collect(),
            ),
        }
    }

    /// どのチャンクも通さない範囲か(Only(空))。呼び手は、空振りの理由を「一致が無い」と
    /// 混同せずに言える。
    pub fn is_empty(&self) -> bool {
        matches!(self, CollectionScope::Only(names) if names.is_empty())
    }
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

/// ストア上のオブジェクトを c1 として読む。読めなければ None(索引は導出データであり、
/// 壊れた 1 個で構築全体を失敗させない。ReferrerIndex と同じ扱い)。入出力の失敗だけは
/// 伝える。
///
/// 読めない理由は世代の判定にとって二種類に分かれるので、片方だけを missing に数える:
/// - ストアに無い(他ノードの ref の複製前): あとから届けば索引の中身が変わる。数える。
/// - UTF-8 でない・c1 でない: 中身は content-addressed で不変であり、同じ ID が後から
///   別の内容になることはない。何が届いても索引は変わらないので数えない。
fn read_c1(store: &Store, id: &str, missing: &mut usize) -> Result<Option<Value>> {
    let Some(bytes) = store.get_object(id)? else {
        *missing += 1;
        return Ok(None);
    };
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
    /// 原本(doc_rev.source = 原文 blob のオブジェクト ID)。IndexedChunk と同じもの。
    pub source: Option<String>,
    /// 種別(doc_rev の meta.media。"pdf" / "markdown" / "text")。
    pub media: Option<String>,
    /// 取得日時(このチャンクを見えに置いている ref レコードの at。unix 秒)。ref は
    /// 署名済みの可変層のレコードであり、at は署名者がその版を書いた時刻である
    /// (SPEC §4.4)。文書を取り込んだ時刻であって、原文が書かれた時刻ではない。
    pub at: i64,
    /// このチャンクの索引語(chunk_terms_of の結果。呼び手が数え直さずに済むように渡す)。
    pub terms: Vec<String>,
}

/// ref の完全名 <署名者>/<パス> が文書の見えを置く形 collections/<コレクション名>/<文書名>
/// なら、その (コレクション名, 文書名) を返す。署名者は見ない(他のDBノードの ref も
/// 同じ形なら同じ文書である)。tombstone かどうかも見ない(target は呼び手が見る)。
/// ref 名の読み方の唯一の家(should/0135): 索引の走査(visit_indexable_chunks)・
/// 鮮度札(Generation)・コレクションの一覧(node/src/api.rs の GET /v1/collections)が
/// 共用する。
pub fn document_ref_parts(name: &str) -> Option<(&str, &str)> {
    let (_signer, path) = name.split_once('/')?;
    let rest = path.strip_prefix("collections/")?;
    rest.split_once('/')
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
///
/// 返り値は「ストアに無い」で飛ばしたオブジェクトの数(doc_rev と chunk の合計)。
/// 走査から作った導出データの鮮度札(Generation)がこれを要る: 0 なら、その札は
/// ストアに何が届いても揺るがない(理由は Generation のコメント)。
pub fn visit_indexable_chunks(
    store: &Store,
    visit: &mut dyn FnMut(IndexableChunk),
) -> Result<usize> {
    let mut missing = 0usize;
    for (name, state) in store.list_refs() {
        // tombstone は現在の見えに無い(原理 5)。
        let Some(target) = &state.target else { continue };
        let Some((collection, document)) = document_ref_parts(name) else { continue };
        let Some(Value::Object(doc_rev)) = read_c1(store, target, &mut missing)? else { continue };
        // 原本(source = 原文 blob)と種別(meta.media)は、いま手にしている doc_rev から
        // そのまま採る(走査の回数もストアの読み込み回数も増えない)。チャンクからは
        // 「どの原本のどこか」へ辿れないと、PDF のページ画像を後から作れない。既存の
        // 取り込み済みデータを読み直す必要が無いよう、ストアには何も書き足さない。
        let source = match doc_rev.get("source") {
            Some(Value::Text(blob)) => Some(blob.clone()),
            _ => None,
        };
        let media = match doc_rev.get("meta") {
            Some(Value::Object(meta)) => match meta.get("media") {
                Some(Value::Text(media)) => Some(media.clone()),
                _ => None,
            },
            _ => None,
        };
        let Some(Value::Array(chunk_ids)) = doc_rev.get("chunks") else { continue };
        for (position, chunk_ref) in chunk_ids.iter().enumerate() {
            let Value::Text(chunk_id) = chunk_ref else { continue };
            let Some(Value::Object(chunk)) = read_c1(store, chunk_id, &mut missing)? else {
                continue;
            };
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
                source: source.clone(),
                media: media.clone(),
                at: state.at,
                terms,
            });
        }
    }
    Ok(missing)
}

impl SearchIndex {
    /// 見えの全チャンクを一度走査して構築する(対象の決め方は visit_indexable_chunks)。
    pub fn build(store: &Store) -> Result<SearchIndex> {
        // 束縛は走査の前に採り、取りこぼしの数だけ走査のあとで足す(Generation)。
        let mut generation = Generation::current(store);
        let mut chunks: Vec<IndexedChunk> = Vec::new();
        let mut postings: BTreeMap<String, Vec<Posting>> = BTreeMap::new();
        let mut total_terms = 0usize;
        let missing = visit_indexable_chunks(store, &mut |chunk| {
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
                source: chunk.source,
                media: chunk.media,
                at: chunk.at,
                // 判定は構築時に一度だけ行う(応答のたびに全文を持ち歩かないため。
                // IndexedChunk が持つのは先頭 200 文字のスニペットだけである)。
                low_information: is_low_information(&chunk.text),
                term_count: chunk.terms.len(),
            });
        })?;
        generation.note_missing(missing);
        let average_length = if chunks.is_empty() {
            0.0
        } else {
            total_terms as f64 / chunks.len() as f64
        };
        Ok(SearchIndex { generation, chunks, postings, average_length })
    }

    /// この索引が store の現状について最新か(判定の規則は Generation)。collections/ の
    /// 束縛が変わっていなければ最新である。索引が読まないもの(collections/ の外の ref、
    /// 索引の対象でないオブジェクト)がいくら増えても作り直しにはならない。
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

    /// 索引にある語を 1 度ずつ渡す(語と、その語を含むチャンク数)。
    ///
    /// カタカナのクエリを英語の術語へ寄せる層(node/src/translit.rs)が、実在する語だけを
    /// 候補にするために使う。実在しない綴りを作らないことがあの層の要であり、その判定は
    /// 「この索引に載っているか」でしかできない。チャンク数を添えるのは、骨格が同じ語が
    /// 複数あるときの決め手になるからである(実測で、これを綴りの近さより先に見ると
    /// 当たりが 32/70 から 53/70 に増えた)。
    pub fn visit_terms(&self, visit: &mut dyn FnMut(&str, usize)) {
        for (term, postings) in &self.postings {
            visit(term, postings.len());
        }
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
        scope: &CollectionScope,
        top_k: usize,
    ) -> Vec<SearchHit<'_>> {
        self.search_positions(query, scope, top_k)
            .into_iter()
            .map(|scored| SearchHit { score: scored.score, chunk: &self.chunks[scored.position] })
            .collect()
    }

    /// BM25 で top_k 件を、索引内の位置と得点で返す。scope で挙げたコレクションだけに
    /// 絞る(df は索引全体で数える。順位はどちらでも同一応答内でのみ意味を持つ)。同点は
    /// 索引順(ref 名の昇順 → chunks 列の順)で安定に決める(should/0125 の決定性)。
    ///
    /// 位置で返す形を持つのは、融合(node/src/embed.rs の RRF)がベクトル側の順位と
    /// 突き合わせるためである。引用を組むのは応答を作る側の仕事で、順位付けはここで
    /// 完結する。
    pub fn search_positions(
        &self,
        query: &str,
        scope: &CollectionScope,
        top_k: usize,
    ) -> Vec<ScoredChunk> {
        self.ranked_lexical(query, scope, top_k).hits
    }

    /// BM25 の順位と、その順位がクエリの語をどれだけ覆っているか。被覆は呼び手(融合)が
    /// 「語の一致がどれだけ効いているか」を判断するために要る。
    pub fn ranked_lexical(
        &self,
        query: &str,
        scope: &CollectionScope,
        top_k: usize,
    ) -> LexicalRanking {
        // 語のまとまり(ASCII の 1 語 / 非 ASCII の連なりから出た bigram の束)。被覆は
        // まとまり単位で数える(term_groups_of の理由を参照)。
        let groups = term_groups_of(query);
        let mut group_of: BTreeMap<String, usize> = BTreeMap::new();
        for (index, group) in groups.iter().enumerate() {
            for term in group {
                group_of.entry(term.clone()).or_insert(index);
            }
        }
        let mut query_terms = terms_of(query);
        query_terms.sort();
        query_terms.dedup();
        // (得点の合計, 一致した異なりクエリ語の数)。
        let mut scores: BTreeMap<usize, (f64, usize)> = BTreeMap::new();
        // チャンクごとに、まとまりの何番目の語が当たったか(被覆をまとまり単位で数える)。
        let mut hits_per_group: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for term in &query_terms {
            let postings = self.postings_of(term);
            if postings.is_empty() {
                continue;
            }
            let document_frequency = postings.len();
            for posting in &postings {
                let chunk = &self.chunks[posting.chunk];
                if !scope.allows(&chunk.collection) {
                    continue;
                }
                let entry = scores.entry(posting.chunk).or_insert((0.0, 0));
                entry.0 += bm25_term_score(
                    posting.term_frequency,
                    document_frequency,
                    self.chunks.len(),
                    chunk.term_count,
                    self.average_length,
                );
                entry.1 += 1;
                if let Some(group) = group_of.get(term) {
                    let counts =
                        hits_per_group.entry(posting.chunk).or_insert_with(|| vec![0; groups.len()]);
                    counts[*group] += 1;
                }
            }
        }
        let query_term_count = query_terms.len();
        // まとまりごとの当たり具合の平均。ASCII の 1 語は当たれば 1.0、非 ASCII の連なりは
        // 「当たった bigram / その連なりの bigram 数」になる。
        let coverage_of = |chunk: usize| -> f64 {
            let Some(counts) = hits_per_group.get(&chunk) else { return 0.0 };
            if groups.is_empty() {
                return 0.0;
            }
            let sum: f64 = counts
                .iter()
                .zip(groups.iter())
                .map(|(matched, group)| *matched as f64 / group.len().max(1) as f64)
                .sum();
            sum / groups.len() as f64
        };
        // 被覆率を得点に掛ける(古典的な coord)。クエリ語の一部にしか当たっていない
        // チャンクを、当たった語が稀だというだけで上位に置かないためである。文字 bigram の
        // 索引では、この歪みが日本語の複合語で顕著に出る: 「割り込み」と「取り込み」は
        // bigram を 2 つ共有し、英語コーパスの中では日本語 bigram の df が 1 なので idf が
        // 跳ね上がって、無関係な文書が高得点で 1 位を取る(実測 2026-08-17。
        // 20260817-japanese-partial-match (uuid:ff9299d5-3dfb-4ad0-9b3b-f5d597271cdc))。
        let mut ranked: Vec<(usize, f64, f64)> = scores
            .into_iter()
            .map(|(position, (score, _matched))| {
                let coverage = coverage_of(position);
                (position, score * coverage, coverage)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(top_k);
        // 返す順位の中で最もよく覆っているものを、その順位全体の被覆とする(融合に効かせる
        // のは「語の一致がどれだけ当たったか」であり、下位の弱い件ではない)。
        let best_coverage =
            ranked.iter().map(|(_, _, coverage)| *coverage).fold(0.0f64, f64::max);
        LexicalRanking {
            hits: ranked
                .into_iter()
                .map(|(position, score, _)| ScoredChunk { position, score })
                .collect(),
            coverage: best_coverage,
            query_terms: query_term_count,
        }
    }
}

/// BM25 の順位 1 回ぶんと、その被覆(クエリの異なり語のうち何語に当たったか)。
pub struct LexicalRanking {
    pub hits: Vec<ScoredChunk>,
    /// 返した順位の中で最もよく覆っている件の被覆率(0.0..=1.0)。まとまり単位で数える
    /// (term_groups_of)。
    coverage: f64,
    /// クエリの異なり語の数(0 なら索引語の無いクエリ)。誤りの本文に出す。
    pub query_terms: usize,
}

impl LexicalRanking {
    /// 被覆率(0.0..=1.0)。当たりが無ければ 0。
    pub fn coverage(&self) -> f64 {
        self.coverage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{chunk_markdown, ingest_document, Chunk, DocumentInput, IngestOutcome};
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
                extra_meta: &[],
            },
        )
        .expect("ingest")
    }

    /// PDF の取り込みを真似る(原本の PDF そのものを source に、pdftotext の出力を
    /// チャンクにする形。ここでは PDF の中身は問われないので短いバイト列でよい)。
    fn ingest_pdf(
        store: &mut Store,
        collection: &str,
        name: &str,
        bytes: &[u8],
        chunks: &[Chunk],
    ) -> IngestOutcome {
        ingest_document(
            store,
            &DocumentInput {
                collection,
                name,
                source: bytes,
                media: "pdf",
                chunks,
                extractor: Some("pdftotext 22.02"),
                extra_meta: &[],
            },
        )
        .expect("ingest")
    }

    /// 索引項は原本(doc_rev.source = 原文 blob)と種別(meta.media)を運ぶ。ページ画像の
    /// 遅延生成は、チャンクからこの二つと page を辿って「どの PDF の何ページか」を組む。
    /// doc_rev には既に両方あるので、ストアには何も書き足していない(取り込み済みの
    /// データを読み直さずに済むことが要件である)。
    #[test]
    fn the_index_carries_the_source_blob_and_the_media_of_each_document() {
        let (dir, mut store) = temp_store("provenance");
        let pdf_bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n";
        let chunks = vec![Chunk {
            text: "割り込みの初期化手順を述べる。合言葉は雷鳥である。".to_string(),
            breadcrumbs: Vec::new(),
            page: Some(7),
        }];
        ingest_pdf(&mut store, "specs", "manual", pdf_bytes, &chunks);
        let markdown = "# 章\n\nこちらは markdown の本文。合言葉は雲雀である。\n";
        ingest_markdown(&mut store, "notes", "memo", markdown);
        let index = SearchIndex::build(&store).expect("build");

        // PDF のチャンク: media は "pdf"、source は原本 PDF の blob そのもの。
        let hits = index.search("雷鳥", &CollectionScope::All, 10);
        assert_eq!(hits.len(), 1);
        let pdf_chunk = hits[0].chunk;
        assert_eq!(pdf_chunk.media.as_deref(), Some("pdf"));
        assert_eq!(pdf_chunk.page, Some(7), "PDF のチャンクは紙面の番号を持つ");
        let pdf_source = pdf_chunk.source.clone().expect("原本の PDF を指しているはず");
        assert_eq!(
            store.get_object(&pdf_source).expect("get").as_deref(),
            Some(&pdf_bytes[..]),
            "source が指す blob は取り込んだ PDF そのもののはず"
        );

        // markdown のチャンク: media は "markdown"、source は原文テキストの blob。
        let hits = index.search("雲雀", &CollectionScope::All, 10);
        assert_eq!(hits.len(), 1);
        let markdown_chunk = hits[0].chunk;
        assert_eq!(markdown_chunk.media.as_deref(), Some("markdown"));
        assert_eq!(markdown_chunk.page, None, "markdown に紙面は無い");
        let markdown_source = markdown_chunk.source.clone().expect("原文を指しているはず");
        assert_eq!(
            store.get_object(&markdown_source).expect("get").as_deref(),
            Some(markdown.as_bytes()),
            "source が指す blob は取り込んだ原文テキストそのもののはず"
        );
        assert_ne!(markdown_source, pdf_source, "文書ごとに別の原本を指す");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 世代は collections/ の束縛だけを見る(規則は Generation)。張り替えと tombstone では
    /// ずれ、索引が読まないもの — collections/ の外の ref、索引の対象でないオブジェクト —
    /// が増えてもずれない。後者が肝である: PDF のページ画像をストアに足しても索引は生き
    /// 残る(足すたびに作り直すと、実測 8.80 秒 + 8.31 秒が次の検索に乗る)。
    #[test]
    fn the_generation_shifts_only_when_the_collections_bindings_change() {
        let (dir, mut store) = temp_store("generation");
        ingest_markdown(&mut store, "notes", "memo", "# 章\n\n初版だけの合言葉。\n");
        let index = SearchIndex::build(&store).expect("build");
        assert!(index.is_current(&store));

        // 索引が読まないオブジェクト(たとえば後から作るページ画像)を足しても、束縛は
        // 変わらないので索引は生き残る。
        let (image, is_new) = store.put_object(b"\"page image bytes\"").expect("put");
        assert!(is_new, "新しいオブジェクトとして入る");
        assert!(index.is_current(&store), "無関係なオブジェクトで索引を捨ててはならない");
        // collections/ の外へ ref を張っても同じ(reflog は伸び、署名者の seq も進む)。
        store.set_ref("pages/memo/1", Some(&image)).expect("set_ref");
        assert!(index.is_current(&store), "collections/ の外の ref で索引を捨ててはならない");
        assert_eq!(
            index.search("合言葉", &CollectionScope::All, 10).len(),
            1,
            "生き残った索引はそのまま引ける"
        );

        // 改版(collections/ の ref の張り替え)ではずれる。
        ingest_markdown(&mut store, "notes", "memo", "# 章\n\n改訂で言い換えた本文。\n");
        assert!(!index.is_current(&store), "束縛が変わったのに最新と誤認している");
        let rebuilt = SearchIndex::build(&store).expect("rebuild");
        assert!(rebuilt.is_current(&store));

        // tombstone でもずれる(オブジェクトは 1 個も増えない)。
        let objects = store.object_count();
        store.set_ref("collections/notes/memo", None).expect("tombstone");
        assert_eq!(store.object_count(), objects, "tombstone はオブジェクトを増やさない");
        assert!(!rebuilt.is_current(&store), "tombstone を世代が検出できていない");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 取りこぼし(ref が指す doc_rev がまだ複製されていない)を抱えて構築した索引は、
    /// その doc_rev が届いたことに気づく。取りこぼしが 0 でない間だけオブジェクト数を
    /// 見る、という規則の証明である(0 なら束縛だけを見る。上の試験)。
    #[test]
    fn an_index_built_over_a_missing_doc_rev_notices_it_arriving() {
        let (dir_a, mut a) = temp_store("missing-a");
        let (dir_b, mut b) = temp_store("missing-b");
        let outcome = ingest_markdown(&mut a, "notes", "memo", "# 章\n\n複製前の合言葉。\n");
        // B へは ref レコードだけを複製する(L1 の受け側と同じ経路)。
        let signer = a.node_id_hex().to_string();
        for record in a.export_ref_records(&signer, 0).expect("export") {
            assert!(b.ingest_ref_record(&record).expect("ingest record"));
        }
        assert!(!b.has_object(&outcome.doc_rev_id), "B は ref の先をまだ持たない");

        let index = SearchIndex::build(&b).expect("build");
        assert_eq!(index.chunk_count(), 0, "実体が無いので索引は空");
        assert!(index.is_current(&b));

        // doc_rev が届けば索引の中身は変わる。束縛は同じままだが、取りこぼしを抱えて
        // いたので、オブジェクト数の変化で古いと判る。
        let doc_rev = a.get_object(&outcome.doc_rev_id).expect("get").expect("A は持つ");
        b.put_object(&doc_rev).expect("put");
        assert!(!index.is_current(&b), "取りこぼしが埋まったのに最新と誤認している");
        std::fs::remove_dir_all(&dir_a).expect("cleanup");
        std::fs::remove_dir_all(&dir_b).expect("cleanup");
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
        let hits = index.search("合言葉", &CollectionScope::All, 10);
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
            rebuilt.search("合言葉", &CollectionScope::All, 10).is_empty(),
            "旧版のチャンクが索引に残っている"
        );
        assert_eq!(rebuilt.search("改訂", &CollectionScope::All, 10).len(), 1);

        // ref の張り替えだけの変化(tombstone)はオブジェクトを増やさないが、世代
        // (collections/ の束縛)がずれて作り直しになる。
        let objects = store.object_count();
        store.set_ref("collections/notes/memo", None).expect("tombstone");
        assert_eq!(store.object_count(), objects, "tombstone はオブジェクトを増やさない");
        assert!(!rebuilt.is_current(&store), "ref だけの変化を世代が検出できていない");
        let after = SearchIndex::build(&store).expect("rebuild");
        assert!(after.search("改訂", &CollectionScope::All, 10).is_empty(), "tombstone 後も見えに残っている");
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
        let hits = index.search("鍵", &CollectionScope::All, 10);
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
        let hits = index.search("監査証跡", &CollectionScope::All, 10);
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
        let hits = index.search("共通 特有", &CollectionScope::All, 10);
        assert_eq!(hits.len(), 3, "共通の語で全チャンクが候補になる");
        assert_eq!(hits[0].chunk.document, "c", "希少語を含むチャンクが先頭に来るべき");
        assert!(hits[0].score > hits[1].score);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

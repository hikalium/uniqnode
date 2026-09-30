//! 埋め込みとハイブリッド検索(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))。
//!
//! この層が持つのは四つである。
//! - 埋め込みサーバ(llama-server の OpenAI 互換 /v1/embeddings)への最小クライアント。
//! - (チャンクのオブジェクト ID, embedder_id)を鍵にしたベクトルの導出層と、その永続
//!   キャッシュ。
//! - 全走査コサインの意味検索(RAM に載る規模では ANN は要らない)。
//! - BM25(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))との RRF 融合。
//!
//! ベクトルは導出データ(I4)であり、真実の源泉には入れない: オブジェクトにも reflog にも
//! 書かず、<data_dir>/derived/embeddings/ の下のキャッシュファイルに置く。消しても失われる
//! のは計算時間だけで、同じチャンクと同じ模型から同じ鍵で作り直せる(ASSERTIONS
//! (uuid:c05379e2-2d30-41bc-8342-62103d94bb21) の原理 3 帰結 2 と同じ扱い)。鍵に
//! embedder_id が入るので、模型を替えたベクトルは別物として並び、混ざらない。
//!
//! ベクトルはビット単位の再現性を持たない。同じ入力でもバッチの構成が変わると 1e-4 級で
//! ずれる(GPU の畳み込み順序)ので、ベクトルをバイト一致で同一視する設計にはしない。
//! 同一視の鍵はあくまで(チャンク ID, embedder_id)である。
//!
//! 届かないときは黙って劣化しない: 検索は BM25 だけで答え、何が起きて方式が落ちたのかを
//! 応答(method と degraded)と標準エラーに残す(should/0128)。

use crate::c1;
use crate::http;
use crate::json::Json;
use crate::search::{visit_indexable_chunks, CollectionScope, Generation, ScoredChunk, SearchIndex};
use crate::store::{Store, StoreError};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 既定の埋め込みエンドポイント(手元の llama-server)。要求の組み立てと、テストや CLI が
/// 既定として使う文字列の家はここだけである(must/0023)。
pub const DEFAULT_EMBEDDING_URL: &str = "http://127.0.0.1:8083/v1/embeddings";
/// 既定の模型識別子。ベクトルの鍵の半分(embedder_id)であり、要求の "model" でもある。
pub const DEFAULT_EMBEDDER_ID: &str = "bge-m3";
/// bge-m3 のベクトルの次元。応答がこの次元でなければ通さない(must/0022)。
pub const DEFAULT_EMBEDDING_DIMENSION: usize = 1024;
/// 1 要求に載せるテキストの数。llama-server は入力の配列を一度に処理するので、往復の
/// 回数はここで決まる。大きくしすぎるとサーバ側のバッチに載らないので控えめに採る。
pub const DEFAULT_EMBED_BATCH: usize = 16;
/// まとめて埋め込むときの期限(1 要求ぶん)。
pub const DEFAULT_EMBED_TIMEOUT: Duration = Duration::from_secs(120);
/// serve がクエリ 1 本を埋め込むときの期限。要求の待ちを長引かせない。
pub const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(15);

/// RRF の定数 k。順位 r に 1/(k + r) を与える。
///
/// 原典(Cormack et al., SIGIR 2009)の k = 60 は数千件の完全な順位列を融合する設定で
/// 選ばれた値で、ここが融合するのは各方式の上位 RRF_DEPTH 件だけである。深さ 30 に
/// k = 60 を当てると寄与は 1/61〜1/90(最上位と最下位の差が 1.5 倍)にしかならず、
/// 順位の情報がほとんど残らない。k = 20 なら 1/21〜1/50(2.4 倍)である。
///
/// 深さと組にして測って選んだ(実測 2026-08-18。測り方と格子は RRF_DEPTH のコメント)。
/// 深さ 30 で k を振ったときの Recall@1/@3/@5/@10・MRR は次のとおりである。
/// - k = 10: 0.618 / 0.824 / 0.912 / 0.971 / 0.737
/// - k = 20: 0.618 / 0.824 / 0.941 / 0.971 / 0.738
/// - k = 60: 0.618 / 0.794 / 0.941 / 0.971 / 0.731
///
/// k = 20 はどの打ち切りでも最良か同値である。k = 60 との差は 2 問の入れ替わりで、
/// 「USB の standard device request の一覧」が 4 位から 2 位に上がり(語の一致と意味検索の
/// 両方が上位に置く件が、片方だけの件を追い越す)、「ELF program header table entries」が
/// 5 位から 7 位に下がる。10 と 20 の差は 1 問(「System V AMD64 ABI の引数レジスタ」が
/// 7 位と 5 位)しかないので、この標本では 10 と 20 を強くは分けられない。両端(60)を
/// 外せることだけがはっきり言えて、中を採った。
pub const RRF_K: f64 = 20.0;
/// 融合に持ち込む各方式の候補数の下限(実際には top_k とこの値の大きい方)。要求が
/// 10 件でも、各方式の上位 30 件どうしを融合する。
///
/// 深さと k は格子で測って選んだ(実測 2026-08-18)。測り方: 実ストアの複製(仕様書
/// PDF 25 本・25,884 チャンク)を /tmp に置いて 34 問を top_k=10 で引き、正解ページの
/// 順位を数えた。問いは前の測定(20260817-real-corpus-search-quality
/// (uuid:faeda9ac-5e9e-4091-8122-2fba9f80c8db))の 14 主題を英語と日英混在で言い直した
/// 28 問に、xhci のスクラッチパッド・CLI・SYSCALL・red zone・HPET の周期・RTL8139 の
/// TSD の 6 問を足したものである。低情報の後処理は生産経路と同じく融合の後に掛けた。
/// 原本のストアには書いていない。MRR の格子は次のとおりである。
///
/// | 深さ | k=10 | k=20 | k=60 |
/// |---|---|---|---|
/// | 10 | 0.722 | 0.722 | 0.722 |
/// | 30 | 0.737 | 0.738 | 0.731 |
/// | 50 | 0.736 | 0.736 | 0.731 |
/// | 100 | 0.733 | 0.736 | 0.731 |
///
/// 深さ 30・k = 20 は Recall@1 0.618・@3 0.824・@5 0.941・@10 0.971 で、12 組のどの指標
/// でも最良か同値だった(深さ 10 は @3 0.794・@10 0.941)。深さ 10 から 30 で拾えた
/// のは「System V AMD64 ABI argument registers calling convention」で、圏外から 5 位に
/// 入る。深さ 10 では、正解が意味検索の 11 位以降にいると、語の一致がいくら上位に
/// 置いても融合に入れなかった。
///
/// 深さ 50・100 とさらに広げると戻る。同じ問いが 50 で 7 位、100 で 8 位に下がる:
/// 深く採るほど、片方の方式にとって無関係な候補まで順位を持ち、両方式に現れる凡庸な
/// 件が上がってくる。天井は 30 のあたりにある。
///
/// 深さ 10 では k を 10 から 60 まで振っても 34 問の順位が 1 件も動かない。10 件しか
/// 融合しない形では和の大小関係が変わらないためで、k を測るには深さが要る。これが
/// 「深さと k を別々に選べない」ことの実測である。
///
/// 被覆のゲート(MIN_FUSION_COVERAGE)は深さで動かなかった。34 問の被覆は深さ
/// 10/30/50/100 ですべて同値で、片肺になる問い(10/34)もどの深さでも同じ 10 問である。
/// BM25 は多くのクエリ語に当たる件を上位に置くので、いちばんよく覆う件は深さ 10 の中に
/// すでにいる。
///
/// 深さは片肺の問いには効かない。「スクラッチパッド」は BM25 が 0 件なので融合の入力が
/// 意味検索だけになり、順位は意味検索そのままである。正解(xhci_1_2 p.334)が意味検索の
/// 21 位にいる以上、深さをいくら広げても上位 10 件には入らない(直すのはクエリ展開と
/// リランカーの側)。深さが効くのは、両方式が順位を持つ問いと、その先のリランカーへ渡す
/// 候補集合の側である。
///
/// 固定コーパス(node/tests/eval.rs)では 12 組すべてが同じ数値だった。23 チャンクしか
/// なく、BM25 の順位も 10 件に届かないので、深さを広げても融合の入力が増えない。この
/// 二つの定数は実データでしか選べない。
pub const RRF_DEPTH: usize = 30;

/// 融合に語の一致を入れる下限の被覆率(クエリの異なり語のうち、当たった語の割合)。
/// これを下回る順位は融合の入力にしない。
///
/// 境目は実測で選んだ(実測 2026-08-17。20260817-japanese-partial-match
/// (uuid:ff9299d5-3dfb-4ad0-9b3b-f5d597271cdc))。実データ(仕様書 PDF 25 本 + 論文 1 本)に
/// 対する問いの被覆は、はっきり二つに割れる:
/// - 英語の問い 7 本(2 語から 12 語まで): 0.90〜1.00。コーパスが英語なので、当たる
///   チャンクは問いの語をほぼ全部持っている。
/// - 日本語の部分一致: 0.25〜0.33。文字 bigram が複合語の断片として当たっている状態で、
///   「割り込み」が「取り込み」に当たる型である。
///
/// 谷は 0.33 と 0.90 のあいだにあり、0.5 はその中に置いた値である。
///
/// 得点を下げるだけでは足りないので、融合の入力から外す。RRF は順位しか見ないため、
/// 被覆の低い 1 件でも「BM25 の 1 位」として意味検索の 1 位と同じ重みを持ってしまう。
pub const MIN_FUSION_COVERAGE: f64 = 0.5;

/// 方式を指定しない要求で、埋め込みを備えた節点が embedding を選ぶ top_k の上限
/// (これを超えたら hybrid)。
///
/// 実データ(仕様書 PDF 25 本・25,098 チャンク)での実測 2026-08-17 が境目を決めた
/// (20260817-real-corpus-search-quality
/// (uuid:faeda9ac-5e9e-4091-8122-2fba9f80c8db))。14 主題を 3 通りの言い方(英語・日本語に
/// 英語術語を混ぜたもの・純日本語)で問い、低情報の後処理を効かせた状態で測った 3 通りの
/// 平均は、浅い打ち切りで embedding、深い打ち切りで hybrid が上に出る:
/// Recall@3 は 0.738 対 0.691(embedding が上)、Recall@5 は 0.809 対 0.833、
/// Recall@10 は 0.905 対 0.929(いずれも hybrid が上)である。交差点は k=3 と k=5 の
/// あいだにあり、境目をそこに置いた。
///
/// 言い方ごとの内訳は割れる。日本語に英語術語を混ぜた問いでは embedding の
/// Recall@1 0.786・Recall@3 1.000 に対して hybrid が 0.714・0.857 と落ちるが、英語の
/// 問いでは逆に hybrid の Recall@1 0.714 が embedding の 0.571 を上回る。深い側は割れず、
/// 英語の Recall@10 は hybrid 1.000 対 embedding 0.929 である。
///
/// 少ししか返さない要求は 1 位を外すと取り返せないので、そこは意味の近さだけで答える
/// (語の一致は、クエリが文書の語をそのまま持つときにしか効かない)。たくさん返す要求は
/// 読み手が下位まで見られるので、語の一致が拾う当たりを足す融合を採る。
pub const EMBEDDING_ONLY_TOP_K: usize = 3;

/// 方式を指定しない要求の既定(決め方の家はここだけ。should/0135)。埋め込みが無ければ
/// BM25 しかない。あるときは top_k で選ぶ(EMBEDDING_ONLY_TOP_K)。
pub fn default_method(has_embedding: bool, top_k: usize) -> SearchMethod {
    match (has_embedding, top_k <= EMBEDDING_ONLY_TOP_K) {
        (false, _) => SearchMethod::Bm25,
        (true, true) => SearchMethod::Embedding,
        (true, false) => SearchMethod::Hybrid,
    }
}

/// 埋め込み層の失敗。ストアの異常(ノードの故障)と、埋め込みサービスの側の失敗
/// (劣化して動き続けられる)を分ける。sync.rs の SyncError と同じ分け方である。
#[derive(Debug)]
pub enum EmbedError {
    Service(String),
    Store(StoreError),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Service(m) => write!(f, "embed: {m}"),
            EmbedError::Store(e) => write!(f, "store: {e}"),
        }
    }
}

impl From<StoreError> for EmbedError {
    fn from(e: StoreError) -> Self {
        EmbedError::Store(e)
    }
}

// ---- 検索の方式 ----

/// 検索の方式。要求の "method"、応答の "method" と "score_semantics" に出る文字列の家は
/// ここだけである(must/0023: 組み立てる側と読み取る側が同じ定数を見る)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMethod {
    /// 語の一致(BM25)。
    Bm25,
    /// 意味の近さ(全走査コサイン)。
    Embedding,
    /// 両者の順位を RRF で融合。
    Hybrid,
}

impl SearchMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchMethod::Bm25 => "bm25",
            SearchMethod::Embedding => "embedding",
            SearchMethod::Hybrid => "hybrid",
        }
    }

    /// 要求の文字列から方式を読む。知らない綴りは None(呼び手が理由を言って断る)。
    pub fn parse(text: &str) -> Option<SearchMethod> {
        match text {
            "bm25" => Some(SearchMethod::Bm25),
            "embedding" => Some(SearchMethod::Embedding),
            "hybrid" => Some(SearchMethod::Hybrid),
            _ => None,
        }
    }

    /// 得点の意味論。方式ごとに得点の出どころが違うので、応答は必ずこれを添える。
    /// どれも同一応答内の順位付けにだけ意味を持ち、応答をまたいだ比較には使えない。
    pub fn score_semantics(&self) -> &'static str {
        match self {
            // BM25 の得点(SEARCH の score_semantics)。
            SearchMethod::Bm25 => "bm25",
            // L2 正規化済みベクトルの内積 = コサイン。-1..=1。
            SearchMethod::Embedding => "cosine",
            // 順位から作った融合得点。BM25 の得点ともコサインとも意味が違う。
            SearchMethod::Hybrid => "rrf",
        }
    }
}

// ---- 埋め込みサーバのクライアント ----

/// 埋め込みサーバ 1 台ぶんの設定と往復。Clone なのは、serve が同じ相手に問いの文(短い
/// 期限)とコーパスの補完(長い期限)の 2 通りの期限で当たるためである。
#[derive(Clone)]
pub struct Embedder {
    /// 接続先(host:port)。
    address: String,
    /// 要求パス(/v1/embeddings)。
    path: String,
    /// 模型識別子。要求の "model" であり、ベクトルの鍵の半分(embedder_id)でもある。
    embedder_id: String,
    dimension: usize,
    timeout: Duration,
    batch: usize,
}

impl Embedder {
    /// URL と模型識別子から作る。識別子はキャッシュファイルの名前になるので、字種を
    /// 限る(パス区切りや空白を含む識別子を黙って受けない。must/0022)。
    pub fn new(url: &str, embedder_id: &str) -> Result<Embedder, String> {
        let (address, path) = http::split_http_url(url)?;
        // パスを省いた指定(http://host:port)は、この API の既定のパスを補う。
        let path = if path == "/" { "/v1/embeddings".to_string() } else { path };
        if embedder_id.is_empty()
            || !embedder_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(format!(
                "模型識別子 {embedder_id:?} は英数字と - _ . だけで書く(キャッシュの\
                 ファイル名になる)"
            ));
        }
        Ok(Embedder {
            address,
            path,
            embedder_id: embedder_id.to_string(),
            dimension: DEFAULT_EMBEDDING_DIMENSION,
            timeout: DEFAULT_EMBED_TIMEOUT,
            batch: DEFAULT_EMBED_BATCH,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Embedder {
        self.timeout = timeout;
        self
    }

    pub fn embedder_id(&self) -> &str {
        &self.embedder_id
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    pub fn batch_size(&self) -> usize {
        self.batch
    }

    /// 接続先の表示(劣化の理由に出す。どのサーバに届かなかったのかを言うため)。
    pub fn endpoint(&self) -> String {
        format!("http://{}{}", self.address, self.path)
    }

    /// テキストの列をベクトルの列にする。batch ごとに分けて送り、応答は index で並べ
    /// 替えるので、要求の順序がそのまま返る。
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let mut out = Vec::with_capacity(texts.len());
        for group in texts.chunks(self.batch.max(1)) {
            out.extend(self.embed_group(group)?);
        }
        Ok(out)
    }

    /// クエリ 1 本ぶん。
    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut vectors = self.embed(&[text.to_string()])?;
        match vectors.pop() {
            Some(vector) => Ok(vector),
            None => Err("埋め込みが 1 件も返らなかった".to_string()),
        }
    }

    /// 1 要求ぶん(要求の組み立てと応答の検証)。
    fn embed_group(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        // 要求本文は c1 の直列化で組む(文字列のエスケープの実装を増やさない。
        // should/0135)。c1 は JSON の部分集合なので、そのまま JSON として読める。
        let mut body = BTreeMap::new();
        body.insert(
            "input".to_string(),
            c1::Value::Array(texts.iter().map(|t| c1::Value::Text(t.clone())).collect()),
        );
        body.insert("model".to_string(), c1::Value::Text(self.embedder_id.clone()));
        let payload = c1::to_canonical_bytes(&c1::Value::Object(body));
        let response = http::post_json(&self.address, &self.path, &payload, self.timeout)
            .map_err(|e| format!("{}: {e}", self.endpoint()))?;
        if response.status != 200 {
            return Err(format!(
                "{} が {} を返した: {}",
                self.endpoint(),
                response.status,
                http::body_head(&response.body)
            ));
        }
        let text = std::str::from_utf8(&response.body)
            .map_err(|_| format!("{} の応答が UTF-8 でない", self.endpoint()))?;
        vectors_from_response(text, texts.len(), self.dimension)
            .map_err(|e| format!("{}: {e}", self.endpoint()))
    }
}

/// 応答からベクトルを取り出して要求の順に並べる。
///
/// 検査するのは三つ。data の件数が要求した件数と等しいこと、index が 0..件数 をちょうど
/// 一度ずつ覆うこと(配列の並びには依存しない)、各ベクトルの次元が模型の次元と等しい
/// こと。どれかが崩れていたら黙って通さず、何が違ったのかを言って失敗する(must/0022)。
fn vectors_from_response(
    text: &str,
    count: usize,
    dimension: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let value = Json::parse(text)?;
    let Json::Object(fields) = &value else {
        return Err("応答がオブジェクトでない".to_string());
    };
    let data = match fields.iter().find(|(name, _)| name == "data") {
        Some((_, Json::Array(items))) => items,
        Some(_) => return Err("応答の data が配列でない".to_string()),
        None => return Err("応答に data がない".to_string()),
    };
    if data.len() != count {
        return Err(format!("{count} 件を求めたのに data は {} 件", data.len()));
    }
    let mut slots: Vec<Option<Vec<f32>>> = (0..count).map(|_| None).collect();
    for item in data {
        let Json::Object(entry) = item else {
            return Err("data の要素がオブジェクトでない".to_string());
        };
        let index = match entry.iter().find(|(name, _)| name == "index") {
            Some((_, Json::Number(n))) if *n >= 0.0 && n.fract() == 0.0 => *n as usize,
            Some(_) => return Err("data の index が非負整数でない".to_string()),
            None => return Err("data の要素に index がない".to_string()),
        };
        let numbers = match entry.iter().find(|(name, _)| name == "embedding") {
            Some((_, Json::Array(numbers))) => numbers,
            Some(_) => return Err("data の embedding が配列でない".to_string()),
            None => return Err("data の要素に embedding がない".to_string()),
        };
        if index >= count {
            return Err(format!("index {index} が要求した {count} 件の範囲外"));
        }
        if numbers.len() != dimension {
            return Err(format!(
                "index {index} のベクトルが {} 次元(模型の次元は {dimension})",
                numbers.len()
            ));
        }
        let mut vector = Vec::with_capacity(dimension);
        for number in numbers {
            match number {
                Json::Number(n) => vector.push(*n as f32),
                _ => return Err(format!("index {index} のベクトルに数でない要素がある")),
            }
        }
        if slots[index].is_some() {
            return Err(format!("index {index} が二度現れた"));
        }
        slots[index] = Some(normalized(vector, index)?);
    }
    let mut out = Vec::with_capacity(count);
    for (index, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(vector) => out.push(vector),
            None => return Err(format!("index {index} のベクトルが応答に無い")),
        }
    }
    Ok(out)
}

/// L2 正規化(内積をそのままコサインとして使うための唯一の家。should/0135)。bge-m3 の
/// 応答は正規化済みだが、それを前提にせずここで揃える。方向を持たないベクトル(ノルムが
/// 0 か非有限)は通さない(must/0022)。
fn normalized(mut vector: Vec<f32>, index: usize) -> Result<Vec<f32>, String> {
    let norm = vector.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err(format!("index {index} のベクトルのノルムが {norm}(方向を持たない)"));
    }
    for value in &mut vector {
        *value = (f64::from(*value) / norm) as f32;
    }
    Ok(vector)
}

// ---- ベクトルの永続キャッシュ ----

/// キャッシュファイルの先頭行。版・模型識別子・次元を持ち、読み込み時に照合する。
const CACHE_MAGIC: &str = "uniqnode-vectors 1";

/// (チャンクのオブジェクト ID, embedder_id)→ ベクトルの導出層。
///
/// 置き場所を永続にしたのは規模の判断である: 実データのストアは 25,094 チャンクあり、
/// 1024 次元 f32 で約 100MB になる。100MB は RAM に載る(全走査コサインで十分)が、
/// 25k チャンクの埋め込み計算は GPU で分単位かかるので、プロセスを起こすたびに作り直す
/// わけにはいかない。ディスクに置いて起動のたびに読み直し、足りないぶんだけ計算する。
///
/// 鍵をチャンクのオブジェクト ID(内容ハッシュ)にしたので、文書を改版しても変わって
/// いないチャンクのベクトルはそのまま効く。二つの文書が同じ本文のチャンクを持てば、
/// ベクトルは 1 本で共有される。
pub struct VectorCache {
    /// None ならメモリだけ(評価ハーネスと単体テスト用)。
    path: Option<PathBuf>,
    embedder_id: String,
    dimension: usize,
    vectors: BTreeMap<String, Vec<f32>>,
    /// 読み込み時に捨てた末尾のバイト数。追記の途中で止まると生じうる(導出データなので
    /// 捨ててよいが、捨てたことは黙らない。must/0022)。
    discarded_tail_bytes: u64,
    /// 読めた記録の終わり(ファイル先頭からのバイト数)。捨てた末尾があるとき、次の追記は
    /// ここまで切り詰めてから書く。壊れた末尾の後ろに足すと、読み手は壊れた所で読むのを
    /// やめるので、足した記録が永久に見えなくなる(serve の裏の補完が同じチャンクを毎回
    /// 埋め直して、ファイルだけが伸びる)。
    valid_length: u64,
    /// 読み込んだ時点のファイルの見かけ。
    stamp: CacheStamp,
}

/// キャッシュファイルの見かけ(大きさと最終更新)。ベクトルが増えてもストアの世代は
/// 動かない(uniqnode embed はストアに何も書かない)ので、索引を作り直すかどうかの
/// 判断には世代のほかにこれが要る。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheStamp {
    length: u64,
    modified: Option<std::time::SystemTime>,
}

impl CacheStamp {
    /// 今のファイルの見かけ(無いファイルは既定値 = 空のキャッシュと同じ見かけ)。
    pub fn of(path: &Path) -> CacheStamp {
        match std::fs::metadata(path) {
            Ok(metadata) => {
                CacheStamp { length: metadata.len(), modified: metadata.modified().ok() }
            }
            Err(_) => CacheStamp::default(),
        }
    }
}

impl VectorCache {
    /// キャッシュファイルの場所(この導出の家はここだけ。CLI と serve が同じ場所を見る)。
    pub fn path_for(data_dir: &Path, embedder_id: &str) -> PathBuf {
        data_dir.join("derived").join("embeddings").join(format!("{embedder_id}.vec"))
    }

    /// メモリだけのキャッシュ。
    pub fn in_memory(embedder_id: &str, dimension: usize) -> VectorCache {
        VectorCache {
            path: None,
            embedder_id: embedder_id.to_string(),
            dimension,
            vectors: BTreeMap::new(),
            discarded_tail_bytes: 0,
            valid_length: 0,
            stamp: CacheStamp::default(),
        }
    }

    /// ファイルを読み込む(無ければ空)。模型識別子か次元が食い違うファイルは、黙って
    /// 別物のベクトルを混ぜずに断る(must/0022)。
    pub fn open(
        path: PathBuf,
        embedder_id: &str,
        dimension: usize,
    ) -> Result<VectorCache, EmbedError> {
        let mut cache = VectorCache {
            path: Some(path.clone()),
            embedder_id: embedder_id.to_string(),
            dimension,
            vectors: BTreeMap::new(),
            discarded_tail_bytes: 0,
            valid_length: 0,
            stamp: CacheStamp::of(&path),
        };
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cache),
            Err(e) => {
                return Err(EmbedError::Service(format!("{} を開けない: {e}", path.display())))
            }
        };
        let total = file
            .metadata()
            .map_err(|e| EmbedError::Service(format!("{} の大きさを見られない: {e}", path.display())))?
            .len();
        let mut reader = std::io::BufReader::new(file);
        let mut header = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            match reader.read_exact(&mut byte) {
                Ok(()) => {
                    if byte[0] == b'\n' {
                        break;
                    }
                    header.push(byte[0]);
                    if header.len() > 256 {
                        return Err(EmbedError::Service(format!(
                            "{} の見出し行が長すぎる",
                            path.display()
                        )));
                    }
                }
                Err(_) => {
                    return Err(EmbedError::Service(format!(
                        "{} の見出し行が読めない",
                        path.display()
                    )))
                }
            }
        }
        let header = String::from_utf8(header)
            .map_err(|_| EmbedError::Service(format!("{} の見出し行が UTF-8 でない", path.display())))?;
        let expected = format!("{CACHE_MAGIC} {embedder_id} {dimension}");
        if header != expected {
            return Err(EmbedError::Service(format!(
                "{} の見出しが {header:?}(期待は {expected:?})。模型か次元が違うベクトルは\
                 混ぜられない",
                path.display()
            )));
        }
        let mut consumed = expected.len() as u64 + 1;
        loop {
            match read_record(&mut reader, dimension) {
                Ok(Some(record)) => {
                    consumed += record.bytes;
                    cache.vectors.insert(record.chunk_id, record.vector);
                }
                Ok(None) => break,
                // 追記の途中で止まった末尾は捨てる(導出データなので作り直せる)。
                Err(_) => break,
            }
        }
        cache.discarded_tail_bytes = total.saturating_sub(consumed);
        cache.valid_length = consumed;
        Ok(cache)
    }

    pub fn embedder_id(&self) -> &str {
        &self.embedder_id
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    pub fn vector_count(&self) -> usize {
        self.vectors.len()
    }

    pub fn discarded_tail_bytes(&self) -> u64 {
        self.discarded_tail_bytes
    }

    pub fn get(&self, chunk_id: &str) -> Option<&[f32]> {
        self.vectors.get(chunk_id).map(|v| v.as_slice())
    }

    /// ベクトルをまとめて足し、ファイルにも追記する。追記は 1 まとまりごとに fsync する
    /// ので、途中で止まってもそこまでの計算は残る(25k チャンクの計算を最初からやり直さ
    /// ないため)。
    pub fn extend(&mut self, entries: Vec<(String, Vec<f32>)>) -> Result<(), EmbedError> {
        for (chunk_id, vector) in &entries {
            if vector.len() != self.dimension {
                return Err(EmbedError::Service(format!(
                    "チャンク {chunk_id} のベクトルが {} 次元(キャッシュは {} 次元)",
                    vector.len(),
                    self.dimension
                )));
            }
        }
        if let Some(path) = self.path.clone() {
            let written = self.append_to_file(&path, &entries)?;
            // 切り詰めた上に足したので、捨てた末尾はもう無い。
            self.discarded_tail_bytes = 0;
            self.valid_length += written;
        }
        for (chunk_id, vector) in entries {
            self.vectors.insert(chunk_id, vector);
        }
        Ok(())
    }

    /// 足したバイト数を返す(見出し行を含む)。
    fn append_to_file(
        &self,
        path: &Path,
        entries: &[(String, Vec<f32>)],
    ) -> Result<u64, EmbedError> {
        let io = |e: std::io::Error| EmbedError::Service(format!("{}: {e}", path.display()));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let fresh = !path.exists();
        let mut file =
            std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(io)?;
        // 捨てた末尾があれば、その手前まで切り詰めてから足す(壊れた記録の後ろに足した
        // 記録は読み手に届かない。valid_length のコメント)。
        if self.discarded_tail_bytes > 0 {
            file.set_len(self.valid_length).map_err(io)?;
        }
        let mut bytes = Vec::new();
        if fresh {
            bytes.extend_from_slice(
                format!("{CACHE_MAGIC} {} {}\n", self.embedder_id, self.dimension).as_bytes(),
            );
        }
        for (chunk_id, vector) in entries {
            let id_bytes = chunk_id.as_bytes();
            let id_length = u16::try_from(id_bytes.len()).map_err(|_| {
                EmbedError::Service(format!("チャンク ID {chunk_id} が長すぎる"))
            })?;
            let mut payload = Vec::with_capacity(id_bytes.len() + vector.len() * 4);
            payload.extend_from_slice(id_bytes);
            for value in vector {
                payload.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&id_length.to_le_bytes());
            bytes.extend_from_slice(&payload);
            bytes.extend_from_slice(&crate::crc32::crc32(&payload).to_le_bytes());
        }
        file.write_all(&bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        Ok(bytes.len() as u64)
    }
}

/// 読み出した 1 レコード。
struct CacheRecord {
    chunk_id: String,
    vector: Vec<f32>,
    /// このレコードが占めたバイト数(末尾の破損量を数えるため)。
    bytes: u64,
}

/// レコードを 1 件読む。Ok(None) はファイルの正常な終わり。壊れていれば Err で、
/// 呼び手はそこで読むのをやめる(前半は正しいので使う)。
fn read_record(
    reader: &mut impl Read,
    dimension: usize,
) -> Result<Option<CacheRecord>, std::io::Error> {
    let mut length = [0u8; 2];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let id_length = u16::from_le_bytes(length) as usize;
    let mut payload = vec![0u8; id_length + dimension * 4];
    reader.read_exact(&mut payload)?;
    let mut checksum = [0u8; 4];
    reader.read_exact(&mut checksum)?;
    if crate::crc32::crc32(&payload) != u32::from_le_bytes(checksum) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "crc が合わない"));
    }
    let chunk_id = String::from_utf8(payload[..id_length].to_vec())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "ID が UTF-8 でない"))?;
    let mut vector = Vec::with_capacity(dimension);
    for index in 0..dimension {
        let at = id_length + index * 4;
        let mut value = [0u8; 4];
        value.copy_from_slice(&payload[at..at + 4]);
        vector.push(f32::from_le_bytes(value));
    }
    Ok(Some(CacheRecord {
        chunk_id,
        vector,
        bytes: (2 + payload.len() + 4) as u64,
    }))
}

// ---- ベクトルの索引(全走査コサイン) ----

/// キャッシュに無いチャンクを埋め込んで足した結果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FillReport {
    /// 見えの索引対象チャンクの数。
    pub chunks: usize,
    /// そのうち相異なるチャンク(同じ本文は 1 個に畳まれる)。
    pub distinct_chunks: usize,
    pub already_cached: usize,
    pub embedded: usize,
}

/// キャッシュに無い見えのチャンク(fill_cache の「集める」半分の結果)。report には
/// chunks・distinct_chunks・already_cached が入り、embedded は 0 である。
pub struct MissingChunks {
    pub report: FillReport,
    /// (チャンクのオブジェクト ID, 本文)。埋め込みに掛けるのは本文である。
    pub chunks: Vec<(String, String)>,
}

/// 見えのチャンクを走査し、キャッシュに無いものの ID と本文を集める(ストアを読むだけで、
/// 埋め込みサーバは呼ばない)。ストアのロックを持つのはこの走査のあいだだけでよい:
/// serve はここをロックの中で呼び、ロックを離してから embed_missing で往復する(往復の
/// あいだストアを塞ぐと、1 本の補完で API 全体が止まる。run_search の問いの埋め込みと
/// 同じ規律)。
pub fn collect_missing(store: &Store, cache: &VectorCache) -> Result<MissingChunks, EmbedError> {
    let mut report = FillReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut chunks: Vec<(String, String)> = Vec::new();
    visit_indexable_chunks(store, &mut |chunk| {
        report.chunks += 1;
        if !seen.insert(chunk.id.clone()) {
            return;
        }
        report.distinct_chunks += 1;
        if cache.get(&chunk.id).is_some() {
            report.already_cached += 1;
        } else {
            chunks.push((chunk.id, chunk.text));
        }
    })?;
    Ok(MissingChunks { report, chunks })
}

/// 集めたチャンクを埋め込み、キャッシュに足す(fill_cache の「埋める」半分)。
/// まとまりごとに追記して fsync するので、途中で止めてもそこまでは残る。progress は
/// まとまりを 1 つ終えるたびに呼ばれる(25k チャンクは分単位かかるので、進みを黙って
/// いられる長さではない)。模型の照合はここ 1 箇所で行う(cache と embedder が出会うのは
/// ここである)。
pub fn embed_missing(
    embedder: &Embedder,
    cache: &mut VectorCache,
    missing: MissingChunks,
    progress: &mut dyn FnMut(&FillReport),
) -> Result<FillReport, EmbedError> {
    if embedder.embedder_id() != cache.embedder_id() {
        return Err(EmbedError::Service(format!(
            "模型 {} のベクトルを {} のキャッシュには足せない",
            embedder.embedder_id(),
            cache.embedder_id()
        )));
    }
    let MissingChunks { mut report, chunks } = missing;
    for group in chunks.chunks(embedder.batch_size()) {
        let texts: Vec<String> = group.iter().map(|(_, text)| text.clone()).collect();
        let vectors = embedder.embed(&texts).map_err(EmbedError::Service)?;
        // 件数がずれたまま zip すると、チャンクと違うベクトルを鍵に結びつけてしまう。
        if vectors.len() != group.len() {
            return Err(EmbedError::Service(format!(
                "{} 件を求めたのに {} 件のベクトルが返った",
                group.len(),
                vectors.len()
            )));
        }
        let entries: Vec<(String, Vec<f32>)> = group
            .iter()
            .map(|(id, _)| id.clone())
            .zip(vectors)
            .collect();
        report.embedded += entries.len();
        cache.extend(entries)?;
        progress(&report);
    }
    Ok(report)
}

/// 見えのチャンクのうちキャッシュに無いものを埋め込み、キャッシュに足す(collect_missing
/// と embed_missing を続けて呼ぶだけ)。CLI の uniqnode embed と評価ハーネスはストアを
/// 独占しているのでこの形でよい。serve は二つを別々に呼ぶ(EmbeddingService::fill_missing)。
pub fn fill_cache(
    store: &Store,
    embedder: &Embedder,
    cache: &mut VectorCache,
    progress: &mut dyn FnMut(&FillReport),
) -> Result<FillReport, EmbedError> {
    let missing = collect_missing(store, cache)?;
    embed_missing(embedder, cache, missing, progress)
}

/// 見えのチャンクに並びを合わせたベクトルの索引。BM25 の索引(SearchIndex)と同じ走査
/// (visit_indexable_chunks)から作るので、同じ世代なら同じチャンクが同じ位置に並ぶ。
/// この一致が、二つの方式の順位を位置で融合できる根拠である(照合は aligned_with)。
pub struct VectorIndex {
    generation: Generation,
    embedder_id: String,
    dimension: usize,
    /// チャンク位置ごとの data 内の先頭。キャッシュにベクトルが無ければ None。
    offsets: Vec<Option<usize>>,
    /// チャンクのオブジェクト ID(BM25 側との並びの照合に使う)。
    ids: Vec<String>,
    data: Vec<f32>,
    embedded: usize,
    /// 作った元のキャッシュファイルの見かけ(作り直しの判断に使う)。
    stamp: CacheStamp,
}

impl VectorIndex {
    /// キャッシュにあるベクトルだけで索引を作る(埋め込みサーバは呼ばない)。serve は
    /// この経路しか使わないので、検索要求が模型の計算を待つことはない。キャッシュに
    /// 無いチャンクは意味検索から漏れ、その事実は embedded_count が語る。
    pub fn from_cache(store: &Store, cache: &VectorCache) -> Result<VectorIndex, EmbedError> {
        let generation = Generation::current(store);
        let dimension = cache.dimension();
        let mut index = VectorIndex {
            generation,
            embedder_id: cache.embedder_id().to_string(),
            dimension,
            offsets: Vec::new(),
            ids: Vec::new(),
            data: Vec::new(),
            embedded: 0,
            stamp: cache.stamp.clone(),
        };
        let missing = visit_indexable_chunks(store, &mut |chunk| {
            match cache.get(&chunk.id) {
                Some(vector) if vector.len() == dimension => {
                    index.offsets.push(Some(index.data.len()));
                    index.data.extend_from_slice(vector);
                    index.embedded += 1;
                }
                _ => index.offsets.push(None),
            }
            index.ids.push(chunk.id);
        })?;
        // 世代の札は走査の取りこぼしを添えて閉じる(規則は crate::search::Generation。
        // BM25 の索引と同じ走査から作る以上、鮮度の判定も同じ規則でなければ、片方だけが
        // 作り直されて位置の対応(aligned_with)が崩れる)。
        index.generation.note_missing(missing);
        Ok(index)
    }

    pub fn is_current(&self, store: &Store) -> bool {
        self.generation.matches(store)
    }

    pub fn embedder_id(&self) -> &str {
        &self.embedder_id
    }

    /// 索引が見ているチャンクの数(BM25 の索引と同じはず)。
    pub fn chunk_count(&self) -> usize {
        self.offsets.len()
    }

    /// そのうちベクトルを持つチャンクの数。被覆率であり、劣化の観測点である。
    pub fn embedded_count(&self) -> usize {
        self.embedded
    }

    /// BM25 の索引と同じチャンク列を同じ並びで見ているか。位置で順位を融合する前に
    /// 確かめる: 世代が同じなら構造上そうなるが、取り違えた引用を返すよりは、確かめて
    /// 劣化する方を選ぶ。
    pub fn aligned_with(&self, lexical: &SearchIndex) -> bool {
        if self.ids.len() != lexical.chunk_count() {
            return false;
        }
        self.ids.iter().enumerate().all(|(position, id)| lexical.chunk(position).id == *id)
    }

    fn vector_at(&self, position: usize) -> Option<&[f32]> {
        let offset = self.offsets[position]?;
        Some(&self.data[offset..offset + self.dimension])
    }

    /// 全走査コサインで top_k 件。ベクトルは L2 正規化済みなので内積がそのままコサイン
    /// である。コレクションの絞り込みは BM25 側の索引から引く(引用の情報を二重に持たない
    /// ため)。同点は位置の昇順で安定に決める(should/0125 の決定性)。
    pub fn search(
        &self,
        query_vector: &[f32],
        lexical: &SearchIndex,
        scope: &CollectionScope,
        top_k: usize,
    ) -> Vec<ScoredChunk> {
        let mut ranked: Vec<ScoredChunk> = Vec::new();
        for position in 0..self.offsets.len() {
            let Some(vector) = self.vector_at(position) else { continue };
            if !scope.allows(&lexical.chunk(position).collection) {
                continue;
            }
            let score: f64 = vector
                .iter()
                .zip(query_vector)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            ranked.push(ScoredChunk { position, score });
        }
        ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.position.cmp(&b.position)));
        ranked.truncate(top_k);
        ranked
    }
}

// ---- 順位の融合(RRF) ----

/// 融合の結果 1 件。
pub struct FusedItem<T> {
    pub item: T,
    /// 順位から作った得点(1/(k + 順位) の和)。方式をまたいだ得点の較正は要らない。
    pub score: f64,
}

/// 順位ベースの融合(Reciprocal Rank Fusion)。各方式の順位 r(1 始まり)に 1/(k + r) を
/// 与えて足し、和の降順に並べる。得点そのものを見ないので、BM25 の得点とコサインを較正
/// する必要がない(これが順位ベースを採る理由である)。同点は要素の順で安定に決める
/// (should/0125 の決定性)。
///
/// 融合の家はここだけである(should/0135)。生産経路(チャンクの位置を融合する)と評価
/// ハーネス(チャンクの名前を融合する)が、同じこの関数を呼ぶ。
pub fn fuse_by_rank<T: Clone + Ord>(rankings: &[Vec<T>], top_k: usize) -> Vec<FusedItem<T>> {
    let mut scores: BTreeMap<T, f64> = BTreeMap::new();
    for ranking in rankings {
        for (index, item) in ranking.iter().enumerate() {
            let rank = (index + 1) as f64;
            *scores.entry(item.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank);
        }
    }
    let mut fused: Vec<FusedItem<T>> =
        scores.into_iter().map(|(item, score)| FusedItem { item, score }).collect();
    fused.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.item.cmp(&b.item)));
    fused.truncate(top_k);
    fused
}

// ---- ハイブリッド検索 ----

/// クエリを埋め込んだ結果。呼び手はこれを作ってから検索に入る。
///
/// 検索の外で作らせるのは、ロックの規律のためである(node/src/sync.rs の sync_from_peer に
/// 同じ規律がある): 埋め込みは埋め込みサーバとの往復であり、その待ちのあいだ store の
/// ロックを持っていると、1 本の検索の待ちで API 全体が塞がる。だから呼び手はロックを
/// 取る前にここまで済ませ、索引の参照を取ってからは待たない。
pub enum QueryEmbedding {
    Ready(Vec<f32>),
    /// 使えない理由(埋め込みが設定されていない・届かない・応答が壊れている)。
    Unavailable(String),
}

impl QueryEmbedding {
    /// 埋め込みサーバがあればクエリを埋め込む。失敗は理由として持ち帰り、ここでは
    /// 落とさない(呼び手は BM25 に劣化して答え続ける)。
    pub fn of(embedder: Option<&Embedder>, query: &str) -> QueryEmbedding {
        let Some(embedder) = embedder else {
            return QueryEmbedding::Unavailable(
                "埋め込みサーバが設定されていない(serve の --embed)".to_string(),
            );
        };
        match embedder.embed_query(query) {
            Ok(vector) if vector.len() == embedder.dimension() => QueryEmbedding::Ready(vector),
            Ok(vector) => QueryEmbedding::Unavailable(format!(
                "クエリのベクトルが {} 次元(模型の次元は {})",
                vector.len(),
                embedder.dimension()
            )),
            Err(reason) => QueryEmbedding::Unavailable(reason),
        }
    }
}

/// 一度の検索が使える装備。ベクトルの索引は、設定されていないことも、まだ作られて
/// いないこともある。
pub struct HybridSearch<'a> {
    pub lexical: &'a SearchIndex,
    pub vectors: Option<&'a VectorIndex>,
}

/// 検索 1 回の結果。要求した方式で答えられなかったときは、method が実際に使った方式に
/// なり、degraded がその理由を持つ(黙って劣化しない。should/0128)。
pub struct RankedSearch {
    pub method: SearchMethod,
    pub degraded: Option<String>,
    pub hits: Vec<ScoredChunk>,
}

impl HybridSearch<'_> {
    /// 方式を指定して上位 top_k 件を返す。意味検索が使えない場合(未設定・ベクトルが
    /// 無い・索引がずれている・サーバに届かない)は BM25 だけに落ちて、理由を残す。
    pub fn ranked(
        &self,
        requested: SearchMethod,
        query: &str,
        embedding: &QueryEmbedding,
        scope: &CollectionScope,
        top_k: usize,
    ) -> RankedSearch {
        self.ranked_with(requested, query, embedding, scope, top_k)
    }

    /// 語の一致に使うクエリを明示する形。カタカナの術語を索引に実在する英語語へ寄せた
    /// クエリを渡すために要る(node/src/translit.rs)。
    ///
    /// 意味検索の側にこのクエリは渡らない。埋め込みは呼び手がロックの外で既に作って
    /// あり(引数の embedding)、多言語の模型は日本語の問いをそのまま扱えるからである。
    /// 寄せた語を意味検索にも混ぜると、かえって問いの意味が薄まる。
    ///
    /// 被覆率(MIN_FUSION_COVERAGE)は、ここに渡したクエリについて数える。寄せた語が
    /// 当たったかどうかが、まさに測りたいことだからである。
    pub fn ranked_with(
        &self,
        requested: SearchMethod,
        lexical_query: &str,
        embedding: &QueryEmbedding,
        scope: &CollectionScope,
        top_k: usize,
    ) -> RankedSearch {
        let query = lexical_query;
        if requested == SearchMethod::Bm25 {
            return RankedSearch {
                method: SearchMethod::Bm25,
                degraded: None,
                hits: self.lexical.search_positions(query, scope, top_k),
            };
        }
        let semantic = match self.semantic_hits(embedding, scope, top_k) {
            Ok(hits) => hits,
            Err(reason) => {
                return RankedSearch {
                    method: SearchMethod::Bm25,
                    degraded: Some(reason),
                    hits: self.lexical.search_positions(query, scope, top_k),
                }
            }
        };
        // ベクトルが一部のチャンクにしか無いときは答えは返せるが、意味検索が全体を見て
        // いないので、被覆率を添える。
        let coverage = self.vectors.and_then(|vectors| {
            let (embedded, total) = (vectors.embedded_count(), vectors.chunk_count());
            (embedded < total).then(|| {
                format!(
                    "ベクトルは {embedded}/{total} チャンクぶんしかない(残りは意味検索の\
                     対象外。serve は書き込みの後に裏で埋める。ストアを直接開く形では \
                     uniqnode embed で埋める)"
                )
            })
        });
        match requested {
            SearchMethod::Embedding => RankedSearch {
                method: SearchMethod::Embedding,
                degraded: coverage,
                hits: semantic.into_iter().take(top_k).collect(),
            },
            _ => {
                let lexical_ranking =
                    self.lexical.ranked_lexical(query, scope, fusion_depth(top_k));
                // 融合の入力が片方だけになるとき、順位は埋め込み単独とまったく同じ列に
                // なる。融合が効いているように見えたまま片肺で答えないように、そう言う
                // (黙って劣化しない。should/0128)。理由は二つある: 語が 1 つも当たら
                // なかったときと、当たったのが問いの一部だけだったときである。
                let one_sided = if lexical_ranking.hits.is_empty() {
                    // 実データの純日本語の問いでは 14/14 でこれが起きた(実測 2026-08-17)。
                    Some(
                        "BM25 が 1 語も一致せず、順位は埋め込み単独と同じである(融合は効いて\
                         いない。文書が使う語をクエリに入れると語の一致も効く)"
                            .to_string(),
                    )
                } else if lexical_ranking.coverage() < MIN_FUSION_COVERAGE {
                    Some(format!(
                        "BM25 は問いの語のまとまりを {:.2} しか覆えないので(問いの語は \
                         {} 個)、融合に入れていない。順位は埋め込み単独と同じである\
                         (文書が使う語をクエリに入れると語の一致も効く)",
                        lexical_ranking.coverage(),
                        lexical_ranking.query_terms
                    ))
                } else {
                    None
                };
                // 被覆の足りない順位は融合に入れない。入れると、クエリ語の一部にしか当たって
                // いない 1 件が BM25 側の 1 位として、意味検索の 1 位と同じ重みで先頭を争う
                // (RRF は順位しか見ないので、得点をいくら下げても順位は動かない)。
                let lexical =
                    if one_sided.is_some() { Vec::new() } else { lexical_ranking.hits };
                let degraded = match (coverage, one_sided) {
                    (Some(coverage), Some(one_sided)) => Some(format!("{coverage}。{one_sided}")),
                    (found, None) | (None, found) => found,
                };
                let rankings = vec![
                    lexical.iter().map(|hit| hit.position).collect::<Vec<usize>>(),
                    semantic.iter().map(|hit| hit.position).collect::<Vec<usize>>(),
                ];
                let hits = fuse_by_rank(&rankings, top_k)
                    .into_iter()
                    .map(|fused| ScoredChunk { position: fused.item, score: fused.score })
                    .collect();
                RankedSearch { method: SearchMethod::Hybrid, degraded, hits }
            }
        }
    }

    /// 意味検索の順位。使えない理由があれば、その理由を Err で返す(呼び手が劣化を
    /// 記録する)。
    fn semantic_hits(
        &self,
        embedding: &QueryEmbedding,
        scope: &CollectionScope,
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>, String> {
        let Some(vectors) = self.vectors else {
            return Err("ベクトルの索引がない".to_string());
        };
        if vectors.embedded_count() == 0 {
            return Err(format!(
                "模型 {} のベクトルが 1 件も無い(serve は書き込みの後に裏で作る。ストアを\
                 直接開く形では uniqnode embed <dir> で作る)",
                vectors.embedder_id()
            ));
        }
        if !vectors.aligned_with(self.lexical) {
            return Err("ベクトルの索引が BM25 の索引と揃っていない".to_string());
        }
        let query_vector = match embedding {
            QueryEmbedding::Ready(vector) => vector,
            QueryEmbedding::Unavailable(reason) => return Err(reason.clone()),
        };
        Ok(vectors.search(query_vector, self.lexical, scope, fusion_depth(top_k)))
    }
}

/// 融合に持ち込む各方式の候補数。
fn fusion_depth(top_k: usize) -> usize {
    top_k.max(RRF_DEPTH)
}

/// serve が持つ埋め込みの装備一式(--embed が与えられたときだけ存在する)。ベクトルの
/// 索引は BM25 の索引と同じく導出データの遅延キャッシュで、世代がずれたら次の要求で
/// 作り直す。
pub struct EmbeddingService {
    /// 問いの文を埋め込む相手(短い期限。QUERY_EMBED_TIMEOUT)。
    pub embedder: Embedder,
    /// コーパスの補完に使う同じ相手(まとまりごとの長い期限。DEFAULT_EMBED_TIMEOUT)。
    corpus_embedder: Embedder,
    pub cache_path: PathBuf,
    pub index: std::sync::Mutex<Option<VectorIndex>>,
}

impl EmbeddingService {
    pub fn new(data_dir: &Path, embedder: Embedder) -> EmbeddingService {
        let cache_path = VectorCache::path_for(data_dir, embedder.embedder_id());
        let corpus_embedder = embedder.clone().with_timeout(DEFAULT_EMBED_TIMEOUT);
        EmbeddingService {
            embedder,
            corpus_embedder,
            cache_path,
            index: std::sync::Mutex::new(None),
        }
    }

    /// 手元の索引が今も最新か。ストアの世代(見えの変化)と、キャッシュファイルの
    /// 見かけ(ベクトルの増加)の両方を見る。ベクトルを足してもストアには何も書かない
    /// (裏の補完も uniqnode embed も)ので、世代だけを見ていると足したベクトルに気づけない。
    pub fn index_is_current(&self, index: &VectorIndex, store: &Store) -> bool {
        index.is_current(store) && index.stamp == CacheStamp::of(&self.cache_path)
    }

    /// キャッシュファイルを読む(索引を作るときも補完するときもここを通る)。捨てた末尾が
    /// あれば黙らない(must/0022)。捨てたぶんは次の補完が埋め直す。
    pub(crate) fn open_cache(&self) -> Result<VectorCache, EmbedError> {
        let cache = VectorCache::open(
            self.cache_path.clone(),
            self.embedder.embedder_id(),
            self.embedder.dimension(),
        )?;
        if cache.discarded_tail_bytes() > 0 {
            crate::log_line!(
                "uniqnode: {} の末尾 {} バイトを捨てた(追記の途中で止まった記録。次の補完が\
                 そこから埋め直す)",
                self.cache_path.display(),
                cache.discarded_tail_bytes()
            );
        }
        Ok(cache)
    }

    /// キャッシュファイルを読んでベクトルの索引を作る(要求のたびにではなく、世代か
    /// キャッシュの見かけがずれたときだけ呼ばれる)。埋め込みサーバは呼ばない。
    pub fn load_index(&self, store: &Store) -> Result<VectorIndex, EmbedError> {
        let cache = self.open_cache()?;
        VectorIndex::from_cache(store, &cache)
    }

    /// 見えのチャンクのうちベクトルの無いものを埋めてキャッシュに足す(serve の裏の
    /// 補完。api.rs の start_index_warmer が温めの後に呼ぶ)。ストアのロックを持つのは
    /// 「無いチャンクを集める」走査のあいだだけで、埋め込みサーバとの往復はロックを
    /// 離してから行う(往復は分単位かかりうる。その間も検索と書き込みは通る)。
    ///
    /// 走査と往復のあいだに書き込みが入っても正しさは崩れない: 鍵はチャンクの内容
    /// ハッシュなので、消えたチャンクのベクトルは無駄になるだけで、混ざらない。増えた
    /// チャンクは次の合図が拾う。
    pub fn fill_missing(
        &self,
        store: &std::sync::Mutex<Store>,
        progress: &mut dyn FnMut(&FillReport),
    ) -> Result<FillReport, EmbedError> {
        let mut cache = self.open_cache()?;
        let missing = {
            let store = store.lock().expect("lock");
            collect_missing(&store, &cache)?
        };
        embed_missing(&self.corpus_embedder, &mut cache, missing, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 応答は index の順に並べ替えられ、配列の並びには依存しない。次元は模型のものと
    /// 突き合わせる。期待値はリテラルで書く(should/0137)。
    #[test]
    fn a_response_is_reordered_by_index_and_checked_against_the_dimension() {
        // 配列は index の降順で並んでいるが、返るのは要求の順(index 昇順)。
        let text = "{\"data\":[{\"index\":1,\"embedding\":[0.0,3.0]},\
                    {\"index\":0,\"embedding\":[4.0,0.0]}],\"model\":\"bge-m3\"}";
        let vectors = vectors_from_response(text, 2, 2).expect("2 件 2 次元");
        // L2 正規化されるので、長さ 4 と 3 のベクトルは単位ベクトルになる。
        assert_eq!(vectors[0], vec![1.0, 0.0]);
        assert_eq!(vectors[1], vec![0.0, 1.0]);

        // 次元が違う応答は通さない(黙って短いベクトルを使わない)。
        let error = vectors_from_response(text, 2, 1024).expect_err("次元違いは失敗すべき");
        assert!(error.contains("2 次元"), "{error}");
        assert!(error.contains("1024"), "{error}");

        // 件数が足りない・index が重なる・data が無い応答も、理由を言って落ちる。
        let error = vectors_from_response(text, 3, 2).expect_err("件数違いは失敗すべき");
        assert!(error.contains("3 件を求めた"), "{error}");
        let duplicated = "{\"data\":[{\"index\":0,\"embedding\":[1.0,0.0]},\
                          {\"index\":0,\"embedding\":[0.0,1.0]}]}";
        let error = vectors_from_response(duplicated, 2, 2).expect_err("index の重複");
        assert!(error.contains("二度現れた"), "{error}");
        let error = vectors_from_response("{\"model\":\"x\"}", 1, 2).expect_err("data 無し");
        assert!(error.contains("data がない"), "{error}");
        // ノルム 0 のベクトルは方向を持たないので通さない。
        let zero = "{\"data\":[{\"index\":0,\"embedding\":[0.0,0.0]}]}";
        let error = vectors_from_response(zero, 1, 2).expect_err("ノルム 0");
        assert!(error.contains("ノルム"), "{error}");
    }

    /// 方式を指定しない要求の既定。埋め込みが無ければ BM25 しかなく、あるときは top_k で
    /// 分かれる(境目 EMBEDDING_ONLY_TOP_K の根拠は定数のコメントにある実測)。期待値は
    /// リテラルで書く(検査対象から導出しない。should/0137)。
    #[test]
    fn the_default_method_follows_the_equipment_and_how_many_results_are_asked_for() {
        assert_eq!(default_method(false, 1).as_str(), "bm25");
        assert_eq!(default_method(false, 10).as_str(), "bm25");
        assert_eq!(default_method(false, 1000).as_str(), "bm25");
        assert_eq!(default_method(true, 1).as_str(), "embedding");
        assert_eq!(default_method(true, 3).as_str(), "embedding");
        assert_eq!(default_method(true, 4).as_str(), "hybrid");
        assert_eq!(default_method(true, 10).as_str(), "hybrid");
        // 境目は定数のとおり(定数を動かしたらこの 2 行が意味を保つ)。
        assert_eq!(default_method(true, EMBEDDING_ONLY_TOP_K).as_str(), "embedding");
        assert_eq!(default_method(true, EMBEDDING_ONLY_TOP_K + 1).as_str(), "hybrid");
    }

    /// RRF: 期待値は手計算のリテラル(should/0137)。片方の方式が空の列を返す場合
    /// (BM25 が空振りするクエリ)も、もう片方の順位がそのまま残る。
    #[test]
    fn reciprocal_rank_fusion_matches_hand_computed_literals() {
        let bm25 = vec!["a", "b", "c"];
        let semantic = vec!["c", "d", "a"];
        let fused = fuse_by_rank(&[bm25.clone(), semantic.clone()], 10);
        // k = 20 なので、a: 1/21 + 1/23 = 0.091108…、c: 1/23 + 1/21 = 同じ値、b: 1/22、
        // d: 1/22。a と c は同点なので、要素の順(a < c)で決まる。
        let names: Vec<&str> = fused.iter().map(|item| item.item).collect();
        assert_eq!(names, vec!["a", "c", "b", "d"]);
        assert!((fused[0].score - (1.0 / 21.0 + 1.0 / 23.0)).abs() < 1e-12, "{}", fused[0].score);
        assert!((fused[2].score - (1.0 / 22.0)).abs() < 1e-12, "{}", fused[2].score);
        // 両方に出る要素は、片方だけで 1 位の要素を追い越せる。
        let fused = fuse_by_rank(&[vec!["x", "y"], vec!["y", "z"]], 10);
        let names: Vec<&str> = fused.iter().map(|item| item.item).collect();
        assert_eq!(names, vec!["y", "x", "z"], "両方式が挙げた y が先頭に来るべき");
        // 片方が空でも、もう片方の順位がそのまま残る。
        let fused = fuse_by_rank(&[Vec::new(), vec!["p", "q"]], 10);
        let names: Vec<&str> = fused.iter().map(|item| item.item).collect();
        assert_eq!(names, vec!["p", "q"]);
        // top_k で切る。
        assert_eq!(fuse_by_rank(&[vec!["p", "q", "r"]], 2).len(), 2);
    }

    /// キャッシュのファイルは、書いて読み直すと同じベクトルを返す。模型か次元の違う
    /// ファイルは開かない(別の模型のベクトルを混ぜない)。
    #[test]
    fn the_cache_file_round_trips_and_refuses_a_foreign_model() {
        let dir = std::env::temp_dir().join(format!("uniqnode-embed-cache-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        let path = VectorCache::path_for(&dir, "test-model");
        assert!(path.ends_with("derived/embeddings/test-model.vec"), "{}", path.display());

        let mut cache = VectorCache::open(path.clone(), "test-model", 2).expect("open");
        assert_eq!(cache.vector_count(), 0, "無いファイルは空のキャッシュ");
        cache
            .extend(vec![
                ("s256:aa".to_string(), vec![1.0, 0.0]),
                ("s256:bb".to_string(), vec![0.0, 1.0]),
            ])
            .expect("extend");
        let reopened = VectorCache::open(path.clone(), "test-model", 2).expect("reopen");
        assert_eq!(reopened.vector_count(), 2);
        assert_eq!(reopened.get("s256:aa"), Some(&[1.0f32, 0.0][..]));
        assert_eq!(reopened.discarded_tail_bytes(), 0);
        // 追記は既存の記録を保つ。
        let mut reopened = reopened;
        reopened.extend(vec![("s256:cc".to_string(), vec![0.5, 0.5])]).expect("extend");
        assert_eq!(VectorCache::open(path.clone(), "test-model", 2).expect("open").vector_count(), 3);

        // 別の模型・別の次元では開かない。
        let refusal = |embedder_id: &str, dimension: usize| -> String {
            match VectorCache::open(path.clone(), embedder_id, dimension) {
                Ok(_) => panic!("模型 {embedder_id}・{dimension} 次元では開けてはならない"),
                Err(error) => format!("{error}"),
            }
        };
        let error = refusal("other-model", 2);
        assert!(error.contains("見出し"), "{error}");
        let error = refusal("test-model", 3);
        assert!(error.contains("見出し"), "{error}");

        // 追記の途中で止まった末尾(半端なバイト列)は捨てて、前半は使う。
        {
            let mut file =
                std::fs::OpenOptions::new().append(true).open(&path).expect("append");
            file.write_all(b"\x02\x00broken").expect("write");
        }
        let torn = VectorCache::open(path.clone(), "test-model", 2).expect("open");
        assert_eq!(torn.vector_count(), 3, "前半の記録は残るべき");
        assert_eq!(torn.discarded_tail_bytes(), 8, "捨てた末尾のバイト数を数えるべき");

        // 捨てた末尾の後ろに足すのではなく、切り詰めてから足す。壊れた記録の後ろに足すと、
        // 読み手は壊れた所で読むのをやめるので足した記録が見えない(serve の裏の補完が
        // 同じチャンクを毎回埋め直すことになる)。欠陥を戻す(set_len を消す)と、読み直しが
        // 3 件のまま末尾を 8 + 20 バイト捨てて、ここで落ちる(should/0137)。
        let mut torn = torn;
        torn.extend(vec![("s256:dd".to_string(), vec![0.0, 1.0])]).expect("extend after tear");
        assert_eq!(torn.discarded_tail_bytes(), 0, "切り詰めた後は捨てた末尾が無い");
        let healed = VectorCache::open(path.clone(), "test-model", 2).expect("open");
        assert_eq!(healed.vector_count(), 4, "切り詰めた上に足した記録は読めるべき");
        assert_eq!(healed.discarded_tail_bytes(), 0, "壊れた末尾は消えているべき");
        assert_eq!(healed.get("s256:dd"), Some(&[0.0f32, 1.0][..]));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 模型識別子はキャッシュのファイル名になるので、パス区切りを含む識別子は断る。
    #[test]
    fn an_embedder_id_that_would_escape_the_cache_directory_is_refused() {
        assert!(Embedder::new(DEFAULT_EMBEDDING_URL, "../etc/passwd").is_err());
        assert!(Embedder::new(DEFAULT_EMBEDDING_URL, "").is_err());
        let embedder = Embedder::new(DEFAULT_EMBEDDING_URL, "bge-m3").expect("new");
        assert_eq!(embedder.embedder_id(), "bge-m3");
        assert_eq!(embedder.endpoint(), DEFAULT_EMBEDDING_URL);
        assert_eq!(embedder.dimension(), DEFAULT_EMBEDDING_DIMENSION);
    }
}

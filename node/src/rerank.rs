//! cross-encoder リランカー(bge-reranker-v2-m3)への最小クライアントと、順位の取り直し。
//!
//! 検索(node/src/embed.rs の BM25・意味検索・RRF 融合)が挙げた候補を、問いと本文の組を
//! そのまま模型に読ませて採点し直す層である。埋め込みは問いと文書を別々にベクトルにして
//! から比べるので、どちらの側にも現れない語で結ばれた組(問いの言い方と文書の言い方が
//! 違う場合)を取り違える。cross-encoder は組ごとに 1 回推論するので候補の数だけ計算が
//! 要るが、上位 30 件に限れば手元の GPU で 1 秒未満に収まる(下の実測)。
//!
//! 効き目の実測(2026-08-18。実データの仕様書 PDF 25 本のストア): 問い「スクラッチ
//! パッド」に対する正解 xhci_1_2 p.334 は意味検索の 21 位にいて、融合の深さをいくら
//! 広げても上位 10 件に入らなかった(node/src/embed.rs の RRF_DEPTH のコメント)。同じ
//! 候補 30 件をこの層で再採点すると 2 位に上がる。深さでは直せない片肺の問いを直すのが
//! この層の役目である。
//!
//! 得点は 0..1 の確からしさではない。実測で 1.084 / -0.66 / -1.36 / -4.45 のような符号
//! つきの値(logit)が返る。同一応答内の順位付けにしか意味を持たず、閾値に使ってはなら
//! ない(応答をまたいだ比較にも使えない)。呼び手は得点を出すときに必ず意味論
//! (RERANK_SCORE_SEMANTICS)を添える(node/src/embed.rs の score_semantics と同じ扱い)。
//!
//! 模型に送るのは既定では抜粋(検索索引が持つ 200 文字。node/src/search.rs の
//! SNIPPET_CHAR_LIMIT)である。同じ問いで抜粋と全文を較べたところ上位 3 件の並びが一致
//! したので(実測 2026-08-18)、全文を取りに行く往復を足す理由が無い。抜粋なら
//! bge-reranker-v2-m3 の 512 トークンの入力上限にも当たらない。全文を送る形を選ぶ呼び手は
//! (この層の Reranker::rerank に自分で組んだ documents を渡せば送れる)、llama-server に
//! `--batch-size 2048` が要ることに注意する: 既定の 512 のままだと 757 トークンの入力が
//! 「input is too large to process, increase the physical batch size」で落ちる(実測)。
//!
//! 速度の実測(暖機後、RTX 2080): 10 件 0.66 秒、20 件 0.68 秒、30 件 0.82 秒。件数には
//! ほぼ比例しない(1 回の推論に候補をまとめて載せられるため)ので、深さは 30 でよい。
//! 模型を読み込む初回だけ 3.5 秒かかる。期限(DEFAULT_RERANK_TIMEOUT)はその初回に
//! 間に合う長さで採ってある。
//!
//! 起動時に相手の生存は確かめない(should/0114)。リランカーが後から起きても、次の検索
//! から効き始める。
//!
//! 届かないときは黙って劣化しない(should/0128): 順位は元のまま(リランカー無しの順位)
//! で答え続け、何が起きて取り直せなかったのかを Reranked::degraded が持ち帰る。検索を
//! 失敗させないのは、リランカーが落ちている間も語の一致と意味検索は答えられるからで
//! ある(埋め込みの QueryEmbedding::Unavailable と同じ判断)。

use crate::c1;
use crate::http;
use crate::json::Json;
use std::collections::BTreeMap;
use std::time::Duration;

/// 既定のリランカーのエンドポイント(手元の llama-server)。起こし方は次のとおりで、
/// --reranking と --pooling rank が要る(埋め込みサーバとは別のプロセス・別のポート)。
///
/// ```text
/// llama-server --model /work2/llm/models/bge-reranker-v2-m3/bge-reranker-v2-m3-FP16.gguf \
///   --host 127.0.0.1 --port 8084 --reranking --pooling rank -ngl 99 --ctx-size 8192 \
///   --batch-size 2048 --ubatch-size 2048
/// ```
///
/// 要求の組み立てと、テストや CLI が既定として使う文字列の家はここだけである
/// (must/0023)。
pub const DEFAULT_RERANK_URL: &str = "http://127.0.0.1:8084/v1/rerank";
/// 既定の模型識別子。要求の "model" である。
pub const DEFAULT_RERANKER_ID: &str = "bge-reranker-v2-m3";
/// 再採点する候補の数の既定。
///
/// 30 なのは、上流の融合が返す深さ(node/src/embed.rs の RRF_DEPTH)と揃えたからである。
/// そこより深く採っても融合の側が候補を持っていないし、30 件の再採点は 0.82 秒で、10 件
/// (0.66 秒)との差は 0.16 秒しかない(実測)。取りこぼしを減らす方に振れる。
pub const DEFAULT_RERANK_DEPTH: usize = 30;
/// 1 要求ぶんの期限。暖機後は 30 件で 0.82 秒だが、模型を読み込む初回だけ 3.5 秒かかる
/// (実測)ので、その初回に間に合う長さを採る。検索要求の待ちなので長くしすぎない
/// (期限のない待ちを作らない。should/0104)。
pub const DEFAULT_RERANK_TIMEOUT: Duration = Duration::from_secs(20);

/// 取り直した得点の意味論。応答の score_semantics に出る文字列の家はここだけである
/// (must/0023)。"cosine" や "rrf" と違って範囲を持たない符号つきの値(logit)であり、
/// 同一応答内の順位付けにしか使えないことを名前で言う。
pub const RERANK_SCORE_SEMANTICS: &str = "reranker-logit";

// ---- リランカーのクライアント ----

/// リランカー 1 台ぶんの設定と往復。
pub struct Reranker {
    /// 接続先(host:port)。
    address: String,
    /// 要求パス(/v1/rerank)。
    path: String,
    /// 模型識別子。要求の "model" である。
    reranker_id: String,
    timeout: Duration,
    depth: usize,
}

impl Reranker {
    /// URL と模型識別子から作る(Embedder::new と同じ流儀)。
    ///
    /// 識別子の字種を限るのは、埋め込みと違ってファイル名になるからではない(この層は
    /// 何も永続しない)。理由は二つある: 綴りを間違えた識別子(パスや空白や改行を含む
    /// もの)を、要求の本文に載せて相手の 500 として返ってくるまで気づかない形にしない
    /// ことと、識別子が劣化の理由とログの行にそのまま出るので、行を壊す文字を通さない
    /// ことである(must/0022)。
    pub fn new(url: &str, reranker_id: &str) -> Result<Reranker, String> {
        let (address, path) = http::split_http_url(url)?;
        // パスを省いた指定(http://host:port)は、この API の既定のパスを補う。
        let path = if path == "/" { "/v1/rerank".to_string() } else { path };
        if reranker_id.is_empty()
            || !reranker_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(format!(
                "模型識別子 {reranker_id:?} は英数字と - _ . だけで書く(要求の model と\
                 診断の行にそのまま出る)"
            ));
        }
        Ok(Reranker {
            address,
            path,
            reranker_id: reranker_id.to_string(),
            timeout: DEFAULT_RERANK_TIMEOUT,
            depth: DEFAULT_RERANK_DEPTH,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Reranker {
        self.timeout = timeout;
        self
    }

    /// 再採点する候補の数を変える(既定は DEFAULT_RERANK_DEPTH)。
    pub fn with_depth(mut self, depth: usize) -> Reranker {
        self.depth = depth;
        self
    }

    pub fn reranker_id(&self) -> &str {
        &self.reranker_id
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    /// 接続先の表示(劣化の理由に出す。どのサーバに届かなかったのかを言うため)。
    pub fn endpoint(&self) -> String {
        format!("http://{}{}", self.address, self.path)
    }

    /// 問いと文書の組を採点し直す。返るのは(要求した documents の添字, 得点)の列で、
    /// 得点の降順・同点は添字の昇順に並ぶ(同点の並びを決めておく。should/0125)。
    ///
    /// 得点は logit である(意味論はこのファイルの冒頭の説明)。順位にしか使えない。
    ///
    /// 送る文書は呼び手が決める。既定の経路(rerank_items)は検索索引の抜粋を送るが、
    /// 全文を送りたい呼び手はここへ直接渡せる(そのとき llama-server 側に
    /// `--batch-size 2048` が要る)。
    pub fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<(usize, f64)>, String> {
        if documents.is_empty() {
            // 採点する組が無いので往復しない(空の応答を作って検査するより短い)。
            return Ok(Vec::new());
        }
        if query.is_empty() {
            return Err("問いが空のままリランカーには掛けられない".to_string());
        }
        // 要求本文は c1 の直列化で組む(文字列のエスケープの実装を増やさない。
        // should/0135)。c1 は JSON の部分集合なので、そのまま JSON として読める。
        let mut body = BTreeMap::new();
        body.insert(
            "documents".to_string(),
            c1::Value::Array(documents.iter().map(|d| c1::Value::Text(d.clone())).collect()),
        );
        body.insert("model".to_string(), c1::Value::Text(self.reranker_id.clone()));
        body.insert("query".to_string(), c1::Value::Text(query.to_string()));
        // top_n は送った件数そのものにする。上位だけを求めると「返らなかった件」と
        // 「得点が低かった件」が応答から区別できず、欠落の検査(must/0022)ができない。
        body.insert("top_n".to_string(), c1::Value::Integer(documents.len() as i64));
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
        order_from_response(text, documents.len()).map_err(|e| format!("{}: {e}", self.endpoint()))
    }
}

/// 応答から(添字, 得点)の列を取り出し、得点の降順に並べる。
///
/// 検査するのは四つ。results の件数が要求した件数と等しいこと、index が 0..件数 をちょうど
/// 一度ずつ覆うこと(配列の並びには依存しない)、得点が数であること、その得点が有限で
/// あること(NaN や無限大を混ぜたまま並べ替えると、順位が入力の並び次第で変わる)。
/// どれかが崩れていたら黙って通さず、何が違ったのかを言って失敗する(must/0022)。
/// 埋め込みクライアント(node/src/embed.rs の vectors_from_response)と同じ検査である:
/// 添字で結び直す応答は、添字が壊れていたら別の文書の得点を掴む。
fn order_from_response(text: &str, count: usize) -> Result<Vec<(usize, f64)>, String> {
    let value = Json::parse(text)?;
    let Json::Object(fields) = &value else {
        return Err("応答がオブジェクトでない".to_string());
    };
    let results = match fields.iter().find(|(name, _)| name == "results") {
        Some((_, Json::Array(items))) => items,
        Some(_) => return Err("応答の results が配列でない".to_string()),
        None => return Err("応答に results がない".to_string()),
    };
    if results.len() != count {
        return Err(format!("{count} 件を求めたのに results は {} 件", results.len()));
    }
    let mut slots: Vec<Option<f64>> = (0..count).map(|_| None).collect();
    for item in results {
        let Json::Object(entry) = item else {
            return Err("results の要素がオブジェクトでない".to_string());
        };
        let index = match entry.iter().find(|(name, _)| name == "index") {
            Some((_, Json::Number(n))) if *n >= 0.0 && n.fract() == 0.0 => *n as usize,
            Some(_) => return Err("results の index が非負整数でない".to_string()),
            None => return Err("results の要素に index がない".to_string()),
        };
        let score = match entry.iter().find(|(name, _)| name == "relevance_score") {
            Some((_, Json::Number(n))) => *n,
            Some(_) => return Err("results の relevance_score が数でない".to_string()),
            None => return Err("results の要素に relevance_score がない".to_string()),
        };
        if index >= count {
            return Err(format!("index {index} が要求した {count} 件の範囲外"));
        }
        if !score.is_finite() {
            return Err(format!("index {index} の得点が {score}(有限の値でない)"));
        }
        if slots[index].is_some() {
            return Err(format!("index {index} が二度現れた"));
        }
        slots[index] = Some(score);
    }
    let mut order = Vec::with_capacity(count);
    for (index, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(score) => order.push((index, score)),
            None => return Err(format!("index {index} の得点が応答に無い")),
        }
    }
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(order)
}

// ---- 順位の取り直し ----

/// 順位を取り直した結果。順位そのものは呼び手の列を並べ替えて返すので、ここに残るのは
/// 「効いたかどうか」と「効かなかった理由」である。
pub struct Reranked {
    /// 並べ替えたか。true のときだけ得点の意味論が RERANK_SCORE_SEMANTICS に変わる。
    /// false なら列は入力のまま(得点も元の意味論のまま)である。
    pub reranked: bool,
    /// 実際に再採点した件数(先頭から数えて)。深さより長い列を渡されたときは、後ろの
    /// 候補は元の順のまま残るので、どこまでが取り直した順位なのかを呼び手に言う。
    pub rescored: usize,
    /// 取り直せなかった理由(黙って劣化しない。should/0128)。呼び手はこれを応答の
    /// degraded に足す。リランカーを設定していない節点で毎回この理由を載せたくない
    /// 呼び手は、そもそも呼ばなければよい(Option を受ける口を残したのは、要求で明示的に
    /// リランカーを求められる呼び手のためである)。
    pub degraded: Option<String>,
}

/// 検索が挙げた候補の順位を取り直す。
///
/// items の先頭 depth 件(既定 30)を text_of の本文で採点し直し、得点の降順に並べ替えて、
/// score_of が指す得点をリランカーの得点で置き換える。深さより後ろの候補は元の順のまま
/// 後ろに残る(黙って捨てない)。
///
/// text_of には検索索引の抜粋(200 文字)を渡すのが既定である。全文でも上位の並びは
/// 変わらなかった(モジュール説明の実測)ので、全文を取りに行く往復は足さない。
///
/// リランカーが無い・届かない・応答が壊れている場合、items には触れずに理由だけを返す。
/// 呼び手はリランカー無しの順位でそのまま答え続けられる(検索を失敗させない)。
///
/// 呼び手の側でロックを持ったまま呼ばないこと。0.8 秒級の往復であり、その間ストアの
/// ロックを持つと 1 本の検索で API 全体が塞がる(node/src/embed.rs の QueryEmbedding が
/// ロックの外で作られるのと同じ規律)。候補の本文(抜粋)と引用を先に取り出してから
/// ロックを放し、この関数を呼ぶ。
pub fn rerank_items<T>(
    reranker: Option<&Reranker>,
    query: &str,
    items: &mut Vec<T>,
    text_of: impl Fn(&T) -> String,
    score_of: impl Fn(&mut T) -> &mut f64,
) -> Reranked {
    let Some(reranker) = reranker else {
        return Reranked {
            reranked: false,
            rescored: 0,
            degraded: Some("リランカーが設定されていない".to_string()),
        };
    };
    // 1 件以下では順位が動かない。往復の 0.7 秒と、動かない順位に別の意味論の得点を
    // 貼ることの両方を避ける(劣化ではないので理由も言わない)。
    if items.len() < 2 {
        return Reranked { reranked: false, rescored: 0, degraded: None };
    }
    let depth = items.len().min(reranker.depth().max(1));
    let documents: Vec<String> =
        items[..depth].iter().map(&text_of).collect();
    // 往復が失敗したときに items を壊さないよう、並べ替えは応答を検査し終えてから行う。
    let order = match reranker.rerank(query, &documents) {
        Ok(order) => order,
        Err(reason) => {
            return Reranked { reranked: false, rescored: 0, degraded: Some(reason) };
        }
    };
    // 取り直した順位で置き換えるのではなく、一次検索の順位と融合する。リランカーは
    // 「もう一つの意見」であって託宣ではない、という扱いである(この系が順位の統合に
    // ずっと使ってきた形と同じ。crate::embed::fuse_by_rank)。
    //
    // なぜ置き換えないか。同じ 34 問・同じ候補集合で両方を測った(実測 2026-08-18。
    // 20260818-search-quality (uuid:4b5e913a-5a50-4633-9ecb-b65afc24fc8a)):
    //
    // | | R@1 | R@3 | R@5 | R@10 | MRR |
    // | 融合 | 0.559 | 0.941 | 1.000 | 1.000 | 0.747 |
    // | 置き換え | 0.559 | 0.853 | 0.853 | 0.941 | 0.699 |
    //
    // 差の中身は「置き換えは取りこぼす」ことである。置き換えると 2 問が上位 10 件から
    // 完全に落ち、1 問が 1 位から 9 位へ下がった。置き換えが勝つ問いもある(2 問が 2〜3 位
    // から 1 位へ)が、片方の判断だけに委ねたときの失敗の幅が大きい。融合ではどの問いも
    // 上位 5 件から外れなかった。
    let rerank_order: Vec<usize> = order.iter().map(|(index, _)| *index).collect();
    let retrieval_order: Vec<usize> = (0..depth).collect();
    let fused = crate::embed::fuse_by_rank(&[retrieval_order, rerank_order], depth);
    // 得点は融合の得点で置き換える(順位付けにしか意味を持たない、という性質は
    // どちらの側とも同じである)。
    let mut slots: Vec<Option<T>> = items.drain(..depth).map(Some).collect();
    let mut reordered = Vec::with_capacity(depth);
    for entry in fused {
        let mut item = slots[entry.item].take().expect("添字は一度だけ現れる");
        *score_of(&mut item) = entry.score;
        reordered.push(item);
    }
    items.splice(0..0, reordered);
    Reranked { reranked: true, rescored: depth, degraded: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// リランカーが要るテストの前提確認。走っていない環境では黙って飛ばさず、起動の
    /// 手順を示して失敗する(node/tests/common/mod.rs の require_embedding_server と同じ
    /// 扱い。飛ばして緑にすると、検証したのか検証を諦めたのかが結果から区別できなく
    /// なる)。
    ///
    /// 生存の判定は実際に 1 組採点してみることで行う(設定ではなく観測された効果で
    /// 確かめる。should/0116)。プロセスが起きていることと、--reranking を付けて起きて
    /// いることは別であり、後者は採点が通るかどうかでしか分からない。
    fn require_rerank_server() -> Reranker {
        let reranker = Reranker::new(DEFAULT_RERANK_URL, DEFAULT_RERANKER_ID).expect("既定の設定");
        if let Err(error) = reranker.rerank("疎通確認", &["疎通確認".to_string()]) {
            panic!(
                "リランカーのテストには {} で待ち受けるリランカーが必要({error})。\
                 起動例: llama-server --model \
                 /work2/llm/models/bge-reranker-v2-m3/bge-reranker-v2-m3-FP16.gguf \
                 --host 127.0.0.1 --port 8084 --reranking --pooling rank -ngl 99 \
                 --ctx-size 8192 --batch-size 2048 --ubatch-size 2048",
                reranker.endpoint()
            );
        }
        reranker
    }

    /// 応答は得点の降順に並び、配列の並びには依存しない。期待値はリテラルで書く
    /// (should/0137)。
    #[test]
    fn a_response_is_ordered_by_score_and_does_not_depend_on_the_array_order() {
        // 実測の値の形(0..1 ではなく符号つきの logit)をそのまま使う。
        let text = "{\"results\":[{\"index\":0,\"relevance_score\":-1.36},\
                    {\"index\":2,\"relevance_score\":1.084},\
                    {\"index\":1,\"relevance_score\":-4.45}],\"model\":\"bge-reranker-v2-m3\"}";
        let order = order_from_response(text, 3).expect("3 件");
        assert_eq!(order[0], (2, 1.084));
        assert_eq!(order[1], (0, -1.36));
        assert_eq!(order[2], (1, -4.45));
        // 同点は添字の昇順で決まる(順位を決定的にする。should/0125)。
        let tied = "{\"results\":[{\"index\":1,\"relevance_score\":0.5},\
                     {\"index\":0,\"relevance_score\":0.5}]}";
        let order = order_from_response(tied, 2).expect("2 件");
        assert_eq!(order, vec![(0, 0.5), (1, 0.5)]);
        // 0 件を求めたなら空の results が正しい応答である。
        assert_eq!(order_from_response("{\"results\":[]}", 0).expect("0 件"), Vec::new());
    }

    /// 壊れた応答は通さず、何が違ったのかを言って失敗する(must/0022)。添字で結び直す
    /// 応答なので、添字が壊れていれば別の文書の得点を掴む。
    #[test]
    fn a_broken_response_is_refused_with_the_reason() {
        // index が要求の範囲外。
        let out_of_range = "{\"results\":[{\"index\":0,\"relevance_score\":1.0},\
                            {\"index\":7,\"relevance_score\":0.0}]}";
        let error = order_from_response(out_of_range, 2).expect_err("範囲外は失敗すべき");
        assert!(error.contains("index 7"), "{error}");
        assert!(error.contains("範囲外"), "{error}");
        // index の重複(片方の文書の得点が二度現れ、もう片方が欠ける)。
        let duplicated = "{\"results\":[{\"index\":0,\"relevance_score\":1.0},\
                          {\"index\":0,\"relevance_score\":0.0}]}";
        let error = order_from_response(duplicated, 2).expect_err("重複は失敗すべき");
        assert!(error.contains("二度現れた"), "{error}");
        // 件数が足りない(欠落)。
        let short = "{\"results\":[{\"index\":0,\"relevance_score\":1.0}]}";
        let error = order_from_response(short, 3).expect_err("件数違いは失敗すべき");
        assert!(error.contains("3 件を求めた"), "{error}");
        // 件数は合っているが index が飛んでいる(1 が欠けて 2 が二度…ではなく欠落だけ)。
        let missing = "{\"results\":[{\"index\":0,\"relevance_score\":1.0},\
                       {\"index\":2,\"relevance_score\":0.0}]}";
        let error = order_from_response(missing, 3).expect_err("欠落は失敗すべき");
        assert!(error.contains("3 件を求めた"), "{error}");
        // results そのものが無い応答(--reranking なしで起こしたサーバの誤り本文など)。
        let error = order_from_response("{\"error\":{\"message\":\"x\"}}", 1)
            .expect_err("results 無しは失敗すべき");
        assert!(error.contains("results がない"), "{error}");
        // 空の応答を「0 件の順位」として通さない(1 件求めたのに 0 件は欠落である)。
        let error = order_from_response("{\"results\":[]}", 1).expect_err("空は失敗すべき");
        assert!(error.contains("1 件を求めた"), "{error}");
        // 得点が数でない・有限でない。
        let text = "{\"results\":[{\"index\":0,\"relevance_score\":\"1.0\"}]}";
        let error = order_from_response(text, 1).expect_err("文字列の得点は失敗すべき");
        assert!(error.contains("数でない"), "{error}");
        let text = "{\"results\":[{\"index\":0,\"relevance_score\":1e400}]}";
        let error = order_from_response(text, 1).expect_err("無限大は失敗すべき");
        assert!(error.contains("有限"), "{error}");
    }

    /// URL の分解と模型識別子の検査(期待値はリテラル。should/0137)。
    #[test]
    fn the_endpoint_and_the_model_id_are_checked_when_the_client_is_built() {
        let reranker = Reranker::new(DEFAULT_RERANK_URL, DEFAULT_RERANKER_ID).expect("new");
        assert_eq!(reranker.endpoint(), DEFAULT_RERANK_URL);
        assert_eq!(reranker.reranker_id(), "bge-reranker-v2-m3");
        assert_eq!(reranker.depth(), DEFAULT_RERANK_DEPTH);
        // パスを省いた指定は既定のパスを補う。
        let reranker = Reranker::new("http://127.0.0.1:8084", DEFAULT_RERANKER_ID).expect("new");
        assert_eq!(reranker.endpoint(), "http://127.0.0.1:8084/v1/rerank");
        // https は平文にすり替えずに断る(http::split_http_url の規則)。
        assert!(Reranker::new("https://example.test/v1/rerank", DEFAULT_RERANKER_ID).is_err());
        // 綴りを間違えた識別子は、要求に載せる前に断る。
        assert!(Reranker::new(DEFAULT_RERANK_URL, "").is_err());
        assert!(Reranker::new(DEFAULT_RERANK_URL, "bge reranker").is_err());
        assert!(Reranker::new(DEFAULT_RERANK_URL, "models/bge-reranker-v2-m3").is_err());
    }

    /// 届かないリランカーは、どこへ届かなかったのかを言って失敗し、順位は元のまま残る
    /// (検索を失敗させない。should/0128)。
    #[test]
    fn an_unreachable_reranker_leaves_the_order_untouched_and_reports_why() {
        // ポート 1 は特権ポートで、この試験環境では誰も待ち受けていない。
        let reranker = Reranker::new("http://127.0.0.1:1/v1/rerank", DEFAULT_RERANKER_ID)
            .expect("new")
            .with_timeout(Duration::from_millis(500));
        let error = reranker.rerank("問い", &["a".to_string()]).expect_err("繋がらないはず");
        assert!(error.contains("http://127.0.0.1:1/v1/rerank"), "{error}");
        assert!(error.contains("接続できない"), "{error}");

        let mut items = vec![("a", 2.0f64), ("b", 1.0)];
        let outcome = rerank_items(
            Some(&reranker),
            "問い",
            &mut items,
            |item| item.0.to_string(),
            |item| &mut item.1,
        );
        assert!(!outcome.reranked, "取り直せていないと言うべき");
        assert_eq!(outcome.rescored, 0);
        assert_eq!(items, vec![("a", 2.0), ("b", 1.0)], "順位も得点も元のまま残るべき");
        let reason = outcome.degraded.expect("理由を持ち帰るべき");
        assert!(reason.contains("接続できない"), "{reason}");

        // リランカーが設定されていない呼び手も、同じ形で答え続けられる。
        let outcome =
            rerank_items(None, "問い", &mut items, |item| item.0.to_string(), |item| &mut item.1);
        assert!(!outcome.reranked);
        assert_eq!(items, vec![("a", 2.0), ("b", 1.0)]);
        assert!(outcome.degraded.expect("理由").contains("設定されていない"));

        // 1 件以下では順位が動かないので往復しない(届かない相手でも理由が出ない)。
        let mut single = vec![("a", 2.0f64)];
        let outcome = rerank_items(
            Some(&reranker),
            "問い",
            &mut single,
            |item| item.0.to_string(),
            |item| &mut item.1,
        );
        assert!(!outcome.reranked);
        assert!(outcome.degraded.is_none(), "1 件は劣化ではない");
    }

    /// 走っているリランカーとの疎通(should/0138: 生産の呼び手が組む要求そのものを送る)。
    /// 順位が問いに応じて動くこと、得点が 0..1 に収まらない値であること(閾値に使えない
    /// ことの実測)、深さより後ろの候補が元の順で残ることを見る。
    #[test]
    fn the_running_reranker_reorders_the_candidates() {
        let reranker = require_rerank_server();
        // 意味検索が取り違えやすい形(問いの語がどの候補にも現れない)を使う。正解を
        // 末尾に置き、リランカーの意見が順位に効くことを見る。
        //
        // 取り直しは置き換えではなく融合なので(rerank_items の理由)、末尾の 1 件が
        // 先頭に飛ぶとは限らない: 一次検索が最下位に置いたものは、リランカーが 1 位に
        // 置いても、一次検索の 1 位と釣り合う。ここで縛るのは「上がること」であって
        // 「先頭に来ること」ではない。
        let query = "xHCI のスクラッチパッドバッファは何に使うのか";
        let mut items = vec![
            ("イーサネットフレームの CRC は末尾 4 バイトである", 3.0f64),
            ("ELF のプログラムヘッダはセグメントの配置を表す", 2.0),
            (
                "The xHC may request pages from system software for its own internal use \
                 via the Scratchpad Buffer Array; the Max Scratchpad Buffers field \
                 reports how many it needs.",
                1.0,
            ),
        ];
        let outcome = rerank_items(
            Some(&reranker),
            query,
            &mut items,
            |item| item.0.to_string(),
            |item| &mut item.1,
        );
        assert!(outcome.reranked, "{:?}", outcome.degraded);
        assert_eq!(outcome.rescored, 3);
        assert!(outcome.degraded.is_none());
        let answer = items
            .iter()
            .position(|item| item.0.contains("Scratchpad"))
            .expect("正解は消えない");
        assert!(answer <= 1, "末尾(2)から上がっているべき: {items:?}");
        // 得点は融合の得点(順位から作った値)で、降順に並ぶ。閾値に使えないことは
        // 変わらないが、値の出どころはリランカーの logit ではなくなる。
        assert!(items[0].1 >= items[1].1 && items[1].1 >= items[2].1, "{items:?}");

        // 深さを 2 に絞ると、3 件目は元の順のまま後ろに残る(黙って捨てない)。
        let shallow = require_rerank_server().with_depth(2);
        let mut items = vec![("犬", 1.0f64), ("猫", 2.0), ("末尾に残る候補", 3.0)];
        let outcome = rerank_items(
            Some(&shallow),
            "猫について",
            &mut items,
            |item| item.0.to_string(),
            |item| &mut item.1,
        );
        assert!(outcome.reranked);
        assert_eq!(outcome.rescored, 2);
        assert_eq!(items[2], ("末尾に残る候補", 3.0), "深さの外は得点も順位も変えない");
    }
}

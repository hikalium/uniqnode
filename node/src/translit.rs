//! カタカナのクエリを、索引に実在する英語の術語へ寄せる層(逆音訳)。
//!
//! コーパスは英語の技術仕様書である。文字 bigram の索引(node/src/search.rs)に日本語の
//! bigram は一つも載らないので、「スクラッチパッド」と問うと BM25 の当たりは 0 件になる。
//! 同じ主題を "scratchpad" と英語で問えば 1 位で当たるのだから、正解はコーパスにあり、
//! 足りないのは到達手段だけである。この層はその手段だけを引き受ける。
//!
//! 道筋は カタカナ → ローマ字 → 子音の骨格 → 索引に実在する語 の 4 段である。骨格まで
//! 落とすのは、外来語の母音が英語の綴りから復元できないからである(buffer の /ʌ/ は
//! バ、descriptor の /ɪ/ は ディ で、対応表が書けない)。子音は逆に写しやすいので、
//! 母音を捨てた骨格どうしを突き合わせれば「バッファ = buffer」が距離 0 で結べる。
//!
//! この層は綴りを思いつくだけで、思いついた綴りが索引に無ければ捨てる。架空の語を
//! クエリに足すと BM25 の語被覆(SearchIndex::ranked_lexical)を薄めて順位を壊すので、
//! 「当たらないより、外す方が悪い」に倒してある。倒し方は 6 つで、どの閾値も実測で
//! 決めた(理由はそれぞれの定数に書いた):
//!
//! - 返すのは Vocabulary が実在すると言った語だけ(思いついた綴りは返さない)。
//! - 骨格が短い(子音 2 個未満)候補は捨てる。情報が足りず、何にでも当たるためである。
//! - 骨格の編集距離は 0 を既定とし、骨格が 5 子音以上あるときだけ 1 まで許す。
//! - 骨格が合っても、綴りが離れすぎ・語長が倍以上違う組は捨てる
//!   (MAX_SPELLING_GAP、MAX_LENGTH_RATIO)。
//! - 分割した片は、まるごとの当たりが無いときにだけ返す(Split)。
//! - 返す語数に上限を置く(MAX_TERMS)。クエリが展開語で薄まらないようにするため。
//!
//! 効き目も実測した。この索引の語彙を渡して 50 語のカタカナ術語を問うと、日本語だけでは
//! 44 語が当たり 0 件だったのが、展開後は 50 語すべてで当たりが出て、そのうち 42 語は
//! 「英語で問うたときと同じチャンク」が 1 位に来た。
//!
//! 扱うのはカタカナだけである。漢字の訳語(「割り込み」→ interrupt)は音ではなく意味の
//! 対応で、逆音訳では届かない。届かないものを届くふりをしないため、カタカナ以外は
//! 何も返さない(訳語辞書が要るなら、それは別の層の仕事である)。
//!
//! 語彙は引数で受け取る(索引そのものには依らない)。SearchIndex を直に見ないのは、
//! この層を純関数のまま単体で試せるようにするためである。配線する側が索引の索引語を
//! Vocabulary として渡す。
//!
//! 掛かりは語彙の大きさに比例する(1 クエリにつき語彙を 1 度走査する)。実測では、
//! 仕様書 27 本の索引(索引語 77376、うち英字だけの語 37912)で 1 クエリ 9ms である。
//! 走査を避けたければ、呼び手が骨格の表を持って Vocabulary を作り直せばよいが、この層は
//! 索引の作りを知らないので、そこまではしない。

/// 展開して足す語の総数の上限。クエリの語が増えるほど BM25 の語被覆は薄まるので、
/// 当たりを増やす効き目と釣り合う範囲で切る。
const MAX_TERMS: usize = 6;

/// カタカナの連なり 1 つから足す語数の上限。「まるごと」と「分割した 2 片」で 3 語。
const MAX_TERMS_PER_RUN: usize = 3;

/// 候補として認める骨格の最短の長さ(子音の数)。1 子音の骨格はコーパスの何十語にも
/// 当たってしまい、どれを採っても当てずっぽうになる。
const MIN_SKELETON_LEN: usize = 2;

/// 骨格の編集距離 1 を許し始める長さ。短い骨格で 1 を許すと別語に化ける。
const FUZZY_SKELETON_LEN: usize = 5;

/// 語彙の側で相手にする語の長さの範囲。1〜2 文字の語は骨格が短すぎ、長すぎる語は
/// 識別子か連結語で、外来語 1 語の相手ではない。
const MIN_WORD_CHARS: usize = 3;
const MAX_WORD_CHARS: usize = 24;

/// 相手にするカタカナの連なりの長さ(文字数)。1 文字の連なりは骨格が立たない。上限を
/// 超える連なりは文の丸ごとの音写であり、1 語の術語ではない。
const MIN_RUN_CHARS: usize = 2;
const MAX_RUN_CHARS: usize = 24;

/// カタカナの連なりを分割するとき、片が持つべき最短のモーラ数。1 モーラの片は骨格が
/// 立たない。
const MIN_SPLIT_MORAS: usize = 2;

/// まるごとの当たりが無いときの分割で、片に求める骨格の長さと、その合計。裏付けが
/// 無いぶん厳しくする(裏付けの意味は split_of を見よ)。合計で見るのは、狙いの
/// スクラッチ(4 子音)+ パッド(2 子音)を残しつつ、2 子音どうしの当てずっぽうな
/// 割り方を落とすためである。
const LONE_SPLIT_SKELETON_LEN: usize = 2;
const LONE_SPLIT_SKELETON_SUM: usize = 6;

/// 分割した片に求める出現の多さ(まるごとの語の出現の何分の 1 まで許すか)。実測で決めた。
///
/// 綴りが割れている語(Scratchpad / Scratch Pad)なら、片も単独でよく使われる語である
/// (scratchpad が 109 チャンク、scratch が 93、pad が 459)。一方、PDF から採ったコーパスは
/// 行の折り返しで語が割れた断片(endpo、transa、configura)を語彙に持っていて、これらは
/// 綴りとしては まるごとの語をぴったり覆ってしまう。出現の多さで見ると桁が違うので、
/// ここで落とせる。
const SPLIT_WEIGHT_RATIO: u64 = 4;

/// ローマ字の異読を組み合わせて作る候補の数の上限。増やすほど当たりは増えるが、
/// 骨格が増えるぶん別語にも当たりやすくなる。
const MAX_ROMAJI_VARIANTS: usize = 8;

/// 綴りの隔たりを整数で持つときの倍率(spelling_gap)。
const SPELLING_SCALE: usize = 1000;

/// 当たりとして認める綴りの隔たりの上限(SPELLING_SCALE 分)。実測で決めた。当たって
/// ほしい組でも隔たりは大きい(ページ/page が 0.80、キャッシュ/cache が 0.67)ので、
/// ここは「音写ではありえない」を切るだけの緩い関にしてある。締めると正解の方が先に
/// 落ちる(0.64 まで締めると テーブル/table(0.71)も レジスタ/register(0.75)も消えた)。
const MAX_SPELLING_GAP: usize = 850;

/// ローマ字と英語の語長の開きの上限(何倍まで許すか)。外来語のローマ字は母音が増えるので
/// 元の英語より長くなるが、倍を越えるほどではない。
const MAX_LENGTH_RATIO: usize = 2;

/// 候補 1 つにつき覚えておく当たりの数(Piece::hits)。
const MAX_HITS_PER_PIECE: usize = 8;

/// 索引に実在する語の集合。この層は「思いついた綴り」を必ずこれと突き合わせてから返す。
///
/// 走査の形にしてあるのは、照合が編集距離だからである。実在を問う述語(contains)だけ
/// では、綴りが 1 字ずれた語に寄せられない。
pub trait Vocabulary {
    /// 語彙の各語を 1 度ずつ visit に渡す。第 2 引数はその語の出現の多さ(索引なら語を
    /// 含むチャンク数)で、骨格が同じ語が複数あるときにどれを採るかの目安に使う。
    /// 分からなければ 1 を渡してよい(そのときは綴りの近さと辞書順だけで決まる)。
    fn visit_words(&self, visit: &mut dyn FnMut(&str, u64));
}

impl Vocabulary for [&str] {
    fn visit_words(&self, visit: &mut dyn FnMut(&str, u64)) {
        for word in self {
            visit(word, 1);
        }
    }
}

impl Vocabulary for [String] {
    fn visit_words(&self, visit: &mut dyn FnMut(&str, u64)) {
        for word in self {
            visit(word, 1);
        }
    }
}

impl Vocabulary for [(&str, u64)] {
    fn visit_words(&self, visit: &mut dyn FnMut(&str, u64)) {
        for (word, weight) in self {
            visit(word, *weight);
        }
    }
}

impl Vocabulary for Vec<String> {
    fn visit_words(&self, visit: &mut dyn FnMut(&str, u64)) {
        self[..].visit_words(visit)
    }
}

/// 走査の手続きをそのまま語彙にする被せ物。索引のような、語の列を貸し出せるが
/// スライスにはできない持ち主のためにある。
///
/// ```ignore
/// let vocabulary = translit::VocabularyFn(|visit: &mut dyn FnMut(&str, u64)| {
///     index.visit_terms(&mut |term, chunks| visit(term, chunks as u64));
/// });
/// let added = translit::expand_terms("スクラッチパッド", &vocabulary);
/// ```
pub struct VocabularyFn<F>(pub F);

impl<F: Fn(&mut dyn FnMut(&str, u64))> Vocabulary for VocabularyFn<F> {
    fn visit_words(&self, visit: &mut dyn FnMut(&str, u64)) {
        (self.0)(visit)
    }
}

/// カタカナ語を分割した片(スクラッチパッド → scratch + pad)をいつ返すか。
///
/// 既定を Fallback にしたのは実測による。仕様書 27 本のコーパスで、まるごとの当たりが
/// あるのに片も足すと、狙いのチャンクの順位が落ちた:
///
/// | クエリ                                       | §4.20 Scratchpad Buffers(p.334)の順位 |
/// |----------------------------------------------|----------------------------------------|
/// | スクラッチパッド                             | 当たり 0 件                            |
/// | スクラッチパッド scratchpad                  | 1 位                                   |
/// | スクラッチパッド scratchpad scratch pad      | 9 位                                   |
///
/// 片は正しい英語(どちらもコーパスに実在し、scratch は同じ節に出る)なのに順位を壊す。
/// 片は元の語より桁で多く出るので idf が低く、BM25 では「pad を含むだけの無関係な
/// チャンク」を大量に持ち上げるからである。まるごとが当たっているなら、それが最も
/// 情報量の多い語であり、足すものは無い。
///
/// Always が要るのは、コーパスが 2 語綴りしか持たない場合(Scratch Pad とだけ書く文書)
/// である。ただしそのときは、まるごとの当たりが無いので Fallback でも片が返る。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Split {
    /// まるごとが当たらなかったときだけ、分割した片を返す(既定)。
    #[default]
    Fallback,
    /// まるごとが当たっていても、綴りの割れ(Scratchpad / Scratch Pad)として片も返す。
    Always,
}

/// クエリに足すべき英語の語を返す(元のクエリの語は含まない)。返る語は必ず vocabulary が
/// 実在すると言ったものだけで、当てが無ければ空になる。
///
/// 並びは「連なりの現れた順 → まるごとの当たり → 分割した片」で、どこから来た語かが
/// 順に読める。同じ語は 1 度しか返さない。
pub fn expand_terms<V: Vocabulary + ?Sized>(query: &str, vocabulary: &V) -> Vec<String> {
    expand_terms_with(query, vocabulary, Split::default())
}

/// expand_terms の、分割の扱いを選べる形。
pub fn expand_terms_with<V: Vocabulary + ?Sized>(
    query: &str,
    vocabulary: &V,
    split: Split,
) -> Vec<String> {
    let runs = katakana_runs(query);
    if runs.is_empty() {
        return Vec::new();
    }
    // 連なりごとに「まるごと」と「2 分割」の候補を並べ、骨格を作る。
    let mut pieces: Vec<Piece> = Vec::new();
    for (run_index, run) in runs.iter().enumerate() {
        let Some(moras) = moras_of(run) else { continue };
        push_pieces(&mut pieces, run_index, &moras);
    }
    if pieces.is_empty() {
        return Vec::new();
    }
    // 語彙は 1 度だけ走査する。候補の側は数十個しかないので、語ごとに候補の骨格と
    // 突き合わせる方が、候補ごとに語彙を舐め直すより桁で安い。作業場を輪の外で確保
    // するのは、索引の語彙が 4 万語あり、1 語ごとの確保が効いてくるためである。
    let mut spelling: Vec<u8> = Vec::new();
    let mut skeleton = String::new();
    vocabulary.visit_words(&mut |word, weight| {
        // 相手にするのは英字だけの語である。索引には日本語の文字 bigram も識別子も
        // 載っているが、逆音訳の行き先になるのは英単語だけである。
        if !word.bytes().all(|b| b.is_ascii_alphabetic()) {
            return;
        }
        if !(MIN_WORD_CHARS..=MAX_WORD_CHARS).contains(&word.len()) {
            return;
        }
        skeleton_into(word, &mut spelling, &mut skeleton);
        if skeleton.len() < MIN_SKELETON_LEN {
            return;
        }
        for piece in pieces.iter_mut() {
            piece.offer(word, &skeleton, weight);
        }
    });
    collect(query, &pieces, split)
}

/// 元のクエリの後ろに展開語を足した 1 本のクエリを返す(足すものが無ければ元のまま)。
/// 元の文字列には触らない: 「xHCI のスクラッチパッド」の xHCI は既に英語であり、
/// そのままの方が強い。
pub fn expand_query<V: Vocabulary + ?Sized>(query: &str, vocabulary: &V) -> String {
    let added = expand_terms(query, vocabulary);
    if added.is_empty() {
        return query.to_string();
    }
    format!("{query} {}", added.join(" "))
}

/// 候補 1 つ(カタカナの連なりまるごと、または分割した片)。
struct Piece {
    /// 何番目のカタカナの連なりから来たか(返す順に使う)。
    run: usize,
    /// 分割の識別。0 = 分割なし(まるごと)。1 以上は分割の境の位置。
    split: usize,
    /// 連なりの中での並び(分割した片を左から右へ返すため)。
    order: usize,
    /// ローマ字の写し(長音符の読み替えで複数になる)。綴りの近さの目安に使う。
    romaji: Vec<String>,
    /// 骨格(長音符の読み替えごと)。
    skeletons: Vec<String>,
    /// skeletons の長さの下限と上限(語彙の走査で、長さだけで振り落とすため)。
    shortest_skeleton: usize,
    longest_skeleton: usize,
    /// いま見つかっている当たり(良い順。先頭が最良)。1 つに絞らず数えるだけ持つのは、
    /// 分割の側が「まるごとの語をぴったり覆う片」を選び直せるようにするためである
    /// (split_of)。パッド の骨格 pd には識別子 pid が pad より多く出るので、単独では
    /// pid が勝つが、scratchpad を覆えるのは pad だけである。
    hits: Vec<Hit>,
}

/// 語彙の語 1 つとの当たり。
struct Hit {
    word: String,
    /// 骨格どうしの編集距離。0 が完全一致。
    skeleton_distance: usize,
    /// ローマ字との綴りの隔たり(SPELLING_SCALE 分の値。小さいほど近い)。
    spelling: usize,
    /// 出現の多さ(多い方を採る)。
    weight: u64,
}

impl Hit {
    /// 良い当たりほど小さくなる順序の鍵。骨格の近さが第一、出現の多さが第二、綴りの
    /// 隔たりが第三。最後に辞書順で決めるのは、同じ入力に同じ答えを返すためである
    /// (should/0125 の決定性)。
    ///
    /// 出現の多さを綴りの近さより先に見るのは実測で決めた(70 語の逆音訳で、綴りの近さを
    /// 先にすると 32 語、出現の多さを先にすると 53 語が当たった)。骨格が合った語の中で
    /// 綴りが最も近いのは、たいてい正解ではなく「ローマ字をそのまま綴ったような屑語」
    /// である。コーパスには OCR の欠けや語の断片が混じっていて、バッファ には buffer より
    /// buff が、サーバ には server より saba が近い。どちらが問われているかの当てとしては
    /// 「仕様書で何度も使われている語の方」に賭ける方が桁で当たる。
    ///
    /// 綴りの隔たりは、それでも並んだときの決め手として要る(バッファ の骨格 bf には
    /// buffer / before / beef が等しく当たる)。
    fn key(&self) -> (usize, u64, usize, &str) {
        (self.skeleton_distance, u64::MAX - self.weight, self.spelling, self.word.as_str())
    }
}

impl Piece {
    /// 語彙の語 1 つを候補に当ててみて、いまより良ければ差し替える。
    fn offer(&mut self, word: &str, skeleton: &str, weight: u64) {
        // 骨格が長いときだけ距離 1 を許す。短い骨格で 1 を許すと別語に化ける。
        let limit = usize::from(self.longest_skeleton.max(skeleton.len()) >= FUZZY_SKELETON_LEN);
        // 長さだけで振り落とせる語はここで切る。語彙 4 万語のほとんどはここで落ちるので、
        // 骨格を 1 つずつ突き合わせる輪に入る前に見る。
        if skeleton.len() + limit < self.shortest_skeleton
            || skeleton.len() > self.longest_skeleton + limit
        {
            return;
        }
        let Some(skeleton_distance) =
            self.skeletons.iter().filter_map(|s| bounded_distance(s, skeleton, limit)).min()
        else {
            return;
        };
        // 外来語のローマ字は母音が増えるぶん元の英語より長いが、倍にはならない。長さが
        // 倍を越えて開いている組は音写の関係ではない(スケジューラ = sukejuura の骨格には
        // コーパスの識別子 scgo が合ってしまうが、ここで落ちる)。
        let shortest = self.romaji.iter().map(|r| r.chars().count()).min().unwrap_or(0);
        let longest = self.romaji.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let count = word.chars().count();
        if count * MAX_LENGTH_RATIO < shortest || longest * MAX_LENGTH_RATIO < count {
            return;
        }
        // 綴りを比べるのはここから先だけなので、小文字化もここでする(語彙 4 万語の
        // ほとんどは骨格の段で落ちる。そこまで来る語だけを写す)。
        let word = word.to_ascii_lowercase();
        let spelling =
            self.romaji.iter().map(|r| spelling_gap(r, &word)).min().unwrap_or(usize::MAX);
        // 綴りがあまりに違う語は、骨格が合っていても当たりとしない。骨格は母音を捨てた
        // 粗い見方なので、これが最後の関である。
        if spelling > MAX_SPELLING_GAP {
            return;
        }
        let candidate = Hit { word, skeleton_distance, spelling, weight };
        if self.hits.len() == MAX_HITS_PER_PIECE
            && self.hits.last().is_some_and(|last| last.key() <= candidate.key())
        {
            return;
        }
        let at = self.hits.partition_point(|hit| hit.key() < candidate.key());
        self.hits.insert(at, candidate);
        self.hits.truncate(MAX_HITS_PER_PIECE);
    }

    /// 最良の当たり。
    fn best(&self) -> Option<&Hit> {
        self.hits.first()
    }
}

/// 綴りの隔たり: 編集距離を長い方の語長で割ったもの(SPELLING_SCALE 倍の整数)。整数で
/// 持つのは、順序が丸め方に左右されないようにするためである(should/0125 の決定性)。
fn spelling_gap(romaji: &str, word: &str) -> usize {
    let longest = romaji.chars().count().max(word.chars().count()).max(1);
    levenshtein(romaji, word) * SPELLING_SCALE / longest
}

/// 連なり 1 つぶんの候補(まるごと + 2 分割)を積む。
fn push_pieces(pieces: &mut Vec<Piece>, run: usize, moras: &[Mora]) {
    if let Some(piece) = piece_of(run, 0, 0, moras) {
        pieces.push(piece);
    }
    // 仕様書の中で外来語の元の綴りは割れている(xhci_1_2 では Scratchpad が 1 語で 22 回、
    // 別ページには Scratch Pad が 2 語で出る)。どちらにも当たるよう、分割した片も候補に
    // する。分割は境を 1 つだけ入れる形に限る(2 つ入れると片が短くなりすぎ、骨格が
    // 何にでも当たる)。
    for boundary in MIN_SPLIT_MORAS..moras.len().saturating_sub(MIN_SPLIT_MORAS - 1) {
        if moras.len() - boundary < MIN_SPLIT_MORAS {
            continue;
        }
        // 長音符は前のモーラに属するので、その手前では切らない。
        if matches!(moras[boundary], Mora::Long) {
            continue;
        }
        let (left, right) = moras.split_at(boundary);
        let (Some(a), Some(b)) =
            (piece_of(run, boundary, 0, left), piece_of(run, boundary, 1, right))
        else {
            continue;
        };
        pieces.push(a);
        pieces.push(b);
    }
}

/// モーラの列から候補を 1 つ作る(骨格が短すぎれば作らない)。
fn piece_of(run: usize, split: usize, order: usize, moras: &[Mora]) -> Option<Piece> {
    let romaji = romaji_variants(moras);
    let mut skeletons: Vec<String> = Vec::new();
    for form in &romaji {
        let skeleton = consonant_skeleton(form);
        if skeleton.len() >= MIN_SKELETON_LEN && !skeletons.contains(&skeleton) {
            skeletons.push(skeleton);
        }
    }
    let shortest_skeleton = skeletons.iter().map(|s| s.len()).min()?;
    let longest_skeleton = skeletons.iter().map(|s| s.len()).max()?;
    Some(Piece {
        run,
        split,
        order,
        romaji,
        skeletons,
        shortest_skeleton,
        longest_skeleton,
        hits: Vec::new(),
    })
}

/// 当たった候補を、上限と重複を見ながら返す語の列にまとめる。
fn collect(query: &str, pieces: &[Piece], split: Split) -> Vec<String> {
    // 既にクエリに英語で書かれている語は足さない(「xHCI の割り込み」の xHCI など)。
    let present: Vec<String> = query
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let runs = pieces.iter().map(|p| p.run).max().map_or(0, |m| m + 1);
    let mut terms: Vec<String> = Vec::new();
    for run in 0..runs {
        let whole =
            pieces.iter().find(|p| p.run == run && p.split == 0).and_then(|p| p.best());
        let mut chosen: Vec<&str> = Vec::new();
        if let Some(hit) = whole {
            chosen.push(&hit.word);
        }
        if whole.is_none() || split == Split::Always {
            if let Some((left, right)) = split_of(pieces, run, whole) {
                chosen.push(left);
                chosen.push(right);
            }
        }
        let mut added = 0usize;
        for word in chosen {
            if added >= MAX_TERMS_PER_RUN || terms.len() >= MAX_TERMS {
                break;
            }
            let word = word.to_string();
            if present.contains(&word) || terms.contains(&word) {
                continue;
            }
            terms.push(word);
            added += 1;
        }
    }
    terms
}

/// 連なり run の分割のうち、返すに足るものを選ぶ。
///
/// 分割は当てずっぽうになりやすい(レジスタ を レジ+スタ と切ると reg と that が当たる)。
/// 実測では、分割から出る語のほとんどが本題と関わりのない語だった。そこで裏付けを要求する:
///
/// - まるごとが当たっているなら、その語を左右の片が「ぴったり覆い」「どちらも単独で
///   よく使われている」分割だけを認める。scratchpad = scratch + pad は通り、
///   command = com + and(1 字余る)も endpoint = endpo + int(endpo は断片で、出現が
///   桁で少ない)も通らない。仕様書の表記揺れ(Scratchpad / Scratch Pad)はこの形で
///   現れるので、狙いは外さない。
/// - まるごとが当たっていないなら、両の片が骨格の完全一致で、かつ骨格が
///   LONE_SPLIT_SKELETON_LEN 子音以上あるものだけを認める。
///
/// どちらの条件も満たす分割が複数あれば、綴りの隔たりの小さい方を採る。
fn split_of<'a>(
    pieces: &'a [Piece],
    run: usize,
    whole: Option<&Hit>,
) -> Option<(&'a str, &'a str)> {
    let mut best: Option<(usize, &str, &str)> = None;
    for piece in pieces.iter().filter(|p| p.run == run && p.split != 0 && p.order == 0) {
        let Some(partner) =
            pieces.iter().find(|p| p.run == run && p.split == piece.split && p.order == 1)
        else {
            continue;
        };
        // 骨格の完全一致だけを相手にする。分割は元から当てずっぽうなので、1 字ずれた
        // 当たりまで拾うと外れが増える。
        let lefts = piece.hits.iter().filter(|hit| hit.skeleton_distance == 0);
        for left in lefts {
            for right in partner.hits.iter().filter(|hit| hit.skeleton_distance == 0) {
                let acceptable = match whole {
                    Some(whole) => {
                        whole.word.starts_with(&left.word)
                            && whole.word.ends_with(&right.word)
                            && left.word.len() + right.word.len() == whole.word.len()
                            && left.weight * SPLIT_WEIGHT_RATIO >= whole.weight
                            && right.weight * SPLIT_WEIGHT_RATIO >= whole.weight
                    }
                    None => {
                        piece.shortest_skeleton >= LONE_SPLIT_SKELETON_LEN
                            && partner.shortest_skeleton >= LONE_SPLIT_SKELETON_LEN
                            && piece.shortest_skeleton + partner.shortest_skeleton
                                >= LONE_SPLIT_SKELETON_SUM
                    }
                };
                if !acceptable {
                    continue;
                }
                let cost = left.spelling + right.spelling;
                if best.is_none_or(|(best_cost, _, _)| cost < best_cost) {
                    best = Some((cost, &left.word, &right.word));
                }
            }
        }
    }
    best.map(|(_, left, right)| (left, right))
}


/// カタカナのモーラ 1 つ。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mora {
    /// ローマ字に写した音と、英語の綴りから来る異読(ALTERNATE_READINGS)。
    Sound(&'static str, Option<&'static str>),
    /// 長音符(ー)。前の母音を伸ばすが、英語では r のこともある(サーバ = server、
    /// インターフェース = interface)。どちらかは決められないので両方を候補にする。
    Long,
}

/// 英語から来た音の、日本語では 1 つに潰れている書き分け。日本語にはこの対立が無いので、
/// カタカナだけを見てどちらだったかは決められない。両方をローマ字の候補にする。
///
/// - シ は sh とは限らない(serial → シリアル、shift → シフト)。
/// - チ は ch とは限らない(multi → マルチ、chip → チップ)。
///
/// ジ(radio → ラジオ)とツ(tool → ツール)も同じ形で割れるが、足しても実測の当たりは
/// 増えず(70 語で 53 のまま)、ページ が pad に当たるような外れだけが増えたので入れない。
const ALTERNATE_READINGS: &[(&str, &str)] = &[("shi", "si"), ("chi", "ti")];

fn alternate_of(reading: &str) -> Option<&'static str> {
    ALTERNATE_READINGS.iter().find(|(base, _)| *base == reading).map(|(_, alt)| *alt)
}

/// 文字列からカタカナの連なりを取り出す。カタカナ以外(漢字・ひらがな・英数字・記号)は
/// 区切りであり、連なりには入らない。
///
/// ひらがなと漢字を相手にしないのは、この層が音の対応しか持たないからである。「割り込み」
/// を interrupt に結ぶのは意味の対応で、ここでは扱えない。
pub fn katakana_runs(text: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut run = String::new();
    for character in text.chars() {
        if is_katakana(character) {
            run.push(character);
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs.retain(|run| {
        let count = run.chars().count();
        (MIN_RUN_CHARS..=MAX_RUN_CHARS).contains(&count)
    });
    runs
}

fn is_katakana(character: char) -> bool {
    // U+30A1..U+30FA が片仮名、U+30FC が長音符。中黒(U+30FB)は区切りなので入れない
    // (「スクラッチ・パッド」は 2 つの連なりとして扱いたい)。
    matches!(character, '\u{30A1}'..='\u{30FA}' | '\u{30FC}')
}

/// カタカナの連なりをモーラの列に切る。カタカナ以外が混ざっていれば None。
///
/// 促音(ッ)は落とす。日本語の促音は英語の重ね字と対応しない(バッファ の ッ は buffer の
/// ff ではなく母音の短さである)し、骨格では重ね字を潰すので、残しても意味が無い。
fn moras_of(run: &str) -> Option<Vec<Mora>> {
    let chars: Vec<char> = run.chars().collect();
    let mut moras: Vec<Mora> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == 'ッ' {
            i += 1;
            continue;
        }
        if chars[i] == 'ー' {
            // 先頭の長音符は音を伸ばす相手が無い。捨てる。
            if !moras.is_empty() {
                moras.push(Mora::Long);
            }
            i += 1;
            continue;
        }
        if i + 1 < chars.len() {
            let pair: String = chars[i..i + 2].iter().collect();
            if let Some(sound) = lookup(&pair, DIGRAPH_MORAS) {
                moras.push(Mora::Sound(sound, alternate_of(sound)));
                i += 2;
                continue;
            }
        }
        let single: String = chars[i..i + 1].iter().collect();
        let sound = lookup(&single, SINGLE_MORAS)?;
        moras.push(Mora::Sound(sound, alternate_of(sound)));
        i += 1;
    }
    if moras.is_empty() {
        return None;
    }
    Some(moras)
}

fn lookup(key: &str, table: &[(&str, &'static str)]) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// モーラの列をローマ字に写す。読みが割れるモーラ(長音符と ALTERNATE_READINGS)ごとに
/// 候補が分かれるので、返るのは 1 本ではなく組み合わせの列である。先頭は「どのモーラも
/// 既定の読み」の 1 本で、上限(MAX_ROMAJI_VARIANTS)で切るときは後ろの異読から落ちる。
fn romaji_variants(moras: &[Mora]) -> Vec<String> {
    let mut forms: Vec<String> = vec![String::new()];
    for mora in moras {
        let mut next: Vec<String> = Vec::new();
        for form in &forms {
            match mora {
                Mora::Sound(sound, alternate) => {
                    push_unique(&mut next, format!("{form}{sound}"));
                    if let Some(alternate) = alternate {
                        push_unique(&mut next, format!("{form}{alternate}"));
                    }
                }
                Mora::Long => {
                    // 伸ばす読み。伸ばす相手の母音が無ければ何も足さない。
                    let mut lengthened = form.clone();
                    if let Some(vowel) = form.chars().last().filter(|c| is_vowel(*c)) {
                        lengthened.push(vowel);
                    }
                    push_unique(&mut next, lengthened);
                    push_unique(&mut next, format!("{form}r"));
                }
            }
        }
        next.truncate(MAX_ROMAJI_VARIANTS);
        forms = next;
    }
    forms.retain(|form| !form.is_empty());
    forms
}

fn push_unique(forms: &mut Vec<String>, form: String) {
    if !forms.contains(&form) {
        forms.push(form);
    }
}

fn is_vowel(character: char) -> bool {
    matches!(character, 'a' | 'e' | 'i' | 'o' | 'u')
}

/// 語を子音の骨格に落とす(この層の要。ローマ字と英語の綴りの両方を同じ規則で通す)。
///
/// 外来語の母音は英語の綴りから復元できない(buffer→バッファ、descriptor→ディスクリプタ、
/// throughput→スループット。どれも母音が変わっている)一方、子音はよく残る。そこで母音を
/// 全部捨て、綴りが違っても同じ音になる子音を 1 つに寄せてから比べる。
///
/// 寄せは英語側にもローマ字側にも同じように掛かるので、多対一に潰しても取りこぼしは
/// 増えない(増えるのは別語への当たりだけで、それは骨格の長さと語彙の実在で抑える)。
/// 主な寄せ:
///
/// - `tion`/`sion`/`tch`/`ch`/`sh` → S。cache の ch と キャッシュ の シュ を同じにする。
/// - `th` → s。θ を日本語はサ行で写す(throughput → スループット)。
/// - `gh` → 無音。through / night の gh は綴りだけのもの。
/// - `c` は後ろが e/i/y なら s、それ以外は k。scratchpad の c と スクラッチ の ク が結ぶ。
/// - `g` → j。日本語は硬い g と柔らかい g を書き分けない(page → ページ、register → レジスタ)。
/// - `l` → r、`v` → b、`z` → s。日本語に対立が無い組。
/// - 重ね字は潰す。英語の重ね字は綴りの都合で、音ではない。
/// - 語末の r は落とす。日本語は語末の r を書かないことが多い(buffer → バッファ)。
///
/// ASCII 以外の文字は無視する(英語の綴りに対応する子音を持たない)。
pub fn consonant_skeleton(word: &str) -> String {
    let mut spelling = Vec::new();
    let mut skeleton = String::new();
    skeleton_into(word, &mut spelling, &mut skeleton);
    skeleton
}

/// consonant_skeleton の中身。作業場(spelling)と行き先(skeleton)を呼び手から借りる。
///
/// 借りる形にしてあるのは、これが語彙 1 語ごとに呼ばれるからである。索引の語彙 4 万語で
/// 1 語につき 2 つ確保すると、それだけで 1 クエリが十数ミリ秒に伸びる。
fn skeleton_into(word: &str, spelling: &mut Vec<u8>, skeleton: &mut String) {
    // 重ね字を潰すのは母音を捨てる前である。捨てたあとで隣り合った同じ子音を潰すと、
    // memory が m だけになって消える(m と m は元の綴りでは隣り合っていない)。
    spelling.clear();
    skeleton.clear();
    for character in word.chars() {
        if !character.is_ascii() {
            continue;
        }
        let byte = character.to_ascii_lowercase() as u8;
        if spelling.last() != Some(&byte) {
            spelling.push(byte);
        }
    }
    let chars: &[u8] = spelling;
    let raw = skeleton;
    let mut i = 0usize;
    while i < chars.len() {
        let rest = &chars[i..];
        // 綴りが 2〜4 字で 1 音になる組を先に片づける。順序は長い方から。
        if starts_with(rest, "tion") || starts_with(rest, "sion") {
            raw.push('S');
            i += 3; // 残る n は次の周で拾う。
            continue;
        }
        if starts_with(rest, "tch") {
            raw.push('S');
            i += 3;
            continue;
        }
        if starts_with(rest, "ch") || starts_with(rest, "sh") {
            raw.push('S');
            i += 2;
            continue;
        }
        if starts_with(rest, "th") {
            raw.push('s');
            i += 2;
            continue;
        }
        if starts_with(rest, "ph") {
            raw.push('f');
            i += 2;
            continue;
        }
        if starts_with(rest, "gh") {
            i += 2;
            continue;
        }
        if starts_with(rest, "ck") {
            raw.push('k');
            i += 2;
            continue;
        }
        if starts_with(rest, "qu") {
            raw.push('k');
            raw.push('w');
            i += 2;
            continue;
        }
        let character = chars[i] as char;
        i += 1;
        match character {
            'a' | 'e' | 'i' | 'o' | 'u' | 'y' => {}
            'c' => {
                let soft = matches!(chars.get(i), Some(b'e' | b'i' | b'y'));
                raw.push(if soft { 's' } else { 'k' });
            }
            'g' => raw.push('j'),
            'v' => raw.push('b'),
            'l' => raw.push('r'),
            'z' => raw.push('s'),
            'x' => {
                raw.push('k');
                raw.push('s');
            }
            _ if character.is_ascii_alphabetic() => raw.push(character),
            _ => {}
        }
    }
    // 語末の r は落とす。日本語は語末の r を書かない(buffer → バッファ、table → テーブル)。
    if raw.len() >= 2 && raw.ends_with('r') {
        raw.pop();
    }
}

fn starts_with(rest: &[u8], pattern: &str) -> bool {
    rest.len() >= pattern.len() && rest[..pattern.len()] == *pattern.as_bytes()
}

/// レーベンシュタイン距離(挿入・削除・置換を 1 と数える)。
pub fn levenshtein(a: &str, b: &str) -> usize {
    let left: Vec<char> = a.chars().collect();
    let right: Vec<char> = b.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, a) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let substitute = previous[j] + usize::from(a != b);
            current[j + 1] = substitute.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// 骨格どうしの距離が limit(0 か 1)以下なら返す。
///
/// levenshtein を呼ばないのは速さのためである。これは語彙 1 語 × 候補 1 つごとに呼ばれ、
/// 索引の語彙 4 万語では 1 クエリで 80 万回に届く。汎用の DP は行 2 本を確保するので、
/// その確保だけで 1 クエリが数十ミリ秒に伸びた。距離 1 までなら確保無しの一走査で判る。
///
/// 骨格は consonant_skeleton の作りから ASCII だけなので、バイトで見てよい。
fn bounded_distance(a: &str, b: &str, limit: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    if a == b {
        return Some(0);
    }
    if limit == 0 {
        return None;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    if long.len() == short.len() {
        // 置換 1 回で済むか(食い違いが 1 箇所だけか)。
        let mismatches = long.iter().zip(short).filter(|(x, y)| x != y).count();
        return (mismatches == 1).then_some(1);
    }
    // 長い方から 1 字消せば揃うか。食い違いの手前までは同じで、そこから 1 字ずれる。
    let same = long.iter().zip(short).take_while(|(x, y)| x == y).count();
    (long[same + 1..] == short[same..]).then_some(1)
}

/// 拗音・外来音(2 文字で 1 モーラ)。1 文字の表より先に引く。
const DIGRAPH_MORAS: &[(&str, &str)] = &[
    ("キャ", "kya"),
    ("キュ", "kyu"),
    ("キョ", "kyo"),
    ("キェ", "kye"),
    ("ギャ", "gya"),
    ("ギュ", "gyu"),
    ("ギョ", "gyo"),
    ("シャ", "sha"),
    ("シュ", "shu"),
    ("ショ", "sho"),
    ("シェ", "she"),
    ("ジャ", "ja"),
    ("ジュ", "ju"),
    ("ジョ", "jo"),
    ("ジェ", "je"),
    ("チャ", "cha"),
    ("チュ", "chu"),
    ("チョ", "cho"),
    ("チェ", "che"),
    ("ニャ", "nya"),
    ("ニュ", "nyu"),
    ("ニョ", "nyo"),
    ("ヒャ", "hya"),
    ("ヒュ", "hyu"),
    ("ヒョ", "hyo"),
    ("ビャ", "bya"),
    ("ビュ", "byu"),
    ("ビョ", "byo"),
    ("ピャ", "pya"),
    ("ピュ", "pyu"),
    ("ピョ", "pyo"),
    ("ミャ", "mya"),
    ("ミュ", "myu"),
    ("ミョ", "myo"),
    ("リャ", "rya"),
    ("リュ", "ryu"),
    ("リョ", "ryo"),
    ("ファ", "fa"),
    ("フィ", "fi"),
    ("フェ", "fe"),
    ("フォ", "fo"),
    ("フュ", "fyu"),
    ("ティ", "ti"),
    ("トゥ", "tu"),
    ("テュ", "tyu"),
    ("ディ", "di"),
    ("ドゥ", "du"),
    ("デュ", "dyu"),
    ("ウィ", "wi"),
    ("ウェ", "we"),
    ("ウォ", "wo"),
    ("ヴァ", "va"),
    ("ヴィ", "vi"),
    ("ヴェ", "ve"),
    ("ヴォ", "vo"),
    ("ヴュ", "vyu"),
    ("クァ", "kwa"),
    ("クィ", "kwi"),
    ("クェ", "kwe"),
    ("クォ", "kwo"),
    ("グァ", "gwa"),
    ("ツァ", "tsa"),
    ("ツィ", "tsi"),
    ("ツェ", "tse"),
    ("ツォ", "tso"),
    ("スィ", "si"),
    ("ズィ", "zi"),
    ("イェ", "ye"),
];

/// 1 文字 1 モーラ。小書きの仮名が単独で残った場合(拗音の表で拾えなかった並び)も
/// 母音として拾う。
const SINGLE_MORAS: &[(&str, &str)] = &[
    ("ア", "a"),
    ("イ", "i"),
    ("ウ", "u"),
    ("エ", "e"),
    ("オ", "o"),
    ("カ", "ka"),
    ("キ", "ki"),
    ("ク", "ku"),
    ("ケ", "ke"),
    ("コ", "ko"),
    ("サ", "sa"),
    ("シ", "shi"),
    ("ス", "su"),
    ("セ", "se"),
    ("ソ", "so"),
    ("タ", "ta"),
    ("チ", "chi"),
    ("ツ", "tsu"),
    ("テ", "te"),
    ("ト", "to"),
    ("ナ", "na"),
    ("ニ", "ni"),
    ("ヌ", "nu"),
    ("ネ", "ne"),
    ("ノ", "no"),
    ("ハ", "ha"),
    ("ヒ", "hi"),
    ("フ", "fu"),
    ("ヘ", "he"),
    ("ホ", "ho"),
    ("マ", "ma"),
    ("ミ", "mi"),
    ("ム", "mu"),
    ("メ", "me"),
    ("モ", "mo"),
    ("ヤ", "ya"),
    ("ユ", "yu"),
    ("ヨ", "yo"),
    ("ラ", "ra"),
    ("リ", "ri"),
    ("ル", "ru"),
    ("レ", "re"),
    ("ロ", "ro"),
    ("ワ", "wa"),
    ("ヰ", "i"),
    ("ヱ", "e"),
    ("ヲ", "o"),
    ("ン", "n"),
    ("ガ", "ga"),
    ("ギ", "gi"),
    ("グ", "gu"),
    ("ゲ", "ge"),
    ("ゴ", "go"),
    ("ザ", "za"),
    ("ジ", "ji"),
    ("ズ", "zu"),
    ("ゼ", "ze"),
    ("ゾ", "zo"),
    ("ダ", "da"),
    ("ヂ", "ji"),
    ("ヅ", "zu"),
    ("デ", "de"),
    ("ド", "do"),
    ("バ", "ba"),
    ("ビ", "bi"),
    ("ブ", "bu"),
    ("ベ", "be"),
    ("ボ", "bo"),
    ("パ", "pa"),
    ("ピ", "pi"),
    ("プ", "pu"),
    ("ペ", "pe"),
    ("ポ", "po"),
    ("ヴ", "vu"),
    ("ァ", "a"),
    ("ィ", "i"),
    ("ゥ", "u"),
    ("ェ", "e"),
    ("ォ", "o"),
    ("ャ", "ya"),
    ("ュ", "yu"),
    ("ョ", "yo"),
    ("ヮ", "wa"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 単体試験の語彙。実物の索引(仕様書 27 本)から採った語と、当たってほしくない
    /// 紛らわしい語を混ぜてある。第 2 要素は実物の索引での出現数。
    const VOCABULARY: &[(&str, u64)] = &[
        ("scratchpad", 109),
        ("scratch", 93),
        ("pad", 459),
        ("pod", 12),
        ("paid", 3),
        ("buffer", 3970),
        ("buffers", 1200),
        ("before", 2100),
        ("beef", 1),
        ("register", 26368),
        ("registers", 9000),
        ("descriptor", 4113),
        ("memory", 14543),
        ("cache", 2938),
        ("throughput", 53),
        ("interface", 5402),
        ("server", 485),
        ("page", 4864),
        ("transaction", 2908),
        ("index", 2381),
        ("table", 18725),
        ("token", 1610),
        ("endpoint", 3351),
        ("interrupt", 7000),
        ("label", 465),
        ("latency", 118),
        ("serial", 372),
    ];

    fn expand(query: &str) -> Vec<String> {
        expand_terms(query, VOCABULARY)
    }

    #[test]
    fn katakana_run_is_taken_and_other_scripts_are_separators() {
        assert_eq!(katakana_runs("xHCI のスクラッチパッド"), vec!["スクラッチパッド"]);
        // 中黒は区切り。表記が割れている外来語を書き手が切ってくれた形である。
        assert_eq!(katakana_runs("スクラッチ・パッド"), vec!["スクラッチ", "パッド"]);
        // 漢字もひらがなも連なりにならない。
        assert!(katakana_runs("割り込みの処理").is_empty());
        // 1 文字の連なりは骨格が立たないので落とす。
        assert!(katakana_runs("アの本").is_empty());
    }

    #[test]
    fn katakana_becomes_romaji_with_youon_sokuon_choon_and_hatsuon() {
        fn romaji(run: &str) -> Vec<String> {
            romaji_variants(&moras_of(run).expect("カタカナ"))
        }
        // 促音(ッ)は落とし、拗音(ャュョ)は 1 モーラにまとめる。チ には ti の異読が
        // あるので 2 本出る。
        assert_eq!(romaji("スクラッチパッド"), vec!["sukurachipado", "sukuratipado"]);
        assert_eq!(romaji("キャッシュ"), vec!["kyashu"]);
        // 撥音(ン)は n。
        assert_eq!(romaji("エンドポイント"), vec!["endopointo"]);
        // 長音符(ー)は母音を伸ばす読みと r の読みの両方を出す。
        assert_eq!(romaji("サーバ"), vec!["saaba", "sarba"]);
        // シ は sh とは限らない(serial → シリアル)。
        assert_eq!(romaji("シフト"), vec!["shifuto", "sifuto"]);
        // カタカナ以外が混ざれば写せない。
        assert!(moras_of("スクラッチpad").is_none());
    }

    #[test]
    fn skeleton_folds_english_spelling_and_romaji_onto_the_same_consonants() {
        // 外来語のローマ字と元の英語が同じ骨格に落ちる。ここが層の要である。
        for (japanese, english) in [
            ("sukurachipado", "scratchpad"),
            ("bafa", "buffer"),
            ("rejisuta", "register"),
            ("disukuriputa", "descriptor"),
            ("kyashu", "cache"),
            ("memori", "memory"),
            ("suruuputto", "throughput"),
            ("toranzakushon", "transaction"),
            ("indekkusu", "index"),
            ("teeburu", "table"),
        ] {
            assert_eq!(
                consonant_skeleton(japanese),
                consonant_skeleton(english),
                "{japanese} と {english} の骨格が合わない"
            );
        }
        // 母音は捨て、重ね字は潰し、語末の r は落とす。
        assert_eq!(consonant_skeleton("buffer"), "bf");
        assert_eq!(consonant_skeleton("scratchpad"), "skrSpd");
    }

    #[test]
    fn levenshtein_counts_insert_delete_and_substitute() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abc", "ac"), 1);
    }

    #[test]
    fn compound_katakana_falls_back_to_the_split_spelling() {
        // 仕様書は Scratchpad(1 語)と Scratch Pad(2 語)の両方で書かれている。
        // 1 語の綴りが索引にあるなら、それだけを返す(Split の表にあるとおり、片も足すと
        // 狙いのチャンクの順位が落ちる)。
        assert_eq!(expand("スクラッチパッド"), vec!["scratchpad"]);
        // 索引が 2 語の綴りしか持たないなら、片が返る。
        let split_only: &[(&str, u64)] = &[("scratch", 93), ("pad", 459), ("buffer", 3970)];
        assert_eq!(expand_terms("スクラッチパッド", split_only), vec!["scratch", "pad"]);
        // 求められれば、1 語の綴りが索引にあっても両方を返す。
        assert_eq!(
            expand_terms_with("スクラッチパッド", VOCABULARY, Split::Always),
            vec!["scratchpad", "scratch", "pad"]
        );
    }

    #[test]
    fn common_loanwords_reach_their_english_terms() {
        assert_eq!(expand("バッファ"), vec!["buffer"]);
        assert_eq!(expand("レジスタ"), vec!["register"]);
        assert_eq!(expand("ディスクリプタ"), vec!["descriptor"]);
        assert_eq!(expand("キャッシュ"), vec!["cache"]);
        assert_eq!(expand("スループット"), vec!["throughput"]);
        assert_eq!(expand("トランザクション"), vec!["transaction"]);
        assert_eq!(expand("インターフェース"), vec!["interface"]);
        assert_eq!(expand("エンドポイント"), vec!["endpoint"]);
        assert_eq!(expand("メモリ"), vec!["memory"]);
        assert_eq!(expand("テーブル"), vec!["table"]);
        // シ の異読(si)が要る組。sh のままでは serial にも latency にも届かない。
        assert_eq!(expand("シリアル"), vec!["serial"]);
        assert_eq!(expand("レイテンシ"), vec!["latency"]);
    }

    #[test]
    fn ascii_in_the_query_is_left_alone_and_only_terms_are_added() {
        // 既に英語で書かれている部分はそのまま残す(足すのは展開語だけ)。
        assert_eq!(
            expand_query("xHCI のスクラッチパッド", VOCABULARY),
            "xHCI のスクラッチパッド scratchpad"
        );
        // 既にクエリにある語は重ねて足さない。
        assert_eq!(expand("register のレジスタ"), Vec::<String>::new());
        // 足すものが無ければ元のクエリのまま。
        assert_eq!(expand_query("割り込み", VOCABULARY), "割り込み");
    }

    #[test]
    fn non_katakana_japanese_returns_nothing() {
        // 漢字の訳語は音ではなく意味の対応であり、この層は扱わない。interrupt は語彙に
        // あるが、それでも返さない(扱えないものを扱えるふりをしない)。
        assert!(expand("割り込み").is_empty());
        assert!(expand("割り込みの処理と表").is_empty());
    }

    #[test]
    fn word_absent_from_the_vocabulary_returns_nothing() {
        // 語彙に無い綴りは思いついても捨てる。架空の語をクエリに足さない。
        let vocabulary: &[&str] = &["register", "buffer"];
        assert!(expand_terms("スクラッチパッド", vocabulary).is_empty());
        assert!(expand_terms("スループット", vocabulary).is_empty());
        // 語彙が空なら何も返らない。
        let empty: &[&str] = &[];
        assert!(expand_terms("バッファ", empty).is_empty());
    }

    #[test]
    fn short_katakana_is_refused_because_the_skeleton_carries_too_little() {
        // 骨格が子音 1 個の連なりは何にでも当たるので候補にしない。
        let vocabulary: &[&str] = &["page", "pad", "queue", "key"];
        assert!(expand_terms("キー", vocabulary).is_empty());
        assert!(expand_terms("ページ", vocabulary).contains(&"page".to_string()));
    }

    #[test]
    fn the_number_of_added_terms_is_capped() {
        let query = "スクラッチパッドのバッファとレジスタとディスクリプタとキャッシュ\
                     とテーブルとメモリとページ";
        let added = expand_terms_with(query, VOCABULARY, Split::Always);
        assert!(added.len() <= MAX_TERMS, "{added:?} が上限 {MAX_TERMS} を超えた");
    }

    #[test]
    fn a_far_fetched_reading_is_refused_even_when_the_skeleton_agrees() {
        // 骨格は母音を捨てた粗い見方なので、それだけでは足りない。ローマ字と綴りが
        // かけ離れている組は返さない(スケジューラ = sukejuura は、骨格 skj が合う
        // 短い識別子に当たってしまう)。
        let vocabulary: &[(&str, u64)] = &[("scgo", 40), ("scheduler", 200)];
        assert!(expand_terms("スケジューラ", vocabulary).is_empty());
    }

    #[test]
    fn expansion_is_deterministic_and_free_of_duplicates() {
        let added = expand("バッファのバッファ");
        assert_eq!(added, vec!["buffer"]);
        for _ in 0..8 {
            assert_eq!(expand("スクラッチパッド"), vec!["scratchpad"]);
        }
    }

    #[test]
    fn vocabulary_can_be_supplied_as_a_visiting_closure() {
        // 索引のように、語をスライスで貸せない持ち主のための形。
        let words = vec![("buffer".to_string(), 3970u64), ("beef".to_string(), 1u64)];
        let vocabulary = VocabularyFn(|visit: &mut dyn FnMut(&str, u64)| {
            for (word, weight) in &words {
                visit(word, *weight);
            }
        });
        assert_eq!(expand_terms("バッファ", &vocabulary), vec!["buffer"]);
    }
}

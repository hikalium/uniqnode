//! uniqnode CLI。ストアの初期化・操作・検証(SPEC §10 の HTTP API はこの上に載せる)。

use std::io::{Read, Write};
use uniqnode::clock::unix_now;
use uniqnode::store::{Store, StoreConfig, StoreError};

fn usage() -> ! {
    eprintln!(
        "usage: uniqnode <command> <data_dir> [args]\n\
         commands:\n\
           init <dir>                 ストアを初期化し node id を表示する\n\
           status <dir>               件数・seq・node id を表示する(在るストアだけ)\n\
           put <dir>                  標準入力をオブジェクトとして投入し id を表示する\n\
           get <dir> <id>             オブジェクトを標準出力へ書く\n\
           set-ref <dir> <path> <id>  自名前空間の ref を設定する(id が '-' なら tombstone)\n\
           refs <dir>                 ref を一覧する\n\
           fsck <dir>                 全再ハッシュ検査(異常があれば非0で終了。在るストア\n\
                                      だけを開き、ストアでない場所は初期化せず断る)\n\
           backup <dir> <backup_dir>  封印済みセグメント・MANIFEST・node_key・設定を\n\
                                      backup_dir へ写し(増分: 写し済みの封印済みセグメントは\n\
                                      写さない)、写し先を開いて fsck まで通す。ロックを取らない\n\
                                      ので serve と同時に走れる(異常があれば非0で終了)\n\
           gc <dir> [--dry-run] [--threshold <割合>]\n\
                                      pack ごとに、生きているバイト数と孤児(どの ref・pin・\n\
                                      保持表明からも辿れないオブジェクト)のバイト数を数え、\n\
                                      孤児率が閾値(既定 {default_gc_threshold})を超えた封印済み\n\
                                      pack を、生きているものだけを写した新しい pack で置き\n\
                                      換えて孤児のバイト列をディスクから取り戻す。--dry-run は\n\
                                      数えるだけで、ストアのデータには何も書かない(参照表\n\
                                      derived/refs/ だけは作る)。ロックを取るので serve が\n\
                                      開いているストアには走れない(走っている serve には\n\
                                      POST /v1/admin/gc を打つ。docs/design/GC.md)\n\
           pin <dir> <root> <min>     root の到達閉包に min_replicas を要求する(0 で解除)\n\
           admin-keygen <keyfile>     グループ管理者鍵を生成する(公開鍵を表示)\n\
           cert-make <node_id> <group_id> <days>\n\
                                      メンバーシップ証明書の本体を標準出力へ(署名なし)\n\
           cert-sign <keyfile>        標準入力の証明書/失効文に管理者署名を1つ追記する\n\
           cert-verify <dir>          標準入力の証明書を <dir>/groups.json で検証する\n\
           revoke-make <node_id> <group_id>\n\
                                      失効文の本体を標準出力へ(署名なし)\n\
           serve <dir> <addr> [--embed <url>] [--embedder <id>] [--rerank <url>]\n\
                              [--reranker <id>] [ログの指定]\n\
                                      HTTP API を提供する(例: 127.0.0.1:7440、:0 で自動割当)。\n\
                                      --embed を与えると POST /v1/search の既定が BM25 と\n\
                                      埋め込みの RRF 融合になる。ベクトルは embed で作った\n\
                                      キャッシュから読むので、検索が模型の計算を待つことは\n\
                                      ない。届かなければ BM25 だけに劣化して答え、応答の\n\
                                      method と degraded がそれを言う。\n\
                                      --rerank を与えると上位候補の順位をリランカー\n\
                                      (--reranker で模型名、既定 {default_reranker})で\n\
                                      取り直す。届かなければ融合の順位のまま答え、\n\
                                      degraded がそれを言う。\n\
                                      ログは既定で <dir>/logs/serve.log にも残す(下記)\n\
           mcp <dir> [--serve-url <url>] [--writable <コレクション名>]... [--embed <url>]\n\
                     [--embedder <id>] [--rerank <url>] [--reranker <id>] [ログの指定]\n\
                                      標準入出力で MCP(Model Context Protocol)を話す。\n\
                                      LLM エージェント(Claude Code など)に、読む 2 ツール\n\
                                      (search・fetch)と、--writable で許したコレクション\n\
                                      へ書く 2 ツール(add_document: テキスト知識の追加、\n\
                                      fetch_url: URL の取り込み)を出す。--writable は繰り\n\
                                      返せる。1 つも無ければ読むだけで、書くツールは一覧に\n\
                                      載らない。許していないコレクションへの書き込みは\n\
                                      ツールの失敗として断る。標準出力はプロトコル専用で、\n\
                                      ログは標準エラーと <dir>/logs/mcp.log へ出す(登録\n\
                                      した相手が標準エラーを吸うので、ファイルが唯一\n\
                                      読める記録になる)。--serve-url を与えると、\n\
                                      自分でストアを開かず、走っている serve の REST へ\n\
                                      転送する(ストアの排他ロックを取らないので、常駐した\n\
                                      まま ingest・embed が通る。埋め込みと順位の取り直し\n\
                                      を装備するのは転送先の serve なので、--embed・\n\
                                      --embedder・--rerank・--reranker とは併用しない)。\n\
                                      無指定ならストアを直接開く(serve 停止中のストア用。\n\
                                      --embed・--embedder・--rerank・--reranker は serve\n\
                                      と同じ意味で、この形だけが受ける)。\n\
                                      登録例:\n\
                                      claude mcp add --transport stdio uniqnode --\n\
                                      <この実行ファイル> mcp <dir>\n\
                                      --serve-url http://127.0.0.1:7440\n\
                                      --writable notes --writable web\n\
           viewer <dir> <addr> [--serve-url <url>] [ログの指定]\n\
                                      RAG ビューワ(1枚のHTML)をブラウザへ出す\n\
                                      (例: 127.0.0.1:7450、:0 で自動割当)。頁が呼ぶ\n\
                                      /v1/* は走っている serve へ転送するので、自分では\n\
                                      ストアを開かない(排他ロックを取らないため、serve が\n\
                                      常駐したまま起こせる)。--serve-url の既定は\n\
                                      http://127.0.0.1:7440。ログは既定で\n\
                                      <dir>/logs/viewer.log にも残す(下記)\n\
           selfcheck                  この実行ファイルが健全であることを標準出力の1行で\n\
                                      言う(mcp が自分を exec で差し替える前に、新しい\n\
                                      イメージを子プロセスとして起こして確かめる用)\n\
           embed <dir> [--embed <url>] [--embedder <id>]\n\
                                      見えのチャンクのうちベクトルの無いものを埋め込み、\n\
                                      <dir>/derived/embeddings/<id>.vec に足す(導出データ。\n\
                                      消しても作り直せる)。まとまりごとに fsync するので\n\
                                      途中で止めても続きから再開できる。serve 停止中の\n\
                                      ストア用\n\
           sync <dir> <peer_addr>     相手から pull で同期する(serve 停止中のストア用。\n\
                                      serve 中は POST /v1/sync を使う)\n\
           ingest <dir> <collection> <path> [--pdftotext <exe>]\n\
                                      文書を取り込む(.md/.markdown/.txt/.html/.htm/.pdf。ディレクトリ\n\
                                      は再帰。serve 停止中のストア用。serve 中は\n\
                                      PUT /v1/collections/{{c}}/documents/{{name}} を使う。\n\
                                      PDF の抽出は pdftotext に委譲し、--pdftotext の明示\n\
                                      指定が優先、無指定なら PATH を引く)\n\
           fetch <dir> <collection> <url> [--name <名>] [--pdftotext <exe>]\n\
                                      URL を取って取り込む(http/https のみ。取りに行くのは\n\
                                      curl で、版を doc_rev.meta.fetcher に残す。転送は 10 回\n\
                                      まで、{fetch_max_seconds} 秒・{fetch_max_bytes} バイトが\n\
                                      上限)。取れたものが HTML なら外部への依存(スクリプト・\n\
                                      画像・フォント・外部スタイル)を落として自足した 1 枚に\n\
                                      してから、PDF なら pdftotext で、素文はそのまま取り込む。\n\
                                      --name を省くと文書名は URL から導き(ホストとパスを\n\
                                      1 語に。拡張子は残さない)、同じ URL の再取得は同じ\n\
                                      文書名への上書きになる。serve 停止中のストア用。serve\n\
                                      中は POST /v1/collections/{{c}}/fetch を使う\n\
           ingest-annotations <dir> <collection> <data.md> [--manual <承認リスト>]\n\
                                      注釈索引を取り込む(先に PDF を ingest しておく。\n\
                                      タイトルとページ本文の照合に一致した注釈だけが\n\
                                      annotates 辺として入り、不一致は一致率とともに\n\
                                      報告される。承認リストは「spec_id ページ番号」の\n\
                                      行の並びで、載っている注釈は照合に落ちても\n\
                                      manual の検証記録付きで入る。serve 停止中の\n\
                                      ストア用)\n\
           correct <dir> <collection> <誤った言明ID> <新しい言明ID> <理由>\n\
                                      ストアにある言明の誤りを訂正する corrects 辺を\n\
                                      発行する(新しい言明は取り込みと同じトークン照合で\n\
                                      再照合され、根拠行つきの検証記録が付く。索引 ref\n\
                                      annotations/<collection> に訂正の項が足される。\n\
                                      serve 停止中のストア用)\n\
           flood <dir>                書き込み続ける(クラッシュ試験用の内部コマンド)\n\
           install <dir> [--listen <addr>] [--viewer-listen <addr>]\n\
                         [--serve-options \"<引数列>\"] [--backup-dir <dir>] [--bin <path>]\n\
                         [--unit-dir <dir>] [--no-start]\n\
                                      serve・viewer・毎日の backup を user 単位の systemd に\n\
                                      据える(docs/mop/SYSTEMD.md)。走っている自分自身を\n\
                                      --bin(既定 ~/.local/bin/uniqnode)へ写し、unit 4 本と\n\
                                      drop-in 3 本を --unit-dir(既定 ~/.config/systemd/user)\n\
                                      に書き、daemon-reload、enable と restart、\n\
                                      loginctl enable-linger の後、serve と viewer 経由の\n\
                                      /v1/status が同じ node_id を返すことと、backup を 1 回\n\
                                      走らせた写し先(--backup-dir、既定 ~/uniqnode-backup)\n\
                                      が fsck で緑であることまで確かめる。--listen の既定は\n\
                                      {default_listen}、--viewer-listen は {default_viewer_listen}。\n\
                                      --serve-options は serve の追加の引数(--embed など)を\n\
                                      1 つの文字列で。--no-start は daemon-reload までで\n\
                                      止める。再実行は更新(写し直し・書き直し・restart)。\n\
                                      <dir> は /tmp の下に置けない(unit の PrivateTmp)。\n\
                                      system 単位(/etc/systemd/system)は未実装で、\n\
                                      SYSTEMD.md の手順で行う\n\
         \n\
         serve・mcp・viewer のログの指定(常駐する命令だけが持つ。既定は保存する):\n\
           --log <path>               保存先を変える(既定 <dir>/logs/<serve|mcp|viewer>.log。\n\
                                      既定の道に書けなければ $XDG_STATE_HOME/uniqnode/logs/、\n\
                                      無ければ ~/.local/state/uniqnode/logs/ へ倒し、その旨を\n\
                                      最初の行で言う。--log で指した道は倒さない)\n\
           --no-log                   ファイルへ残さず標準エラーだけに出す\n\
           --log-max-bytes <n>        1 世代の上限(既定 {default_max_bytes})。越えたら\n\
                                      <path>.1 へ送って新しい世代を開き、{generations} 世代\n\
                                      まで残す(それより古いものは消える)\n\
         ログは標準エラーとファイルの両方に同じ行が出る。行頭は UTC の時刻と pid",
        default_reranker = uniqnode::rerank::DEFAULT_RERANKER_ID,
        default_gc_threshold = uniqnode::gc::DEFAULT_THRESHOLD,
        fetch_max_seconds = uniqnode::fetch::DEFAULT_MAX_SECONDS,
        fetch_max_bytes = uniqnode::fetch::DEFAULT_MAX_BYTES,
        default_listen = uniqnode::install::DEFAULT_LISTEN,
        default_viewer_listen = uniqnode::install::DEFAULT_VIEWER_LISTEN,
        default_max_bytes = uniqnode::log::DEFAULT_MAX_BYTES,
        generations = uniqnode::log::RETAINED_GENERATIONS,
    );
    std::process::exit(2);
}

/// ディレクトリを再帰して通常ファイルを集める(名前順)。
fn collect_files(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    if root.is_dir() {
        let mut entries: Vec<_> =
            std::fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            collect_files(&entry.path(), out)?;
        }
    } else {
        out.push(root.to_path_buf());
    }
    Ok(())
}

/// 取り込みの CLI 本体(INGEST の「CLI と API」節と「PDF 抽出」節)。対象外のファイルは
/// 黙って捨てず、最後に一覧で報告する(must/0022 の同型)。pdftotext の起動と版の取得は
/// 最初の PDF に当たったとき一度だけ行い、以後の PDF で使い回す。
fn run_ingest(
    dir: &str,
    collection: &str,
    root: &str,
    pdftotext: Option<&str>,
) -> Result<(), StoreError> {
    let root_path = std::path::Path::new(root);
    if !root_path.exists() {
        return Err(StoreError::Invalid(format!("{root}: 存在しない")));
    }
    let mut files = Vec::new();
    collect_files(root_path, &mut files)?;
    let base = if root_path.is_dir() {
        root_path
    } else {
        root_path.parent().unwrap_or_else(|| std::path::Path::new(""))
    };
    let mut store = open(dir);
    let mut skipped: Vec<String> = Vec::new();
    let mut pdf_extractor: Option<uniqnode::ingest::PdfExtractor> = None;
    for file in &files {
        let relative = file.strip_prefix(base).unwrap_or(file);
        let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("");
        let Some(media) = uniqnode::ingest::media_for_extension(extension) else {
            skipped.push(relative.display().to_string());
            continue;
        };
        let bytes = std::fs::read(file)?;
        let extracted;
        let text: &str = if media == "pdf" {
            if pdf_extractor.is_none() {
                let located = uniqnode::ingest::PdfExtractor::locate(
                    pdftotext.map(std::path::Path::new),
                )
                .map_err(StoreError::Invalid)?;
                pdf_extractor = Some(located);
            }
            let extractor = pdf_extractor.as_ref().expect("直前に確保した");
            extracted = extractor.extract(&bytes)?;
            &extracted
        } else {
            match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(_) => {
                    return Err(StoreError::Invalid(format!(
                        "{}: UTF-8 でない({media} として取り込めない)",
                        file.display()
                    )));
                }
            }
        };
        let name_path = relative.with_extension("");
        let name = name_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        // PDF は節見出しの経路も載せる(node/src/outline.rs)。取れなければ理由を出して、
        // 見出しの無いチャンクとして続ける。
        let (chunks, outline_reason) =
            uniqnode::ingest::chunk_for_media_with_source(media, text, &bytes);
        if let Some(reason) = outline_reason {
            uniqnode::log_line!("uniqnode: ingest: {name} の節見出し: {reason}");
        }
        let extractor_label = if media == "pdf" {
            pdf_extractor.as_ref().map(|e| e.extractor.as_str())
        } else {
            None
        };
        let outcome = uniqnode::ingest::ingest_document(
            &mut store,
            &uniqnode::ingest::DocumentInput {
                collection,
                name: &name,
                source: &bytes,
                media,
                chunks: &chunks,
                extractor: extractor_label,
                extra_meta: &[],
            },
        )?;
        let state = if outcome.ref_updated { "updated" } else { "no-op" };
        println!(
            "{collection}/{name}: {state} chunks={} new_objects={} doc_rev={}",
            chunks.len(),
            outcome.new_objects,
            outcome.doc_rev_id
        );
    }
    for path in &skipped {
        println!("対象外(拡張子): {path}");
    }
    Ok(())
}

/// URL からの取り込みの CLI 本体(INGEST の「URL からの取り込み」節)。取る・見分ける・
/// 名前を決めるは node/src/fetch.rs にあり、API の POST /v1/collections/{c}/fetch と同じ道を
/// 通る(should/0135)。ストアを開くのは取ってからで、取れない URL でストアを作らない。
fn run_fetch(
    dir: &str,
    collection: &str,
    url: &str,
    name: Option<&str>,
    pdftotext: Option<&str>,
) -> Result<(), StoreError> {
    let request = uniqnode::fetch::FetchRequest {
        url,
        name,
        limits: uniqnode::fetch::FetchLimits::default(),
    };
    // PDF だったときだけ pdftotext を引く(--pdftotext の明示指定が優先、無指定なら PATH)。
    let mut extract_pdf = |pdf: &[u8]| {
        let extractor =
            uniqnode::ingest::PdfExtractor::locate(pdftotext.map(std::path::Path::new))
                .map_err(uniqnode::fetch::FetchError::ToolMissing)?;
        uniqnode::fetch::pdf_text_with(&extractor, pdf)
    };
    let document = uniqnode::fetch::fetch_document(&request, &mut extract_pdf)
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    if let Some(reason) = &document.outline_reason {
        uniqnode::log_line!("uniqnode: ingest: {} の節見出し: {reason}", document.name);
    }
    let mut store = open(dir);
    let outcome = uniqnode::ingest::ingest_document(&mut store, &document.input(collection))?;
    let state = if outcome.ref_updated { "updated" } else { "no-op" };
    println!(
        "{collection}/{}: {state} chunks={} new_objects={} doc_rev={} media={} final_url={}",
        document.name,
        document.chunks.len(),
        outcome.new_objects,
        outcome.doc_rev_id,
        document.media,
        document.final_url
    );
    if let Some(dropped) = &document.dropped {
        println!(
            "dropped: scripts={} stylesheets={} images={} frames={} fonts={} handlers={} others={}",
            dropped.scripts,
            dropped.stylesheets,
            dropped.images,
            dropped.frames,
            dropped.fonts,
            dropped.handlers,
            dropped.others
        );
    }
    Ok(())
}

/// 注釈索引の取り込み CLI 本体(INGEST の「注釈の取り込みと照合」節)。一致した注釈だけを取り込み、
/// 不一致は取り込まずに一致率とともに報告する(どの注釈がどの一致率で落ちたか)。
fn run_ingest_annotations(
    dir: &str,
    collection: &str,
    data_md: &str,
    manual: Option<&str>,
) -> Result<(), StoreError> {
    let text = std::fs::read_to_string(data_md)
        .map_err(|error| StoreError::Invalid(format!("{data_md}: 読めない: {error}")))?;
    let entries = uniqnode::ingest::parse_annotation_index(&text)
        .map_err(|error| StoreError::Invalid(format!("{data_md}: {error}")))?;
    let approvals = match manual {
        None => Default::default(),
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| StoreError::Invalid(format!("{path}: 読めない: {error}")))?;
            uniqnode::ingest::parse_manual_approvals(&text)
                .map_err(|error| StoreError::Invalid(format!("{path}: {error}")))?
        }
    };
    let mut store = open(dir);
    let outcome =
        uniqnode::ingest::ingest_annotations(&mut store, collection, &entries, &approvals)?;
    for accepted in &outcome.accepted {
        println!(
            "取り込み: {} p.{} {} (method={}, 一致 {}/{})",
            accepted.spec_id,
            accepted.page,
            accepted.title,
            accepted.method,
            accepted.matched_tokens,
            accepted.total_tokens
        );
    }
    for rejected in &outcome.rejected {
        println!(
            "不一致: {} p.{} {} (一致 {}/{})",
            rejected.spec_id,
            rejected.page,
            rejected.title,
            rejected.matched_tokens,
            rejected.total_tokens
        );
    }
    let state = if outcome.ref_updated { "updated" } else { "no-op" };
    println!(
        "annotations/{collection}: {state} 取り込み={} 不一致={} new_objects={} index={}",
        outcome.accepted.len(),
        outcome.rejected.len(),
        outcome.new_objects,
        outcome.index_id
    );
    Ok(())
}

/// ストアを開く(無ければ初期化する)。開けない理由(別のプロセスがロックを持っている、
/// など)はログの出口を通す: mcp がこれで落ちたとき、標準エラーは登録した相手の中で
/// 消えるためである。この道を通るのは、状態を作ることが役目の命令(init、put・set-ref・
/// pin・ingest・correct・sync・flood の書き込み系、serve・mcp・embed の初回起動で
/// ディレクトリを用意する常駐と派生の作成)。検査と閲覧は open_existing を通る。
fn open(dir: &str) -> Store {
    exit_unless_opened(Store::open(StoreConfig::new(dir)))
}

/// 在るストアだけを開く。ストアでない場所(空のディレクトリ・存在しない道)には何も作らず
/// 理由を言って 1 で終わる。fsck・status・get・refs はこちら: 検査や閲覧が空のディレクトリ
/// を新しいノードとして初期化すると、復元先を先に fsck した写しが別の node_key を持って
/// backup に断られる(BACKUP.md の「復元」)。
fn open_existing(dir: &str) -> Store {
    exit_unless_opened(Store::open_existing(StoreConfig::new(dir)))
}

fn exit_unless_opened(opened: Result<Store, StoreError>) -> Store {
    match opened {
        Ok(s) => s,
        Err(e) => {
            uniqnode::log_line!("uniqnode: ストアを開けない: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    // 自己検査。mcp の自己置換(node/src/mcp.rs)が exec の前に子プロセスとして起こす
    // 唯一の命令で、データディレクトリを取らないので引数の数の検査より前に見る。
    if arguments.len() == 2 && arguments[1] == uniqnode::mcp::SELF_CHECK_COMMAND {
        println!("{}", uniqnode::mcp::self_check_report());
        return;
    }
    if arguments.len() < 3 {
        usage();
    }
    let command = arguments[1].as_str();
    let dir = arguments[2].as_str();
    let result = run(command, dir, &arguments[3..]);
    if let Err(e) = result {
        eprintln!("uniqnode: {e}");
        std::process::exit(1);
    }
}

/// 埋め込みの指定(serve・mcp・embed が共用する読み取り。should/0135)。
struct EmbedOptions {
    url: String,
    embedder_id: String,
    /// --embed が明示されたか。serve は明示されたときだけ埋め込みを装備する
    /// (既定で外部プロセスに依存させない)。
    requested: bool,
    /// --embedder が明示されたか。装備の可否は --embed だけで決まるが、装備する口を持たない
    /// 転送する形(mcp・viewer)が「指定を受けたのに効かせていない」ことを言えるように、
    /// 明示されたことだけは覚えておく(reranker_named と同型)。embed 命令では --embedder
    /// は正当な指定なので、そこはこの欄を見ない。
    embedder_named: bool,
    /// 順位を取り直すリランカーの URL(--rerank)。
    rerank_url: String,
    reranker_id: String,
    /// --rerank が明示されたか。
    rerank_requested: bool,
    /// --reranker が明示されたか。装備の可否は --rerank だけで決まる(--embedder と
    /// --embed の関係と同じ)が、装備する口を持たない命令が「指定を受けたのに効かせて
    /// いない」ことを言えるように、明示されたことだけは覚えておく。
    reranker_named: bool,
}

impl EmbedOptions {
    /// 順位の取り直しの指定(--rerank か --reranker)が明示されていれば、その引数の
    /// 名前を返す。装備する口を持たない命令(embed・転送する形の mcp・viewer)が、
    /// 受け取った指定を黙って捨てずに断るために問う(must/0022 の同型)。
    fn rerank_flag(&self) -> Option<&'static str> {
        match (self.rerank_requested, self.reranker_named) {
            (true, _) => Some("--rerank"),
            (false, true) => Some("--reranker"),
            (false, false) => None,
        }
    }

    /// 転送する形(mcp・viewer)に渡された、転送先の serve でしか効かない装備の指定。
    /// 埋め込み(--embed か --embedder)が先、次に順位の取り直しである。
    fn misplaced_equipment_flag(&self) -> Option<&'static str> {
        match (self.requested, self.embedder_named) {
            (true, _) => Some("--embed"),
            (false, true) => Some("--embedder"),
            (false, false) => self.rerank_flag(),
        }
    }
}

/// gc の引数を読む(--dry-run の有無と、--threshold <割合>。0 以上 1 以下)。知らない引数と、
/// 割合として読めない・範囲外の閾値は黙って捨てず usage で落とす。
fn parse_gc_options(rest: &[String]) -> uniqnode::gc::GcOptions {
    let mut options = uniqnode::gc::GcOptions {
        dry_run: false,
        threshold: uniqnode::gc::DEFAULT_THRESHOLD,
    };
    let mut at = 0;
    while at < rest.len() {
        match rest[at].as_str() {
            "--dry-run" => {
                options.dry_run = true;
                at += 1;
            }
            "--threshold" => {
                let text = rest.get(at + 1).unwrap_or_else(|| usage());
                match text.parse::<f64>() {
                    Ok(value) if (0.0..=1.0).contains(&value) => options.threshold = value,
                    _ => {
                        eprintln!("uniqnode: --threshold は 0 以上 1 以下の割合: {text}");
                        usage();
                    }
                }
                at += 2;
            }
            _ => usage(),
        }
    }
    options
}

/// gc の報告を出力する。pack ごとに 1 行、集計、回収したなら何を書き直したか、各相の所要
/// (S・A・C がロックの中)。読み方は docs/design/GC.md。
fn print_gc_report(report: &uniqnode::gc::GcReport) {
    for pack in &report.packs {
        println!(
            "pack {:06} {}: objects {} bytes {}, live {}, garbage {} ({:.1}%) -> {}",
            pack.number,
            if pack.sealed { "sealed" } else { "active" },
            pack.objects,
            pack.bytes,
            pack.live_bytes,
            pack.garbage_bytes(),
            pack.garbage_ratio() * 100.0,
            if pack.compact { "compact" } else { "keep" }
        );
    }
    println!(
        "gc: packs {} (sealed {}, compact {}), objects {} live {} garbage {}, \
         garbage bytes {} (compact would reclaim {}), roots {}",
        report.packs.len(),
        report.sealed_packs(),
        report.compact_packs(),
        report.objects,
        report.live_objects,
        report.garbage_objects(),
        report.garbage_bytes(),
        report.compact_bytes(),
        report.roots
    );
    let pack_list = |numbers: &[u64]| -> String {
        numbers.iter().map(|n| format!("{n:06}")).collect::<Vec<_>>().join(" ")
    };
    let optional = |number: Option<u64>| -> String {
        number.map(|n| format!("{n:06}")).unwrap_or_else(|| "none".to_string())
    };
    if report.dry_run {
        println!(
            "gc: live set computed in {} ms (threshold {}, dry-run: nothing written)",
            report.live_set_elapsed().as_millis(),
            report.threshold
        );
    } else if report.compacted.is_empty() {
        println!(
            "gc: nothing to compact (threshold {}, sealed in S: {})",
            report.threshold,
            optional(report.sealed_in_seal_phase)
        );
    } else {
        println!(
            "gc: compacted packs {} -> new pack {}, reclaimed {} bytes (disk {} bytes), \
             revived {} objects, sealed in S: {}, sealed in C: {} (threshold {})",
            pack_list(&report.compacted),
            optional(report.new_pack),
            report.reclaimed_bytes,
            report.disk_bytes_freed,
            report.revived_objects,
            optional(report.sealed_in_seal_phase),
            optional(report.sealed_active),
            report.threshold
        );
    }
    let phases = &report.phases;
    println!(
        "gc: phases S {} ms, P {} ms (tables built {} reused {}), A {} ms, B {} ms, C {} ms, \
         D {} ms; locked (S+A+C) {} ms",
        phases.seal.as_millis(),
        phases.table.as_millis(),
        report.tables_built,
        report.tables_reused,
        phases.analyze.as_millis(),
        phases.copy.as_millis(),
        phases.commit.as_millis(),
        phases.delete.as_millis(),
        phases.locked().as_millis()
    );
}

/// --embed <url> と --embedder <id> を読む。知らない引数は黙って捨てず usage で落とす。
fn parse_embed_options(rest: &[String]) -> EmbedOptions {
    let mut options = EmbedOptions {
        url: uniqnode::embed::DEFAULT_EMBEDDING_URL.to_string(),
        embedder_id: uniqnode::embed::DEFAULT_EMBEDDER_ID.to_string(),
        requested: false,
        embedder_named: false,
        rerank_url: uniqnode::rerank::DEFAULT_RERANK_URL.to_string(),
        reranker_id: uniqnode::rerank::DEFAULT_RERANKER_ID.to_string(),
        rerank_requested: false,
        reranker_named: false,
    };
    let mut at = 0;
    while at < rest.len() {
        let value = || rest.get(at + 1).cloned().unwrap_or_else(|| usage());
        match rest[at].as_str() {
            "--embed" => {
                options.url = value();
                options.requested = true;
            }
            "--embedder" => {
                options.embedder_id = value();
                options.embedder_named = true;
            }
            // 順位の取り直しは埋め込みと同じ流儀で装備する: 明示されたときだけ繋ぎ、
            // 起動時に相手の生存は確かめない(should/0114)。届かなければ融合の順位の
            // まま答え、理由が応答の degraded に出る。
            "--rerank" => {
                options.rerank_url = value();
                options.rerank_requested = true;
            }
            "--reranker" => {
                options.reranker_id = value();
                options.reranker_named = true;
            }
            _ => usage(),
        }
        at += 2;
    }
    options
}

/// ログの指定(常駐する serve・mcp・viewer が持つ)。既定は保存する。ログは重要なデバッグ
/// 資料であり、シェルのリダイレクトを忘れたら失われる、という置き方をしない。
struct LogOptions {
    /// 保存先(--log)。無指定なら <data_dir>/logs/<役割>.log。
    path: Option<String>,
    /// ファイルへ残すか(--no-log で false)。
    enabled: bool,
    /// 1 世代の上限(--log-max-bytes)。
    max_bytes: u64,
}

/// 常駐する命令(serve・mcp・viewer)の指定。
struct RunOptions {
    embed: EmbedOptions,
    log: LogOptions,
}

/// ログの指定だけを抜き取り、残りは埋め込みの読み手に渡す(読み取りを二重に実装
/// しない。should/0135)。
fn parse_run_options(rest: &[String]) -> RunOptions {
    let mut log = LogOptions {
        path: None,
        enabled: true,
        max_bytes: uniqnode::log::DEFAULT_MAX_BYTES,
    };
    let mut others = Vec::new();
    let mut at = 0;
    while at < rest.len() {
        let value = || rest.get(at + 1).cloned().unwrap_or_else(|| usage());
        match rest[at].as_str() {
            "--log" => {
                log.path = Some(value());
                at += 2;
            }
            "--no-log" => {
                log.enabled = false;
                at += 1;
            }
            "--log-max-bytes" => {
                let text = value();
                log.max_bytes = match text.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => bytes,
                    // 誤った指定を既定で埋めて黙って進まない(must/0022)。
                    _ => {
                        eprintln!("uniqnode: --log-max-bytes は正の整数: {text}");
                        std::process::exit(2);
                    }
                };
                at += 2;
            }
            _ => {
                others.push(rest[at].clone());
                at += 1;
            }
        }
    }
    RunOptions { embed: parse_embed_options(&others), log }
}

/// ログの保存を始める(既定で有効)。既定の道が開けなければ利用者の書ける場所へ倒し、
/// --log で明示された道が開けなければ倒さずに理由を言って標準エラーだけで続ける(どちらの
/// 判断も uniqnode::log にある。should/0135)。黙って落とさない(must/0022)が、ログを
/// 書けないことは serve や mcp を止める理由にはしない: 提供できる仕事があるのに、記録の
/// 都合で断る方が損である(should/0114)。
fn start_logging(dir: &str, role: &str, options: &LogOptions) {
    if !options.enabled {
        eprintln!("uniqnode: {role}: --no-log によりログはファイルに残さない(標準エラーだけ)");
        return;
    }
    let opened = match &options.path {
        Some(given) => {
            let path = std::path::PathBuf::from(given);
            uniqnode::log::open(&path, options.max_bytes).map(|()| path)
        }
        None => uniqnode::log::open_default(std::path::Path::new(dir), role, options.max_bytes),
    };
    match opened {
        Ok(path) => uniqnode::log_line!(
            "uniqnode: {role}: ログを {} に残す(1 世代 {} バイト、{} 世代まで保持)",
            path.display(),
            options.max_bytes,
            uniqnode::log::RETAINED_GENERATIONS
        ),
        Err(message) => eprintln!(
            "uniqnode: {role}: ログをファイルに残せない({message})。\
             標準エラーだけに出して続ける(--log で別の道を指せる)"
        ),
    }
}

/// mcp の指定(転送先の serve、書き込みを許すコレクション、埋め込み・ログの指定)。
struct McpOptions {
    /// 走っている serve へ転送する形の転送先(--serve-url)。無ければストアを直接開く。
    serve_url: Option<String>,
    /// 書き込みを許すコレクション(--writable。繰り返せる)。空なら読むだけで、書く
    /// ツール(add_document・fetch_url)は tools/list に載らない。
    writable: Vec<String>,
    run: RunOptions,
}

/// --serve-url <url> と --writable <コレクション名> だけを抜き取り、残りは serve と同じ
/// 読み手に渡す(埋め込みとログの指定の読み取りを二重に実装しない。should/0135)。
/// コレクション名は空でなく / を含まない 1 語で、外れていれば usage で落とす(黙って
/// 捨てると、許したつもりのコレクションに書けない)。同じ名前の繰り返しは 1 つに畳む。
fn parse_mcp_options(rest: &[String]) -> McpOptions {
    let mut serve_url = None;
    let mut writable: Vec<String> = Vec::new();
    let mut others = Vec::new();
    let mut at = 0;
    while at < rest.len() {
        match rest[at].as_str() {
            "--serve-url" => {
                serve_url = Some(rest.get(at + 1).cloned().unwrap_or_else(|| usage()));
                at += 2;
            }
            "--writable" => {
                let collection = rest.get(at + 1).cloned().unwrap_or_else(|| usage());
                if collection.is_empty() || collection.contains('/') {
                    eprintln!(
                        "uniqnode: --writable はコレクション名(空でなく / を含まない 1 語): \
                         {collection:?}"
                    );
                    std::process::exit(2);
                }
                if !writable.contains(&collection) {
                    writable.push(collection);
                }
                at += 2;
            }
            _ => {
                others.push(rest[at].clone());
                at += 1;
            }
        }
    }
    McpOptions { serve_url, writable, run: parse_run_options(&others) }
}

/// install の指定。既定は home の下(node/src/install.rs の Options::defaults)。知らない
/// 引数は黙って捨てず usage で落とす。
fn parse_install_options(dir: &str, rest: &[String]) -> uniqnode::install::Options {
    let home = match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => std::path::PathBuf::from(home),
        _ => {
            eprintln!("uniqnode: install: HOME が無いので既定の置き場を決められない");
            std::process::exit(2);
        }
    };
    let mut options =
        uniqnode::install::Options::defaults(std::path::PathBuf::from(dir), &home);
    let mut at = 0;
    while at < rest.len() {
        let value = || rest.get(at + 1).cloned().unwrap_or_else(|| usage());
        match rest[at].as_str() {
            "--listen" => options.listen = value(),
            "--viewer-listen" => options.viewer_listen = value(),
            "--serve-options" => options.serve_options = value(),
            "--backup-dir" => options.backup_dir = std::path::PathBuf::from(value()),
            "--bin" => options.binary = std::path::PathBuf::from(value()),
            "--unit-dir" => options.unit_dir = std::path::PathBuf::from(value()),
            "--no-start" => {
                options.start = false;
                at += 1;
                continue;
            }
            _ => usage(),
        }
        at += 2;
    }
    options
}

/// 指定から埋め込みクライアントを組む(誤った指定はここで落とす)。
fn embedder_from(options: &EmbedOptions) -> uniqnode::embed::Embedder {
    match uniqnode::embed::Embedder::new(&options.url, &options.embedder_id) {
        Ok(embedder) => embedder,
        Err(message) => {
            uniqnode::log_line!("uniqnode: {message}");
            std::process::exit(2);
        }
    }
}

/// 指定からリランカーを組む(serve と、ストアを直接開く形の mcp が同じ組み立てを使う。
/// should/0135)。埋め込みと同じ扱いで、明示されたときだけ装備し、起動時に相手の生存は
/// 確かめない(should/0114)。誤った URL や識別子はここで落とす: 検索のたびに同じ誤りを
/// 言うより、起動時に一度言うほうが直しやすい。
fn reranker_from(options: &EmbedOptions) -> Option<uniqnode::rerank::Reranker> {
    if !options.rerank_requested {
        return None;
    }
    match uniqnode::rerank::Reranker::new(&options.rerank_url, &options.reranker_id) {
        Ok(reranker) => {
            uniqnode::log_line!(
                "uniqnode: rerank: {} ({})",
                reranker.reranker_id(),
                reranker.endpoint()
            );
            Some(reranker)
        }
        Err(message) => {
            uniqnode::log_line!("uniqnode: rerank: {message}");
            std::process::exit(2);
        }
    }
}

fn run(command: &str, dir: &str, rest: &[String]) -> Result<(), StoreError> {
    match command {
        "init" | "status" => {
            // init は無ければ作り、status は在るものを見るだけ。表示は同じ。
            let store = if command == "init" {
                open(dir)
            } else {
                open_existing(dir)
            };
            println!("node_id: {}", store.node_id_hex());
            println!("objects: {}", store.object_count());
            println!("last_seq: {}", store.last_seq());
        }
        "put" => {
            let mut store = open(dir);
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes)?;
            let (id, new) = store.put_object(&bytes)?;
            println!("{id}{}", if new { "" } else { " (existing)" });
        }
        "get" => {
            let store = open_existing(dir);
            let id = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            match store.get_object(id)? {
                Some(bytes) => std::io::stdout().write_all(&bytes)?,
                None => {
                    // ローカル不保持の事実であって、不存在の言明ではない(SPEC §10)。
                    eprintln!("not held locally: {id}");
                    std::process::exit(4);
                }
            }
        }
        "set-ref" => {
            let mut store = open(dir);
            if rest.len() < 2 {
                usage();
            }
            let path = rest[0].as_str();
            let target = if rest[1] == "-" { None } else { Some(rest[1].as_str()) };
            let seq = store.set_ref(path, target)?;
            println!("seq: {seq}");
        }
        "refs" => {
            let store = open_existing(dir);
            for (name, state) in store.list_refs() {
                let target = state.target.as_deref().unwrap_or("(tombstone)");
                println!("{name} -> {target} (seq {})", state.seq);
            }
        }
        "ingest" => {
            let collection = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let root = rest.get(1).map(String::as_str).unwrap_or_else(|| usage());
            let pdftotext = match rest.get(2).map(String::as_str) {
                None => None,
                Some("--pdftotext") => {
                    Some(rest.get(3).map(String::as_str).unwrap_or_else(|| usage()))
                }
                Some(_) => usage(),
            };
            if rest.len() > 4 {
                usage();
            }
            run_ingest(dir, collection, root, pdftotext)?;
        }
        "fetch" => {
            let collection = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let url = rest.get(1).map(String::as_str).unwrap_or_else(|| usage());
            let mut name = None;
            let mut pdftotext = None;
            let mut at = 2;
            while at < rest.len() {
                let value = rest.get(at + 1).map(String::as_str).unwrap_or_else(|| usage());
                match rest[at].as_str() {
                    "--name" => name = Some(value),
                    "--pdftotext" => pdftotext = Some(value),
                    // 知らない引数は黙って捨てず usage で落とす。
                    _ => usage(),
                }
                at += 2;
            }
            run_fetch(dir, collection, url, name, pdftotext)?;
        }
        "ingest-annotations" => {
            let collection = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let data_md = rest.get(1).map(String::as_str).unwrap_or_else(|| usage());
            let manual = match rest.get(2).map(String::as_str) {
                None => None,
                Some("--manual") => {
                    Some(rest.get(3).map(String::as_str).unwrap_or_else(|| usage()))
                }
                Some(_) => usage(),
            };
            if rest.len() > 4 {
                usage();
            }
            run_ingest_annotations(dir, collection, data_md, manual)?;
        }
        "correct" => {
            let collection = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let wrong_id = rest.get(1).map(String::as_str).unwrap_or_else(|| usage());
            let new_id = rest.get(2).map(String::as_str).unwrap_or_else(|| usage());
            let reason = rest.get(3).map(String::as_str).unwrap_or_else(|| usage());
            if rest.len() > 4 {
                usage();
            }
            let mut store = open(dir);
            let outcome = uniqnode::ingest::correct_statement(
                &mut store, collection, wrong_id, new_id, reason,
            )?;
            println!(
                "corrects: {} (再照合 一致 {}/{}, verification={})",
                outcome.corrects_edge_id,
                outcome.matched_tokens,
                outcome.total_tokens,
                outcome.verification_id
            );
            let state = if outcome.ref_updated { "updated" } else { "no-op" };
            println!(
                "annotations/{collection}: {state} new_objects={} index={}",
                outcome.new_objects, outcome.index_id
            );
        }
        "fsck" => {
            let store = open_existing(dir);
            let report = store.fsck()?;
            println!(
                "objects: {} refs: {} errors: {}",
                report.objects_checked,
                report.refs_checked,
                report.errors.len()
            );
            if !report.unlisted_packs.is_empty() {
                println!(
                    "unlisted packs (MANIFEST に無く最後でもない。事実であってエラーではない): {}",
                    report
                        .unlisted_packs
                        .iter()
                        .map(|n| format!("{n:06}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            for error in &report.errors {
                eprintln!("fsck: {error}");
            }
            if !report.errors.is_empty() {
                std::process::exit(3);
            }
        }
        "backup" => {
            let backup_dir = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            if rest.len() > 1 {
                usage();
            }
            let report = uniqnode::backup::run(
                std::path::Path::new(dir),
                std::path::Path::new(backup_dir),
            )?;
            for name in &report.sealed_copied {
                println!("copied {name}");
            }
            for name in &report.sealed_unchanged {
                println!("unchanged {name}");
            }
            for name in &report.active_copied {
                println!("copied {name} (active)");
            }
            for name in &report.settings_copied {
                println!("copied {name}");
            }
            for name in &report.settings_only_in_backup {
                println!("only in backup {name} (写し元には無い。消していない)");
            }
            for name in &report.packs_removed {
                println!("removed {name} (not in source MANIFEST)");
            }
            for name in &report.segments_only_in_backup {
                println!("only in backup {name} (写し元には無い。消していない)");
            }
            for (name, bytes) in &report.torn_tails_cut {
                println!("cut {name} ({bytes} bytes の書き込み途中の尻尾を検証で切り詰めた)");
            }
            println!(
                "backup: sealed copied {} unchanged {}, active {}, removed {}, settings {}, \
                 bytes {}, not copied: {}",
                report.sealed_copied.len(),
                report.sealed_unchanged.len(),
                report.active_copied.len(),
                report.packs_removed.len(),
                report.settings_copied.len(),
                report.copied_bytes,
                report.not_copied.join(" ")
            );
            println!(
                "verify: objects {} refs {} errors {}",
                report.verification.objects_checked,
                report.verification.refs_checked,
                report.verification.errors.len()
            );
            for error in &report.verification.errors {
                eprintln!("verify: {error}");
            }
            if !report.is_clean() {
                std::process::exit(3);
            }
        }
        "gc" => {
            let options = parse_gc_options(rest);
            let store = std::sync::Mutex::new(open_existing(dir));
            let report = uniqnode::gc::run(&store, options)?;
            print_gc_report(&report);
        }
        "sync" => {
            let peer_address = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let store = std::sync::Mutex::new(open(dir));
            let peer = uniqnode::sync::HttpPeer::new(peer_address);
            match uniqnode::sync::sync_from_peer(&store, &peer) {
                Ok(report) => {
                    println!(
                        "signers: {} ingested: {} known: {} fetched: {} absent: {} mismatches: {}",
                        report.signers_seen,
                        report.records_ingested,
                        report.records_already_known,
                        report.objects_fetched,
                        report.objects_absent,
                        report.hash_mismatches
                    );
                    if report.hash_mismatches > 0 {
                        std::process::exit(5);
                    }
                }
                Err(e) => {
                    eprintln!("uniqnode: sync failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        // ---- グループ鍵のセレモニー(SPEC §6.4)。第1引数はコマンドごとの意味を持つ ----
        "admin-keygen" => {
            let path = std::path::Path::new(dir);
            if path.exists() {
                return Err(StoreError::Invalid(format!("{dir} は既に存在する")));
            }
            let seed = uniqnode::ed25519::generate_secret_seed()?;
            std::fs::write(path, uniqnode::sha2::hex(&seed))?;
            let mut permissions = std::fs::metadata(path)?.permissions();
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o600);
            std::fs::set_permissions(path, permissions)?;
            println!(
                "public_key: {}",
                uniqnode::sha2::hex(&uniqnode::ed25519::public_key(&seed))
            );
        }
        "cert-make" | "revoke-make" => {
            let node_id = dir;
            let group_id = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let now = unix_now();
            let statement = if command == "cert-make" {
                let days: i64 = rest
                    .get(1)
                    .and_then(|d| d.parse().ok())
                    .filter(|d| (1..=3650).contains(d))
                    .ok_or_else(|| StoreError::Invalid("days は 1..=3650".into()))?;
                uniqnode::groups::make_statement(
                    "membership",
                    node_id,
                    group_id,
                    now,
                    Some(now + days * 86_400),
                )
            } else {
                uniqnode::groups::make_statement("revocation", node_id, group_id, now, None)
            };
            let bytes = uniqnode::c1::to_canonical_bytes(&statement);
            println!("{}", String::from_utf8(bytes).expect("c1 は UTF-8"));
        }
        "cert-sign" => {
            let seed_hex = std::fs::read_to_string(dir)?;
            let seed_bytes = uniqnode::sha2::from_hex(seed_hex.trim())
                .filter(|b| b.len() == 32)
                .ok_or_else(|| StoreError::Invalid("鍵ファイルが32バイトの16進でない".into()))?;
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&seed_bytes);
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let mut statement = uniqnode::c1::parse(&input)
                .map_err(|e| StoreError::Invalid(format!("入力が JSON でない: {e}")))?;
            uniqnode::groups::add_signature(&mut statement, &seed);
            let bytes = uniqnode::c1::to_canonical_bytes(&statement);
            println!("{}", String::from_utf8(bytes).expect("c1 は UTF-8"));
        }
        "cert-verify" => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let certificate = uniqnode::c1::parse(&input)
                .map_err(|e| StoreError::Invalid(format!("入力が JSON でない: {e}")))?;
            let groups = uniqnode::groups::read_groups(std::path::Path::new(dir));
            match uniqnode::groups::verify_membership(&certificate, &groups, unix_now()) {
                Ok(group_id) => println!("ok: group {group_id}"),
                Err(reason) => {
                    eprintln!("reject: {reason}");
                    std::process::exit(6);
                }
            }
        }
        "embed" => {
            let options = parse_embed_options(rest);
            // 読み手を serve と共有しているので --rerank も字面としては通る。この命令は
            // ベクトルを作るだけで順位の取り直しに口を持たないから、受けて捨てずに断る
            // (must/0022 の同型)。ストアを開く前に言う。
            if let Some(flag) = options.rerank_flag() {
                eprintln!(
                    "uniqnode: embed: {flag} は embed の引数ではない(順位の取り直しは検索の\
                     層のもので、装備するのは serve と、ストアを直接開く形の mcp である)"
                );
                std::process::exit(2);
            }
            let embedder = embedder_from(&options);
            let store = open(dir);
            let path = uniqnode::embed::VectorCache::path_for(
                std::path::Path::new(dir),
                embedder.embedder_id(),
            );
            let mut cache = match uniqnode::embed::VectorCache::open(
                path.clone(),
                embedder.embedder_id(),
                embedder.dimension(),
            ) {
                Ok(cache) => cache,
                Err(e) => {
                    eprintln!("uniqnode: {e}");
                    std::process::exit(5);
                }
            };
            if cache.discarded_tail_bytes() > 0 {
                eprintln!(
                    "uniqnode: {} の末尾 {} バイトを捨てた(追記の途中で止まった記録)",
                    path.display(),
                    cache.discarded_tail_bytes()
                );
            }
            println!("cache: {} ({} ベクトル)", path.display(), cache.vector_count());
            println!("model: {} ({})", embedder.embedder_id(), embedder.endpoint());
            let mut last = 0usize;
            let report = uniqnode::embed::fill_cache(&store, &embedder, &mut cache, &mut |progress| {
                // 25k チャンクは分単位かかる。どこまで進んだかを黙っていない。
                if progress.embedded >= last + 200 {
                    last = progress.embedded;
                    eprintln!(
                        "uniqnode: embedded {}/{}",
                        progress.embedded,
                        progress.distinct_chunks - progress.already_cached
                    );
                }
            });
            match report {
                Ok(report) => println!(
                    "chunks: {} (distinct {}), cached: {}, embedded: {}",
                    report.chunks,
                    report.distinct_chunks,
                    report.already_cached,
                    report.embedded
                ),
                Err(e) => {
                    eprintln!("uniqnode: {e}");
                    std::process::exit(5);
                }
            }
        }
        // MCP アダプタ(MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9))。serve と同じ
        // ApiContext を組み、HTTP の代わりに標準入出力の JSON-RPC で search と fetch を
        // 出す。ここで標準出力へ書いてよいのは MCP のメッセージだけなので、起動の知らせも
        // 含めてログはすべて標準エラーと <dir>/logs/mcp.log へ出す。標準エラーは登録した
        // LLM クライアントが吸って利用者に見せないので、ファイルが唯一読める記録になる。
        "mcp" => {
            let options = parse_mcp_options(rest);
            // 何よりも先に開く。ストアを開けない・転送先が誤っている、といった起動時の
            // 失敗こそ残したい記録である。
            start_logging(dir, uniqnode::log::MCP_ROLE, &options.run.log);
            let backend = match &options.serve_url {
                // 転送する形: 走っている serve の REST へ回す。ストアを開かない
                // (排他ロックを取らない)ので、常駐したまま ingest・embed が通る。
                Some(url) => {
                    // 埋め込みも順位の取り直しも、装備するのは転送先の serve である。
                    // ここで受けても効かせる先が無いので、黙って捨てずに断る
                    // (must/0022 の同型)。
                    if let Some(flag) = options.run.embed.misplaced_equipment_flag() {
                        uniqnode::log_line!(
                            "uniqnode: mcp: --serve-url と {flag} は併用しない\
                             (埋め込みと順位の取り直しを装備するのは転送先の serve である)"
                        );
                        std::process::exit(2);
                    }
                    match uniqnode::mcp::ServeClient::new(url, dir) {
                        Ok(client) => uniqnode::mcp::Backend::Forward(client),
                        Err(message) => {
                            uniqnode::log_line!("uniqnode: mcp: {message}");
                            std::process::exit(2);
                        }
                    }
                }
                // ストアを直接開く形: serve が走っていないストアに 1 人で向かうとき。
                None => {
                    let data_dir = std::path::PathBuf::from(dir);
                    let store = std::sync::Arc::new(std::sync::Mutex::new(open(dir)));
                    let engine = std::sync::Arc::new(uniqnode::query::QueryEngine::new(
                        store.clone(),
                        data_dir.clone(),
                    ));
                    // serve と同じく、埋め込みは明示されたときだけ装備する
                    // (should/0114)。届かなければ検索は BM25 に劣化し、ツールの応答と
                    // ログの両方がそう言う。
                    let embedding = options.run.embed.requested.then(|| {
                        let embedder = embedder_from(&options.run.embed)
                            .with_timeout(uniqnode::embed::QUERY_EMBED_TIMEOUT);
                        uniqnode::log_line!(
                            "uniqnode: mcp: embedding: {} ({})",
                            embedder.embedder_id(),
                            embedder.endpoint()
                        );
                        uniqnode::embed::EmbeddingService::new(&data_dir, embedder)
                    });
                    // 順位の取り直しも serve と同じ組み立てで装備する。--rerank を受けて
                    // おきながら None を置くと、運用者は装備したつもりで装備の無い検索を
                    // 読む(明示された指定の黙殺。must/0022 の同型)。
                    let reranker = reranker_from(&options.run.embed);
                    uniqnode::mcp::Backend::Local(Box::new(uniqnode::api::ApiContext {
                        store,
                        engine,
                        health: None,
                        referrers: std::sync::Mutex::new(None),
                        search: std::sync::Mutex::new(None),
                        embedding,
                        reranker,
                        data_dir,
                    }))
                }
            };
            let mut server = uniqnode::mcp::StdioServer::new(backend, options.writable);
            server.announce(dir);
            // 先読みバッファは自分で持つ。自己置換の前に「次のメッセージまで読んで
            // しまっていないか」を確かめられるのは、この BufReader だけだからである。
            // 容量を std::io::Stdin の内部バッファ(8KiB)より大きく採るのは、
            // BufReader が自分の buffer 以上の読みを内部バッファを迂回して行うため:
            // 見えない場所にバイトが溜まらない。
            let mut input = std::io::BufReader::with_capacity(64 * 1024, std::io::stdin());
            let stdout = std::io::stdout();
            // 終わりの理由もログに残す(標準エラーだけに出すと、登録した相手の中で消える)。
            if let Err(error) = server.serve(&mut input, &mut stdout.lock()) {
                uniqnode::log_line!("uniqnode: mcp: 標準入出力が壊れた: {error}");
                std::process::exit(1);
            }
        }
        "serve" => {
            let address = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let options = parse_run_options(&rest[1..]);
            // ストアを開くより先にログを開く。「開けない」「そのアドレスを使えない」も
            // 残したい記録である。
            start_logging(dir, uniqnode::log::SERVE_ROLE, &options.log);
            // 順序は「ストアを開く → 装備する → 束縛する → listening on」。標準出力の
            // この 1 行は起動スクリプトとの取り決めで、待つ側は「出たら要求を受け付ける」
            // と信じてよい。ここから束縛までの間で落ちるもの(ロックを持つ別プロセス、誤った
            // URL)は、何も束縛しないうちに理由を言って終わる。束縛してから開くと、待つ側
            // は騙され、その後で exit 1 する(2026-09-05 に systemd の据え付けで観測)。
            let data_dir = std::path::PathBuf::from(dir);
            let (capacity_bytes, health_params) = uniqnode::health::read_node_config(&data_dir);
            let mut store_config = uniqnode::store::StoreConfig::new(&data_dir);
            store_config.capacity_bytes = capacity_bytes;
            let store = match uniqnode::store::Store::open(store_config) {
                Ok(s) => std::sync::Arc::new(std::sync::Mutex::new(s)),
                Err(e) => {
                    uniqnode::log_line!("uniqnode: ストアを開けない: {e}");
                    std::process::exit(1);
                }
            };
            let engine = std::sync::Arc::new(uniqnode::query::QueryEngine::new(
                store.clone(),
                data_dir.clone(),
            ));
            let health = std::sync::Arc::new(uniqnode::health::HealthEngine::new(
                store.clone(),
                data_dir,
                health_params,
            ));
            {
                let health = health.clone();
                std::thread::spawn(move || health.run());
            }
            // 埋め込みは明示されたときだけ装備する。起動時に相手の生存を確かめない
            // (should/0114: 起動を外部プロセスの都合で止めない)。届くかどうかは
            // 検索要求のたびに分かり、届かなければ BM25 に劣化して答える。
            let embedding = options.embed.requested.then(|| {
                let embedder = embedder_from(&options.embed)
                    .with_timeout(uniqnode::embed::QUERY_EMBED_TIMEOUT);
                uniqnode::log_line!(
                    "uniqnode: embedding: {} ({})",
                    embedder.embedder_id(),
                    embedder.endpoint()
                );
                uniqnode::embed::EmbeddingService::new(std::path::Path::new(dir), embedder)
            });
            // 順位の取り直しも埋め込みと同じ扱い(明示されたときだけ・起動時に生存を
            // 確かめない)。組み立てはストアを直接開く形の mcp と共用する。
            let reranker = reranker_from(&options.embed);
            let listener = match std::net::TcpListener::bind(address) {
                Ok(listener) => listener,
                Err(error) => {
                    uniqnode::log_line!("uniqnode: serve: {address} に束縛できない: {error}");
                    std::process::exit(1);
                }
            };
            // テストや起動スクリプトが実際のポートを知れるように、束縛先を必ず表示する。
            let bound = listener.local_addr()?;
            println!("listening on {bound}");
            use std::io::Write as _;
            std::io::stdout().flush()?;
            // 標準出力の 1 行は起動スクリプトとの取り決めなので形を変えない。ログにも
            // 残すのは、後から「いつ、どのアドレスで起きたか」を読めるようにするため。
            uniqnode::log_line!("uniqnode: serve: {bound} で待ち受ける");
            let context = uniqnode::api::ApiContext {
                store,
                engine,
                health: Some(health),
                referrers: std::sync::Mutex::new(None),
                search: std::sync::Mutex::new(None),
                embedding,
                reranker,
                // ページの写しの作業ファイル置き場を組むために持つ(健全性エンジンへ
                // 渡した data_dir は移動済みなので、同じ dir から作り直す)。
                data_dir: std::path::PathBuf::from(dir),
            };
            let handler: std::sync::Arc<uniqnode::http::Handler> =
                std::sync::Arc::new(move |request| uniqnode::api::handle(&context, request));
            uniqnode::http::serve(listener, handler);
        }
        // RAG ビューワ(VIEWER (uuid:4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847))。1 枚の HTML を
        // 出し、その頁が呼ぶ /v1/* は走っている serve へ転送する。ストアを開かないので、
        // serve が常駐したまま起こせる。
        "viewer" => {
            let address = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let options = parse_mcp_options(&rest[1..]);
            start_logging(dir, uniqnode::log::VIEWER_ROLE, &options.run.log);
            // ビューワは書き込みの口を持たない。mcp と読み手を共有しているので字面は
            // 通るが、効かせる先の無い指定は黙って捨てずに断る(must/0022 の同型)。
            if !options.writable.is_empty() {
                uniqnode::log_line!(
                    "uniqnode: viewer: --writable はビューワの引数ではない(書き込みを許すのは \
                     mcp の指定である)"
                );
                std::process::exit(2);
            }
            if let Some(flag) = options.run.embed.misplaced_equipment_flag() {
                uniqnode::log_line!(
                    "uniqnode: viewer: {flag} はビューワの引数ではない\
                     (埋め込みと順位の取り直しを装備するのは転送先の serve である)"
                );
                std::process::exit(2);
            }
            let serve_url = options
                .serve_url
                .unwrap_or_else(|| uniqnode::viewer::DEFAULT_SERVE_URL.to_string());
            let viewer = match uniqnode::viewer::Viewer::new(&serve_url, dir) {
                Ok(viewer) => viewer,
                Err(message) => {
                    uniqnode::log_line!("uniqnode: viewer: {message}");
                    std::process::exit(2);
                }
            };
            // 束縛より先にログを開いてある。「そのアドレスを使えない」も残したい記録である。
            let listener = match std::net::TcpListener::bind(address) {
                Ok(listener) => listener,
                Err(error) => {
                    uniqnode::log_line!("uniqnode: viewer: {address} に束縛できない: {error}");
                    std::process::exit(1);
                }
            };
            let bound = listener.local_addr()?;
            // serve と同じ取り決め: 実際の束縛先を標準出力の 1 行で言う(:0 で起こした
            // テストや起動スクリプトが読む)。
            println!("listening on {bound}");
            std::io::stdout().flush()?;
            uniqnode::log_line!(
                "uniqnode: viewer: http://{bound} で待ち受ける(検索は {} へ転送する)",
                viewer.serve_url()
            );
            let handler: std::sync::Arc<uniqnode::http::Handler> =
                std::sync::Arc::new(move |request| viewer.handle(request));
            uniqnode::http::serve(listener, handler);
        }
        "pin" => {
            if rest.len() < 2 {
                usage();
            }
            let mut store = open(dir);
            let root = rest[0].as_str();
            let min_replicas: u32 = rest[1].parse().map_err(|_| {
                StoreError::Invalid("min_replicas は非負整数".into())
            })?;
            let seq = store.set_pin(root, min_replicas)?;
            if min_replicas > 0 && !store.own_attested_roots().contains(&root.to_string()) {
                store.set_attest(root, true)?;
            }
            println!("seq: {seq}");
        }
        // systemd に据える(SYSTEMD (uuid:7de68e4a-e6a6-4930-8cc7-a56f90f522e2))。手順ごとに
        // 1 行を出し、失敗は理由を言って 1 で終わる(途中まで置いたものはそのまま残る。
        // 再実行が更新になるので、直して同じ命令を打てばよい)。
        "install" => {
            let options = parse_install_options(dir, rest);
            if let Err(message) = uniqnode::install::run(options, &mut std::io::stdout()) {
                eprintln!("uniqnode: install: {message}");
                std::process::exit(1);
            }
        }
        "flood" => {
            // クラッシュ試験用: kill されるまで最速で書き続ける(fsync 済み書き込みの
            // 途中を kill -9 で裂き、回復経路を実プロセスで検証するため)。
            let mut store = open(dir);
            let mut counter: u64 = 0;
            loop {
                let body = format!("{{\"flood\":{counter},\"v\":1}}");
                let (id, _) = store.put_object(body.as_bytes())?;
                store.set_ref(&format!("flood/{counter}"), Some(&id))?;
                counter += 1;
            }
        }
        _ => usage(),
    }
    Ok(())
}

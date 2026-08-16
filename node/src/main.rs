//! uniqnode CLI。ストアの初期化・操作・検証(SPEC §10 の HTTP API はこの上に載せる)。

use std::io::{Read, Write};
use uniqnode::store::{Store, StoreConfig, StoreError};

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn usage() -> ! {
    eprintln!(
        "usage: uniqnode <command> <data_dir> [args]\n\
         commands:\n\
           init <dir>                 ストアを初期化し node id を表示する\n\
           status <dir>               件数・seq・node id を表示する\n\
           put <dir>                  標準入力をオブジェクトとして投入し id を表示する\n\
           get <dir> <id>             オブジェクトを標準出力へ書く\n\
           set-ref <dir> <path> <id>  自名前空間の ref を設定する(id が '-' なら tombstone)\n\
           refs <dir>                 ref を一覧する\n\
           fsck <dir>                 全再ハッシュ検査(異常があれば非0で終了)\n\
           pin <dir> <root> <min>     root の到達閉包に min_replicas を要求する(0 で解除)\n\
           admin-keygen <keyfile>     グループ管理者鍵を生成する(公開鍵を表示)\n\
           cert-make <node_id> <group_id> <days>\n\
                                      メンバーシップ証明書の本体を標準出力へ(署名なし)\n\
           cert-sign <keyfile>        標準入力の証明書/失効文に管理者署名を1つ追記する\n\
           cert-verify <dir>          標準入力の証明書を <dir>/groups.json で検証する\n\
           revoke-make <node_id> <group_id>\n\
                                      失効文の本体を標準出力へ(署名なし)\n\
           serve <dir> <addr> [--embed <url>] [--embedder <id>]\n\
                                      HTTP API を提供する(例: 127.0.0.1:7440、:0 で自動割当)。\n\
                                      --embed を与えると POST /v1/search の既定が BM25 と\n\
                                      埋め込みの RRF 融合になる。ベクトルは embed で作った\n\
                                      キャッシュから読むので、検索が模型の計算を待つことは\n\
                                      ない。届かなければ BM25 だけに劣化して答え、応答の\n\
                                      method と degraded がそれを言う\n\
           mcp <dir> [--serve-url <url>] [--embed <url>] [--embedder <id>]\n\
                                      標準入出力で MCP(Model Context Protocol)を話す。\n\
                                      LLM エージェント(Claude Code など)に search と\n\
                                      fetch の2ツールを出す。標準出力はプロトコル専用で、\n\
                                      ログは標準エラーへ出す。--serve-url を与えると、\n\
                                      自分でストアを開かず、走っている serve の REST へ\n\
                                      転送する(ストアの排他錠を取らないので、常駐した\n\
                                      まま ingest・embed が通る。埋め込みを装備するのは\n\
                                      転送先の serve なので --embed とは併用しない)。\n\
                                      無指定ならストアを直接開く(serve 停止中のストア用)。\n\
                                      登録例:\n\
                                      claude mcp add --transport stdio uniqnode --\n\
                                      <この実行ファイル> mcp <dir>\n\
                                      --serve-url http://127.0.0.1:7440\n\
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
                                      文書を取り込む(.md/.markdown/.txt/.pdf。ディレクトリ\n\
                                      は再帰。serve 停止中のストア用。serve 中は\n\
                                      PUT /v1/collections/{{c}}/documents/{{name}} を使う。\n\
                                      PDF の抽出は pdftotext に委譲し、--pdftotext の明示\n\
                                      指定が優先、無指定なら PATH を引く)\n\
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
           flood <dir>                書き込み続ける(クラッシュ試験用の内部コマンド)"
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
        let chunks = uniqnode::ingest::chunk_for_media(media, text);
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

fn open(dir: &str) -> Store {
    match Store::open(StoreConfig::new(dir)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("uniqnode: ストアを開けない: {e}");
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

/// 埋め込みの指定(serve と embed が共用する読み取り。should/0135)。
struct EmbedOptions {
    url: String,
    embedder_id: String,
    /// --embed が明示されたか。serve は明示されたときだけ埋め込みを装備する
    /// (既定で外部プロセスに依存させない)。
    requested: bool,
}

/// --embed <url> と --embedder <id> を読む。知らない引数は黙って捨てず usage で落とす。
fn parse_embed_options(rest: &[String]) -> EmbedOptions {
    let mut options = EmbedOptions {
        url: uniqnode::embed::DEFAULT_EMBEDDING_URL.to_string(),
        embedder_id: uniqnode::embed::DEFAULT_EMBEDDER_ID.to_string(),
        requested: false,
    };
    let mut at = 0;
    while at < rest.len() {
        let value = || rest.get(at + 1).cloned().unwrap_or_else(|| usage());
        match rest[at].as_str() {
            "--embed" => {
                options.url = value();
                options.requested = true;
            }
            "--embedder" => options.embedder_id = value(),
            _ => usage(),
        }
        at += 2;
    }
    options
}

/// mcp の指定(転送先の serve と、埋め込みの指定)。
struct McpOptions {
    /// 走っている serve へ転送する形の転送先(--serve-url)。無ければストアを直接開く。
    serve_url: Option<String>,
    embed: EmbedOptions,
}

/// --serve-url <url> だけを抜き取り、残りは serve と同じ読み手に渡す(埋め込みの指定の
/// 読み取りを二重に実装しない。should/0135)。
fn parse_mcp_options(rest: &[String]) -> McpOptions {
    let mut serve_url = None;
    let mut others = Vec::new();
    let mut at = 0;
    while at < rest.len() {
        if rest[at] == "--serve-url" {
            serve_url = Some(rest.get(at + 1).cloned().unwrap_or_else(|| usage()));
            at += 2;
            continue;
        }
        others.push(rest[at].clone());
        at += 1;
    }
    McpOptions { serve_url, embed: parse_embed_options(&others) }
}

/// 指定から埋め込みクライアントを組む(誤った指定はここで落とす)。
fn embedder_from(options: &EmbedOptions) -> uniqnode::embed::Embedder {
    match uniqnode::embed::Embedder::new(&options.url, &options.embedder_id) {
        Ok(embedder) => embedder,
        Err(message) => {
            eprintln!("uniqnode: {message}");
            std::process::exit(2);
        }
    }
}

fn run(command: &str, dir: &str, rest: &[String]) -> Result<(), StoreError> {
    match command {
        "init" | "status" => {
            let store = open(dir);
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
            let store = open(dir);
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
            let store = open(dir);
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
            let store = open(dir);
            let report = store.fsck()?;
            println!(
                "objects: {} refs: {} errors: {}",
                report.objects_checked,
                report.refs_checked,
                report.errors.len()
            );
            for error in &report.errors {
                eprintln!("fsck: {error}");
            }
            if !report.errors.is_empty() {
                std::process::exit(3);
            }
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
        // 含めてログはすべて標準エラーへ出す。
        "mcp" => {
            let options = parse_mcp_options(rest);
            let backend = match &options.serve_url {
                // 転送する形: 走っている serve の REST へ回す。ストアを開かない
                // (排他錠を取らない)ので、常駐したまま ingest・embed が通る。
                Some(url) => {
                    if options.embed.requested {
                        eprintln!(
                            "uniqnode: mcp: --serve-url と --embed は併用しない\
                             (埋め込みを装備するのは転送先の serve である)"
                        );
                        std::process::exit(2);
                    }
                    match uniqnode::mcp::ServeClient::new(url, dir) {
                        Ok(client) => uniqnode::mcp::Backend::Forward(client),
                        Err(message) => {
                            eprintln!("uniqnode: mcp: {message}");
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
                    // 標準エラーの両方がそう言う。
                    let embedding = options.embed.requested.then(|| {
                        let embedder = embedder_from(&options.embed)
                            .with_timeout(uniqnode::embed::QUERY_EMBED_TIMEOUT);
                        eprintln!(
                            "uniqnode: mcp: embedding: {} ({})",
                            embedder.embedder_id(),
                            embedder.endpoint()
                        );
                        uniqnode::embed::EmbeddingService::new(&data_dir, embedder)
                    });
                    uniqnode::mcp::Backend::Local(Box::new(uniqnode::api::ApiContext {
                        store,
                        engine,
                        health: None,
                        referrers: std::sync::Mutex::new(None),
                        search: std::sync::Mutex::new(None),
                        embedding,
                    }))
                }
            };
            let mut server = uniqnode::mcp::StdioServer::new(backend);
            server.announce(dir);
            // 先読みバッファは自分で持つ。自己置換の前に「次のメッセージまで読んで
            // しまっていないか」を確かめられるのは、この BufReader だけだからである。
            // 容量を std::io::Stdin の内部バッファ(8KiB)より大きく採るのは、
            // BufReader が自分の buffer 以上の読みを内部バッファを迂回して行うため:
            // 見えない場所にバイトが溜まらない。
            let mut input = std::io::BufReader::with_capacity(64 * 1024, std::io::stdin());
            let stdout = std::io::stdout();
            server.serve(&mut input, &mut stdout.lock())?;
        }
        "serve" => {
            let address = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let options = parse_embed_options(&rest[1..]);
            let listener = std::net::TcpListener::bind(address)?;
            // テストや起動スクリプトが実際のポートを知れるように、束縛先を必ず表示する。
            println!("listening on {}", listener.local_addr()?);
            use std::io::Write as _;
            std::io::stdout().flush()?;
            let data_dir = std::path::PathBuf::from(dir);
            let (capacity_bytes, health_params) = uniqnode::health::read_node_config(&data_dir);
            let mut store_config = uniqnode::store::StoreConfig::new(&data_dir);
            store_config.capacity_bytes = capacity_bytes;
            let store = match uniqnode::store::Store::open(store_config) {
                Ok(s) => std::sync::Arc::new(std::sync::Mutex::new(s)),
                Err(e) => {
                    eprintln!("uniqnode: ストアを開けない: {e}");
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
            let embedding = options.requested.then(|| {
                let embedder =
                    embedder_from(&options).with_timeout(uniqnode::embed::QUERY_EMBED_TIMEOUT);
                eprintln!(
                    "uniqnode: embedding: {} ({})",
                    embedder.embedder_id(),
                    embedder.endpoint()
                );
                uniqnode::embed::EmbeddingService::new(std::path::Path::new(dir), embedder)
            });
            let context = uniqnode::api::ApiContext {
                store,
                engine,
                health: Some(health),
                referrers: std::sync::Mutex::new(None),
                search: std::sync::Mutex::new(None),
                embedding,
            };
            let handler: std::sync::Arc<uniqnode::http::Handler> =
                std::sync::Arc::new(move |request| uniqnode::api::handle(&context, request));
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

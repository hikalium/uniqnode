//! uniqnode CLI。ストアの初期化・操作・検証(SPEC §10 の HTTP API はこの上に載せる)。

use std::io::{Read, Write};
use uniqnode::clock::unix_now;
use uniqnode::store::{Store, StoreConfig, StoreError};

fn usage() -> ! {
    eprintln!("{}", usage_text());
    std::process::exit(2);
}

/// 使い方の全文。断るときは usage が標準エラーへ出して 2 で終わり、求められたとき
/// (--help)は main が標準出力へ出して 0 で終わる。文面はこの 1 箇所だけにある
/// (should/0135)。
fn usage_text() -> String {
    format!(
        "usage: uniqnode <command> <data_dir> [args]\n\
         commands:\n\
           init <dir>                 initialize a store and print its node id\n\
           status <dir>               print counts, seq and node id (existing stores only)\n\
           put <dir>                  store standard input as an object and print its id\n\
           get <dir> <id>             write an object to standard output\n\
           set-ref <dir> <path> <id>  set a ref in our own namespace (id '-' writes a tombstone)\n\
           refs <dir>                 list the refs\n\
           fsck <dir>                 rehash everything (exits non-zero on a problem; opens only\n\
                                      an existing store and refuses a place that is not a store\n\
                                      instead of initializing it)\n\
           backup <dir> <backup_dir>  copy the sealed segments, MANIFEST, node_key and settings\n\
                                      to backup_dir (incremental: sealed segments already copied\n\
                                      are not copied again), then open the copy and run fsck on\n\
                                      it. Takes no lock, so it can run alongside serve (exits\n\
                                      non-zero on a problem)\n\
           gc <dir> [--dry-run] [--threshold <ratio>]\n\
                                      for each pack, count the live bytes and the orphan bytes\n\
                                      (objects no ref, pin or retention claim reaches), and\n\
                                      replace each sealed pack whose orphan ratio exceeds the\n\
                                      threshold (default {default_gc_threshold}) with a new pack\n\
                                      holding only the live objects, reclaiming the orphan bytes\n\
                                      from disk. --dry-run only counts and writes nothing to the\n\
                                      store's data (it only builds the reference table\n\
                                      derived/refs/). Takes the lock, so it cannot run on a store\n\
                                      that serve has open (for a running serve, send\n\
                                      POST /v1/admin/gc; docs/design/GC.md)\n\
           pin <dir> <root> <min>     require min_replicas for the closure reachable from root\n\
                                      (0 removes it)\n\
           admin-keygen <keyfile>     generate a group admin key (prints the public key)\n\
           cert-make <node_id> <group_id> <days>\n\
                                      write the body of a membership certificate to standard\n\
                                      output (unsigned)\n\
           cert-sign <keyfile>        append one admin signature to the certificate or\n\
                                      revocation on standard input\n\
           cert-verify <dir>          verify the certificate on standard input against\n\
                                      <dir>/groups.json\n\
           revoke-make <node_id> <group_id>\n\
                                      write the body of a revocation to standard output (unsigned)\n\
           serve <dir> <addr> [--embed <url>] [--embedder <id>] [--rerank <url>]\n\
                              [--reranker <id>] [--listen-agent <addr>]\n\
                              [--agent-writable <collection>]...\n\
                              [--agent-collections <collection>]...\n\
                              [--agent-graph <graph>]...\n\
                              [--agent-graph-writable <graph>]...\n\
                              [--main-allow-uid <uid,...>]... [--main-allow-host <host:port>]...\n\
                              [log options]\n\
                                      serve the HTTP API (for example 127.0.0.1:7440; :0 picks a\n\
                                      port). <addr> (the main door) accepts only a loopback IP\n\
                                      literal (127.0.0.0/8 or [::1]) and refuses names, 0.0.0.0,\n\
                                      [::] and IPv4-mapped addresses. The main door checks the\n\
                                      uid of the peer's socket (Linux sock_diag) and refuses it\n\
                                      with 403 unless it is in the allowed set (by default only\n\
                                      serve's own euid). --main-allow-uid replaces that set (to\n\
                                      allow root too, list it beside the service user, as in\n\
                                      1000,0). Browser requests are refused: 421 unless Host is\n\
                                      the bound address or localhost:<port> (add more with\n\
                                      --main-allow-host), 403 with Origin, and 415 for\n\
                                      text/plain and form types and for anything but\n\
                                      application/json on the JSON paths (docs/plan/API_AUTH.md).\n\
                                      --listen-agent binds a second door (the agent door) to that\n\
                                      address. The agent door passes only the allowed requests\n\
                                      (healthz, status, search, chunk objects, citation,\n\
                                      collections) and refuses the rest with 403. If it cannot\n\
                                      bind, the main door still comes up and the agent door\n\
                                      retries in the background (docs/design/AGENT_DOOR.md).\n\
                                      --agent-writable names a collection writable through the\n\
                                      agent door (repeatable; only with --listen-agent). Only\n\
                                      PUT /v1/collections/{{c}}/documents/{{name}} to an allowed\n\
                                      collection passes the agent door; other collections are\n\
                                      refused with 403 and the allowed list. The query\n\
                                      ?meta.<key>=<value> of a PUT goes into doc_rev.meta (the\n\
                                      main door does the same).\n\
                                      --agent-collections names a collection readable through\n\
                                      the agent door (repeatable; only with --listen-agent).\n\
                                      Without it every collection is readable. With it, search,\n\
                                      collections, objects and citation are limited to the set,\n\
                                      and anything outside is refused with 403.\n\
                                      --agent-graph names a graph readable through the agent\n\
                                      door, --agent-graph-writable a graph readable and writable\n\
                                      (both repeatable; only with --listen-agent). Unlike\n\
                                      collections, the graph layer does not appear on the agent\n\
                                      door unless one of them is given. Only\n\
                                      /v1/graphs/{{g}}/... of an allowed graph passes, and other\n\
                                      names are refused with 403 (docs/design/GRAPH.md).\n\
                                      --embed makes the default of POST /v1/search an RRF fusion\n\
                                      of BM25 and embeddings. Vectors are read from the cache, so\n\
                                      a search never waits for the model; missing ones are filled\n\
                                      in the background at startup and after writes (searches do\n\
                                      not wait for that). If the embedder cannot be reached, the\n\
                                      answer degrades to BM25 alone, and method and degraded in\n\
                                      the response say so.\n\
                                      --rerank reorders the top candidates with a reranker\n\
                                      (model name in --reranker, default {default_reranker}). If\n\
                                      it cannot be reached, the fused order is kept and degraded\n\
                                      says so.\n\
                                      By default the log is also kept in <dir>/logs/serve.log\n\
                                      (see below)\n\
           mcp <dir> [--serve-url <url>] [--writable <collection>]... [--embed <url>]\n\
                     [--embedder <id>] [--rerank <url>] [--reranker <id>] [log options]\n\
                                      speak MCP (Model Context Protocol) on standard input and\n\
                                      output. Offers an LLM agent (such as Claude Code) 2 tools\n\
                                      that read (search, fetch) and 2 tools that write to the\n\
                                      collections allowed with --writable (add_document: add\n\
                                      text knowledge; fetch_url: ingest a URL). --writable is\n\
                                      repeatable. Without it the server only reads and the write\n\
                                      tools are not listed. A write to a collection not allowed\n\
                                      is refused as a tool failure. Standard output is for the\n\
                                      protocol only; the log goes to standard error and to\n\
                                      <dir>/logs/mcp.log (the client that registered it swallows\n\
                                      standard error, so the file is the only readable record).\n\
                                      With --serve-url it does not open the store but forwards to\n\
                                      the REST API of a running serve (it takes no store lock, so\n\
                                      ingest and embed work while it stays running; embedding and\n\
                                      reranking belong to that serve, so --embed, --embedder,\n\
                                      --rerank and --reranker are not used with it). Without it\n\
                                      the store is opened directly (for a store with serve\n\
                                      stopped; --embed, --embedder, --rerank and --reranker mean\n\
                                      the same as for serve and are accepted only in this form).\n\
                                      Registration example:\n\
                                      claude mcp add --transport stdio uniqnode --\n\
                                      <this executable> mcp <dir>\n\
                                      --serve-url http://127.0.0.1:7440\n\
                                      --writable notes --writable web\n\
           viewer <dir> <addr> [--serve-url <url>] [log options]\n\
                                      serve the RAG viewer (a single HTML page) to a browser\n\
                                      (for example 127.0.0.1:7450; :0 picks a port). The /v1/*\n\
                                      calls of the page are forwarded to a running serve, so the\n\
                                      viewer does not open the store itself (it takes no store\n\
                                      lock, so it can start while serve stays running). The\n\
                                      default --serve-url is http://127.0.0.1:7440. By default\n\
                                      the log is also kept in <dir>/logs/viewer.log (see below)\n\
           selfcheck                  report on one line of standard output that this executable\n\
                                      is sound (used to start a new image as a child process and\n\
                                      check it before mcp replaces itself with exec)\n\
           embed <dir> [--embed <url>] [--embedder <id>]\n\
                                      embed the visible chunks that have no vector and add them\n\
                                      to <dir>/derived/embeddings/<id>.vec (derived data; it can\n\
                                      be rebuilt if deleted). Each batch is fsynced, so a stopped\n\
                                      run resumes where it left off. For a store with serve\n\
                                      stopped (while serve runs, serve fills them itself)\n\
           sync <dir> <peer_addr>     pull from a peer to synchronize (for a store with serve\n\
                                      stopped; while serve runs, use POST /v1/sync)\n\
           ingest <dir> <collection> <path> [--pdftotext <exe>]\n\
           ingest <dir> <collection> <path> --serve-url <url>\n\
                                      ingest documents (.md/.markdown/.txt/.html/.htm/.pdf;\n\
                                      directories are walked recursively; files left out are\n\
                                      listed, and a final line gives the counts). Without\n\
                                      --serve-url the store is opened directly (for a store with\n\
                                      serve stopped; PDF text is extracted by pdftotext, taken\n\
                                      from --pdftotext if given and from PATH otherwise).\n\
                                      With --serve-url the store is not opened (no lock is\n\
                                      taken) and each document is sent to a running serve with\n\
                                      PUT /v1/collections/{{c}}/documents/{{name}}\n\
                                      (for example --serve-url http://127.0.0.1:7440). Selection,\n\
                                      document names and idempotence are the same; if serve\n\
                                      returns an error, the reason is printed and ingest stops\n\
                                      there. serve extracts PDFs, so --pdftotext is not used\n\
                                      with it. Names containing whitespace, control characters\n\
                                      or ? cannot be sent, and they are refused before anything\n\
                                      is sent\n\
           ingest-git <dir> <collection> <tree> --paths <path,path,...> [--ref <ref>]\n\
                      [--serve-url <url> | --pdftotext <exe>]\n\
                                      write the files tracked at the ref (default main) of the\n\
                                      local git tree <tree> under --paths (comma separated,\n\
                                      relative to the root of the tree) to a temporary staging\n\
                                      directory, then ingest from there as ingest does (document\n\
                                      names are paths relative to the root of the tree). The\n\
                                      working directory is not read, so untracked files and\n\
                                      unfinished edits are not included. Symbolic links and\n\
                                      submodules are not written out and are listed instead.\n\
                                      Nothing is fetched from outside git (no fetch or clone).\n\
                                      If the tree or the ref is missing, or one of the paths is\n\
                                      not in that ref, nothing is sent and the command refuses.\n\
                                      Documents removed from git are not deleted\n\
           fetch <dir> <collection> <url> [--name <name>] [--pdftotext <exe>]\n\
                                      fetch a URL and ingest it (http/https only; curl fetches\n\
                                      it and its version is kept in doc_rev.meta.fetcher; up to\n\
                                      10 redirects, {fetch_max_seconds} seconds and\n\
                                      {fetch_max_bytes} bytes). HTML is made self-contained by\n\
                                      dropping external dependencies (scripts, images, fonts,\n\
                                      external styles) before ingesting; PDF goes through\n\
                                      pdftotext; plain text is ingested as is. Without --name\n\
                                      the document name is derived from the URL (host and path\n\
                                      as one word, without the extension), and fetching the same\n\
                                      URL again overwrites the same document. For a store with\n\
                                      serve stopped; while serve runs, use\n\
                                      POST /v1/collections/{{c}}/fetch\n\
           ingest-annotations <dir> <collection> <data.md> [--manual <approval list>]\n\
                                      ingest an annotation index (ingest the PDF first). Only\n\
                                      annotations whose title matches the page text go in as\n\
                                      annotates edges; mismatches are reported with their match\n\
                                      ratio. The approval list is lines of \"spec_id page-number\";\n\
                                      annotations listed there go in with a manual verification\n\
                                      record even if they fail matching. For a store with serve\n\
                                      stopped\n\
           correct <dir> <collection> <wrong-statement-id> <new-statement-id> <reason>\n\
                                      issue a corrects edge that corrects a statement in the\n\
                                      store (the new statement is re-matched with the same token\n\
                                      matching as ingest and gets a verification record with\n\
                                      the evidence lines; an entry for the correction is added\n\
                                      to the index ref annotations/<collection>. For a store\n\
                                      with serve stopped)\n\
           install <dir> [--instance <name>] [--listen <addr>] [--viewer-listen <addr>]\n\
                         [--serve-options \"<arguments>\"] [--backup-dir <dir>] [--bin <path>]\n\
                         [--unit-dir <dir>] [--no-start] [--listen-agent <addr>]\n\
                         [--agent-writable <collection>]...\n\
                         [--agent-collections <collection>]...\n\
                         [--agent-graph <graph>]... [--agent-graph-writable <graph>]...\n\
                         [--system [--user <name>] [--after <unit>]...\n\
                          [--take-over-user-units] [--retire-legacy-units]\n\
                          [--firewall-allow <addr>[,<addr>]...]]\n\
                                      deploy serve, viewer and a daily backup with systemd\n\
                                      (docs/mop/SYSTEMD.md). Copies the running executable to\n\
                                      --bin (default ~/.local/bin/uniqnode), writes 4 units and\n\
                                      3 drop-ins to --unit-dir (default ~/.config/systemd/user),\n\
                                      runs daemon-reload, enable and restart, and\n\
                                      loginctl enable-linger, then checks that /v1/status\n\
                                      through serve and through viewer returns the same node_id\n\
                                      and that one backup run leaves a copy (--backup-dir,\n\
                                      default ~/uniqnode-backup) that passes fsck. The default\n\
                                      --listen is {default_listen} and --viewer-listen is\n\
                                      {default_viewer_listen}. --serve-options gives extra serve\n\
                                      arguments (such as --embed) as one string. --no-start\n\
                                      stops after daemon-reload. Running it again updates (copy,\n\
                                      rewrite, restart). <dir> cannot be under /tmp (the units\n\
                                      use PrivateTmp).\n\
                                      --instance names one of several sets on the same machine\n\
                                      (default {default_instance}). The units are templates\n\
                                      named uniqnode-serve@<name>.service, and the default store,\n\
                                      backup and nft table are separate per name. Up to\n\
                                      {instance_max} lowercase ASCII letters, digits and _.\n\
                                      --listen-agent is serve's agent door (a second listener;\n\
                                      requests outside the allowed list get 403); it writes\n\
                                      UNIQNODE_AGENT_LISTEN to the drop-in and adds the checks\n\
                                      \"/v1/status on the agent door returns the same node_id\"\n\
                                      and \"POST /v1/admin/gc on the agent door gets 403\".\n\
                                      --agent-writable names a collection writable through the\n\
                                      agent door (repeatable; only with --listen-agent); it\n\
                                      writes UNIQNODE_AGENT_WRITABLE to the drop-in, appends\n\
                                      --agent-writable <c> to ExecStart=, and adds the check\n\
                                      \"a PUT to an allowed collection passes the gate and one to\n\
                                      any other gets 403\" (without writing anything).\n\
                                      --agent-collections names a collection readable through\n\
                                      the agent door (repeatable; only with --listen-agent); it\n\
                                      writes UNIQNODE_AGENT_COLLECTIONS to the drop-in, appends\n\
                                      --agent-collections <c> to ExecStart=, and adds the check\n\
                                      \"a search naming a collection not allowed gets 403\".\n\
                                      --agent-graph names a graph readable through the agent\n\
                                      door, --agent-graph-writable a graph readable and writable\n\
                                      (both repeatable; only with --listen-agent); they write\n\
                                      UNIQNODE_AGENT_GRAPH and UNIQNODE_AGENT_GRAPH_WRITABLE to\n\
                                      the drop-in, append to ExecStart=, and add the checks \"an\n\
                                      allowed graph is readable and other names get 403\" and \"a\n\
                                      PUT to an allowed graph passes the gate and one to any\n\
                                      other gets 403\" (without writing anything).\n\
                                      --system deploys system units (/etc/systemd/system): run\n\
                                      it as root (sudo); the units run as the --user account\n\
                                      (SUDO_USER if omitted; root is not allowed), and the\n\
                                      default places are under that account's home. backup and\n\
                                      fsck also run as that account, so nothing owned by root is\n\
                                      left in the store or the backup. --after names a unit for\n\
                                      After=/Wants= in the drop-in (such as\n\
                                      wg-quick@wg1.service; repeatable; only with --system). If\n\
                                      user units are already running, --take-over-user-units has\n\
                                      install stop and remove them (disable --now on the 3 user\n\
                                      units, then check that they stopped and released the\n\
                                      lock). If units with old names (such as\n\
                                      uniqnode-serve.service from before the templates) remain,\n\
                                      --retire-legacy-units has install move them aside and\n\
                                      remove them (/var/backups/uniqnode-legacy-<time>; default\n\
                                      instance only).\n\
                                      --firewall-allow <addr> installs a rule that lets only\n\
                                      <addr> reach the agent door (<addr> is an IPv4 address or\n\
                                      CIDR; list several with ,) and checks its effect (ufw\n\
                                      allow if ufw is active; otherwise serve's ExecStartPre=\n\
                                      installs the nft table inet uniqnode_<name> on every\n\
                                      start; only with --system and --listen-agent). See the\n\
                                      migration steps in SYSTEMD.md\n\
           autodeploy <checkout> --bin <path> [--state-dir <dir>] [--publish-branch <name>]\n\
                                      one run of automatic updates (\"automatic updates\" in\n\
                                      SYSTEMD.md; runs as the service account, without sudo).\n\
                                      Follows main of origin in <checkout>; for a new SHA it\n\
                                      rebuilds, replaces <path>, and watches until the running\n\
                                      serve and viewer switch to the new version by restarting\n\
                                      themselves. If they do not settle, it rolls back to the\n\
                                      previous version (<path>.prev). After deploying it pushes\n\
                                      the SHA to --publish-branch. State is kept in --state-dir\n\
                                      (default ~/.local/state/uniqnode-autodeploy); if a file\n\
                                      named hold is there, it does nothing\n\
           install-autodeploy <checkout> [--user <name>] [--bin <path>]\n\
                              [--publish-branch <name>] [--unit-dir <dir>] [--no-start]\n\
                                      deploy a system service and timer that run autodeploy\n\
                                      every minute (run as root; the account is --user or\n\
                                      SUDO_USER, and --bin defaults to ~/.local/bin/uniqnode)\n\
           apply <file> [--log-dir <dir>]\n\
                                      run each blank-line-separated block of the file as one\n\
                                      uniqnode command (one argument per line within a block),\n\
                                      stopping at the first failure. The output is also kept,\n\
                                      with timestamps, in uniqnode-apply-<time>-<pid>.log under\n\
                                      --log-dir (default /var/log) (docs/mop/vega/vega.apply)\n\
         \n\
         log options of serve, mcp and viewer (only the long-running commands have them; by\n\
         default the log is kept):\n\
           --log <path>               change where the log is kept (default\n\
                                      <dir>/logs/<serve|mcp|viewer>.log; if the default cannot\n\
                                      be written, fall back to $XDG_STATE_HOME/uniqnode/logs/,\n\
                                      or ~/.local/state/uniqnode/logs/ without it, and say so on\n\
                                      the first line; a path given with --log does not fall\n\
                                      back)\n\
           --no-log                   keep no file and write to standard error only\n\
           --log-max-bytes <n>        the limit of one generation (default {default_max_bytes});\n\
                                      beyond it the file moves to <path>.1 and a new generation\n\
                                      starts, keeping {generations} generations (older ones are\n\
                                      removed)\n\
         the same lines go to both standard error and the file; each line starts with the UTC\n\
         time and the pid",
        default_reranker = uniqnode::rerank::DEFAULT_RERANKER_ID,
        default_gc_threshold = uniqnode::gc::DEFAULT_THRESHOLD,
        fetch_max_seconds = uniqnode::fetch::DEFAULT_MAX_SECONDS,
        fetch_max_bytes = uniqnode::fetch::DEFAULT_MAX_BYTES,
        default_instance = uniqnode::install::DEFAULT_INSTANCE,
        instance_max = uniqnode::install::INSTANCE_MAX_CHARS,
        default_listen = uniqnode::install::DEFAULT_LISTEN,
        default_viewer_listen = uniqnode::install::DEFAULT_VIEWER_LISTEN,
        default_max_bytes = uniqnode::log::DEFAULT_MAX_BYTES,
        generations = uniqnode::log::RETAINED_GENERATIONS,
    )
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

/// 取り込み対象の 1 件。ストアを直接開く形と serve へ送る形が同じ選び方・同じ名付けを
/// 通るように、ファイルの選別はここ 1 箇所で決める(should/0135)。
struct IngestTarget {
    file: std::path::PathBuf,
    /// 文書名: 取り込み起点からの相対パスから拡張子を除き、/ で繋いだもの(ref 名の残り)。
    name: String,
    /// 元の拡張子(PUT documents の {name} に付け直す。serve は拡張子で種別を判定する)。
    extension: String,
    media: &'static str,
}

/// 取り込み起点を歩いた結果: 対象(名前順)と、拡張子で外れたもの(起点からの相対パス)。
struct IngestPlan {
    targets: Vec<IngestTarget>,
    skipped: Vec<String>,
}

/// 取り込み起点を再帰で歩き、対象の拡張子(media_for_extension の表)のファイルに文書名を
/// 付ける。起点がファイルならその親を起点とみなす。
fn plan_ingest(root: &str) -> Result<IngestPlan, StoreError> {
    let root_path = std::path::Path::new(root);
    if !root_path.exists() {
        return Err(StoreError::Invalid(format!("{root}: does not exist")));
    }
    let mut files = Vec::new();
    collect_files(root_path, &mut files)?;
    let base = if root_path.is_dir() {
        root_path
    } else {
        root_path.parent().unwrap_or_else(|| std::path::Path::new(""))
    };
    let mut plan = IngestPlan { targets: Vec::new(), skipped: Vec::new() };
    for file in files {
        let relative = file.strip_prefix(base).unwrap_or(&file);
        let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("");
        let Some(media) = uniqnode::ingest::media_for_extension(extension) else {
            plan.skipped.push(relative.display().to_string());
            continue;
        };
        let name = relative
            .with_extension("")
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let extension = extension.to_string();
        plan.targets.push(IngestTarget { file, name, extension, media });
    }
    Ok(plan)
}

/// 取り込みの数え上げ。1 件ごとの行の後に、両方の形が同じ締めの報告を出す。
#[derive(Default)]
struct IngestTally {
    updated: usize,
    unchanged: usize,
}

impl IngestTally {
    /// 1 件の結果を数え、行に置く状態の語を返す。
    fn count(&mut self, ref_updated: bool) -> &'static str {
        if ref_updated {
            self.updated += 1;
            "updated"
        } else {
            self.unchanged += 1;
            "no-op"
        }
    }

    /// 対象外の一覧と締めの 1 行。対象外は黙って捨てず、ここで必ず言う(must/0022 の同型)。
    fn report(&self, skipped: &[String]) {
        for path in skipped {
            println!("skipped (extension): {path}");
        }
        println!(
            "ingested: {} (updated {}, no-op {}), skipped {}",
            self.updated + self.unchanged,
            self.updated,
            self.unchanged,
            skipped.len()
        );
    }
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
    let plan = plan_ingest(root)?;
    let mut store = open(dir);
    let mut tally = IngestTally::default();
    let mut pdf_extractor: Option<uniqnode::ingest::PdfExtractor> = None;
    for target in &plan.targets {
        let (file, name, media) = (&target.file, &target.name, target.media);
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
            let extractor = pdf_extractor.as_ref().expect("ensured just above");
            extracted = extractor.extract(&bytes)?;
            &extracted
        } else {
            match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(_) => {
                    return Err(StoreError::Invalid(format!(
                        "{}: not UTF-8 (cannot be ingested as {media})",
                        file.display()
                    )));
                }
            }
        };
        // PDF は節見出しの経路も載せる(node/src/outline.rs)。取れなければ理由を出して、
        // 見出しの無いチャンクとして続ける。
        let (chunks, outline_reason) =
            uniqnode::ingest::chunk_for_media_with_source(media, text, &bytes);
        if let Some(reason) = outline_reason {
            uniqnode::log_line!("uniqnode: ingest: section headings of {name}: {reason}");
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
                name,
                source: &bytes,
                media,
                chunks: &chunks,
                extractor: extractor_label,
                extra_meta: &[],
            },
        )?;
        let state = tally.count(outcome.ref_updated);
        println!(
            "{collection}/{name}: {state} chunks={} new_objects={} doc_rev={}",
            chunks.len(),
            outcome.new_objects,
            outcome.doc_rev_id
        );
    }
    tally.report(&plan.skipped);
    Ok(())
}

/// 要求行に載らない字(空白・制御文字)と、query の始まりと読まれる ? を含むか。serve は
/// 道のパーセント符号を解かないので、こうした名前は serve に届く前に別の名前に変わる。
fn unsendable_in_path(text: &str) -> bool {
    text.chars().any(|c| c.is_whitespace() || c.is_control() || c == '?')
}

/// 転送する形の取り込み(`ingest … --serve-url`。INGEST の「CLI と API」節)。ストアを
/// 開かず(ロックは serve が持っている)、選んだ文書を 1 件ずつ走っている serve の
/// PUT /v1/collections/{c}/documents/{name} へ送る。選び方と文書名は直接開く形と同じ
/// plan_ingest、種別の判定・PDF の抽出・チャンク分け・同一内容の判定は serve 側の
/// put_document が持つ(同じ内容は同じ ID で、変わらない文書は no-op)。
///
/// serve が 2xx 以外を返したら、その理由を載せてそこで止まる(直接開く形が最初の誤りで
/// 止まるのと同じ。先に送った文書は入ったまま残る)。
fn run_ingest_via_serve(
    dir: &str,
    collection: &str,
    root: &str,
    serve_url: &str,
) -> Result<(), StoreError> {
    let client = uniqnode::mcp::ServeClient::new(serve_url, dir).map_err(StoreError::Invalid)?;
    let plan = plan_ingest(root)?;
    // 送れない名前は 1 件も送る前に全部を確かめる。名前を変えて入れることも、途中まで
    // 入れてから断ることもしない(must/0022)。
    if unsendable_in_path(collection) {
        return Err(StoreError::Invalid(format!(
            "collection name {collection:?} cannot be sent with --serve-url (it contains whitespace, control characters or ?)"
        )));
    }
    let refused: Vec<&str> = plan
        .targets
        .iter()
        .filter(|target| unsendable_in_path(&target.name))
        .map(|target| target.name.as_str())
        .collect();
    if !refused.is_empty() {
        return Err(StoreError::Invalid(format!(
            "document names containing whitespace, control characters or ? cannot be sent with --serve-url (nothing \
             was sent; rename them, or stop serve and ingest by opening the store directly): {refused:?}"
        )));
    }
    let mut tally = IngestTally::default();
    for target in &plan.targets {
        let bytes = std::fs::read(&target.file)?;
        let file_name = format!("{}.{}", target.name, target.extension);
        let fields = client
            .ingest_document(collection, &file_name, &bytes)
            .map_err(|reason| StoreError::Invalid(format!("{}: {reason}", target.file.display())))?;
        let ref_updated =
            matches!(fields.get("ref_updated"), Some(uniqnode::c1::Value::Bool(true)));
        let doc_rev = match fields.get("doc_rev") {
            Some(uniqnode::c1::Value::Text(id)) => id.as_str(),
            _ => "?",
        };
        let new_objects = match fields.get("new_objects") {
            Some(uniqnode::c1::Value::Integer(count)) => count.to_string(),
            _ => "?".to_string(),
        };
        let state = tally.count(ref_updated);
        println!(
            "{collection}/{}: {state} new_objects={new_objects} doc_rev={doc_rev}",
            target.name
        );
    }
    tally.report(&plan.skipped);
    Ok(())
}

/// git の木からの取り込み(`ingest-git`。INGEST の「git の木からの取り込み」節)。ref に
/// 追跡されている --paths の下のファイルを一時の置き場へ書き出し(node/src/ingest_git.rs)、
/// そこを起点に ingest と同じ 2 つの形のどちらかで取り込む。選び方・文書名・べき等は ingest
/// と同じ 1 箇所(plan_ingest)を通る。置き場は成功しても失敗しても消える。
fn run_ingest_git(
    dir: &str,
    collection: &str,
    tree: &str,
    reference: &str,
    paths: &[String],
    serve_url: Option<&str>,
    pdftotext: Option<&str>,
) -> Result<(), StoreError> {
    let staged = uniqnode::ingest_git::stage(
        std::path::Path::new(tree),
        reference,
        paths,
        &std::env::temp_dir(),
    )
    .map_err(StoreError::Invalid)?;
    println!(
        "tree: {reference} of {tree} = {} (written {}, left out {})",
        staged.commit,
        staged.written,
        staged.left_out.len()
    );
    for item in &staged.left_out {
        println!("not written ({item})");
    }
    let root = staged.dir.to_str().ok_or_else(|| {
        StoreError::Invalid(format!("the staging path is not UTF-8: {}", staged.dir.display()))
    })?;
    match serve_url {
        Some(url) => run_ingest_via_serve(dir, collection, root, url),
        None => run_ingest(dir, collection, root, pdftotext),
    }
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
        uniqnode::log_line!("uniqnode: ingest: section headings of {}: {reason}", document.name);
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
        .map_err(|error| StoreError::Invalid(format!("{data_md}: cannot read: {error}")))?;
    let entries = uniqnode::ingest::parse_annotation_index(&text)
        .map_err(|error| StoreError::Invalid(format!("{data_md}: {error}")))?;
    let approvals = match manual {
        None => Default::default(),
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| StoreError::Invalid(format!("{path}: cannot read: {error}")))?;
            uniqnode::ingest::parse_manual_approvals(&text)
                .map_err(|error| StoreError::Invalid(format!("{path}: {error}")))?
        }
    };
    let mut store = open(dir);
    let outcome =
        uniqnode::ingest::ingest_annotations(&mut store, collection, &entries, &approvals)?;
    for accepted in &outcome.accepted {
        println!(
            "accepted: {} p.{} {} (method={}, matched {}/{})",
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
            "mismatch: {} p.{} {} (matched {}/{})",
            rejected.spec_id,
            rejected.page,
            rejected.title,
            rejected.matched_tokens,
            rejected.total_tokens
        );
    }
    let state = if outcome.ref_updated { "updated" } else { "no-op" };
    println!(
        "annotations/{collection}: {state} accepted={} mismatched={} new_objects={} index={}",
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
            uniqnode::log_line!("uniqnode: cannot open the store: {e}");
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
    // install の起動の確認(system 単位で常駐の利用者へ権限を落とした子プロセスとして起こされ、
    // 主の口の /v1/status を引く。node/src/install.rs の status_node_id_as)。データ
    // ディレクトリを取らない。
    if arguments.len() == 3 && arguments[1] == uniqnode::install::STATUS_PROBE_COMMAND {
        match uniqnode::install::status_node_id(&arguments[2]) {
            Ok(node_id) => println!("{node_id}"),
            Err(message) => {
                eprintln!("uniqnode: {}: {message}", uniqnode::install::STATUS_PROBE_COMMAND);
                std::process::exit(1);
            }
        }
        return;
    }
    // ヘルプの求めは、どの位置にあっても使い方だけを出して 0 で終わる。位置引数の数を
    // 数えるより前に見るのは、`uniqnode install --help` が --help をデータディレクトリと
    // 読んでデプロイを実行してしまうからである(2026-09-08 に実際に起きた)。
    if arguments[1..].iter().any(|argument| argument == "--help" || argument == "-h") {
        println!("{}", usage_text());
        return;
    }
    if arguments.len() < 3 {
        usage();
    }
    let command = arguments[1].as_str();
    let dir = arguments[2].as_str();
    // データディレクトリの位置に指定の字面が来ていたら、走らせずに断る。ここを通すと、
    // install のように状態を作る命令が打ち間違いをそのまま実行する(must/0022: 黙って
    // 別のことをしない)。`-` で始まる道を本当に指したいときは ./-dir と書く。
    if dir.starts_with('-') {
        eprintln!(
            "uniqnode: {command}: {dir} is in the place of the data directory \
             (the order is <command> <data_dir> [args]); see uniqnode --help"
        );
        std::process::exit(2);
    }
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
                        eprintln!("uniqnode: --threshold must be a ratio from 0 to 1: {text}");
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
    /// 読み口の束縛先(--listen-agent)。serve だけが持つ口で、無指定なら第 2 の口は
    /// 存在しない(docs/design/AGENT_DOOR.md)。mcp・viewer は読み手を共有しているので
    /// 字面は通るが、効かせる先が無いので断る。
    listen_agent: Option<String>,
    /// 読み口から書けるコレクション(--agent-writable。繰り返せる。与えられた順)。
    /// --listen-agent があるときだけ意味を持ち、無いのに与えられたら断る(黙って捨てると
    /// 「許したつもり」が残る。must/0022)。
    agent_writable: Vec<String>,
    /// 読み口から読めるコレクション(--agent-collections。繰り返せる。与えられた順)。
    /// 空なら読み口は全コレクションを読める(既定)。--agent-writable と同じく、
    /// --listen-agent が無いのに与えられたら断る。
    agent_collections: Vec<String>,
    /// 読み口から読めるグラフ(--agent-graph)と書けるグラフ(--agent-graph-writable)。
    /// どちらも空ならグラフ層は読み口から見えない(コレクションと既定が逆。
    /// docs/design/GRAPH.md)。
    agent_graph: Vec<String>,
    agent_graph_writable: Vec<String>,
    /// 主の口が許す uid の集合(--main-allow-uid。繰り返せて、与えた全部の和)。None なら
    /// serve 自身の euid だけ(docs/plan/API_AUTH.md の 1)。serve だけが持つ。
    main_allow_uid: Option<Vec<u32>>,
    /// 主の口が既定に足して許す Host(--main-allow-host。繰り返せる)。serve だけが持つ。
    main_allow_host: Vec<String>,
}

/// ログの指定と読み口の指定だけを抜き取り、残りは埋め込みの読み手に渡す(読み取りを
/// 二重に実装しない。should/0135)。
fn parse_run_options(rest: &[String]) -> RunOptions {
    let mut log = LogOptions {
        path: None,
        enabled: true,
        max_bytes: uniqnode::log::DEFAULT_MAX_BYTES,
    };
    let mut listen_agent = None;
    let mut agent_writable: Vec<String> = Vec::new();
    let mut agent_collections: Vec<String> = Vec::new();
    let mut agent_graph: Vec<String> = Vec::new();
    let mut agent_graph_writable: Vec<String> = Vec::new();
    let mut main_allow_uid: Option<Vec<u32>> = None;
    let mut main_allow_host: Vec<String> = Vec::new();
    let mut others = Vec::new();
    let mut at = 0;
    while at < rest.len() {
        let value = || rest.get(at + 1).cloned().unwrap_or_else(|| usage());
        match rest[at].as_str() {
            "--log" => {
                log.path = Some(value());
                at += 2;
            }
            uniqnode::install::AGENT_LISTEN_FLAG => {
                listen_agent = Some(value());
                at += 2;
            }
            uniqnode::install::AGENT_WRITABLE_FLAG => {
                let collection = value();
                // 名前の検査は install と同じ 1 箇所(should/0135)。mcp の --writable と同じく
                // 同じ名前の繰り返しは 1 つに畳む。
                if let Err(message) = uniqnode::install::check_agent_writable(&collection) {
                    eprintln!("uniqnode: {message}");
                    std::process::exit(2);
                }
                if !agent_writable.contains(&collection) {
                    agent_writable.push(collection);
                }
                at += 2;
            }
            uniqnode::install::AGENT_COLLECTIONS_FLAG => {
                // 書く集合と全く同じ読み方(名前の検査は install と同じ 1 箇所。
                // should/0135)。同じ名前の繰り返しは 1 つに畳む。
                let collection = value();
                if let Err(message) = uniqnode::install::check_agent_collection(&collection) {
                    eprintln!("uniqnode: {message}");
                    std::process::exit(2);
                }
                if !agent_collections.contains(&collection) {
                    agent_collections.push(collection);
                }
                at += 2;
            }
            uniqnode::install::AGENT_GRAPH_FLAG | uniqnode::install::AGENT_GRAPH_WRITABLE_FLAG => {
                // グラフ名の検査は install と同じ 1 箇所(should/0135)。同じ名の繰り返しは
                // 1 つに畳む。
                let flag = rest[at].clone();
                let graph = value();
                if let Err(message) = uniqnode::install::check_agent_graph(&flag, &graph) {
                    eprintln!("uniqnode: {message}");
                    std::process::exit(2);
                }
                let names = match flag == uniqnode::install::AGENT_GRAPH_FLAG {
                    true => &mut agent_graph,
                    false => &mut agent_graph_writable,
                };
                if !names.contains(&graph) {
                    names.push(graph);
                }
                at += 2;
            }
            uniqnode::main_door::ALLOW_UID_FLAG => {
                let text = value();
                let uids = match uniqnode::main_door::parse_uid_list(&text) {
                    Ok(uids) => uids,
                    Err(message) => {
                        eprintln!("uniqnode: {message}");
                        std::process::exit(2);
                    }
                };
                let set = main_allow_uid.get_or_insert_with(Vec::new);
                for uid in uids {
                    if !set.contains(&uid) {
                        set.push(uid);
                    }
                }
                at += 2;
            }
            uniqnode::main_door::ALLOW_HOST_FLAG => {
                let host = value();
                if let Err(message) = uniqnode::main_door::check_allow_host(&host) {
                    eprintln!("uniqnode: {message}");
                    std::process::exit(2);
                }
                if !main_allow_host.contains(&host) {
                    main_allow_host.push(host);
                }
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
                        eprintln!("uniqnode: --log-max-bytes must be a positive integer: {text}");
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
    if !agent_writable.is_empty() && listen_agent.is_none() {
        // 書く許可は読み口に掛かるもの。読み口が無ければ効かせる先が無いので、黙って捨てずに
        // 断る(must/0022。install の normalize と同じ判断)。
        eprintln!(
            "uniqnode: {} {} is accepted only with {} (write permission applies to the agent door, \
             and without the agent door there is nothing to apply it to)",
            uniqnode::install::AGENT_WRITABLE_FLAG,
            agent_writable.join(" "),
            uniqnode::install::AGENT_LISTEN_FLAG
        );
        std::process::exit(2);
    }
    if !agent_collections.is_empty() && listen_agent.is_none() {
        // 読む許可も読み口に掛かるもの(書く許可と同じ理由で、黙って捨てない)。
        eprintln!(
            "uniqnode: {} {} is accepted only with {} (read permission applies to the agent door, \
             and without the agent door there is nothing to apply it to)",
            uniqnode::install::AGENT_COLLECTIONS_FLAG,
            agent_collections.join(" "),
            uniqnode::install::AGENT_LISTEN_FLAG
        );
        std::process::exit(2);
    }
    for (flag, names) in [
        (uniqnode::install::AGENT_GRAPH_FLAG, &agent_graph),
        (
            uniqnode::install::AGENT_GRAPH_WRITABLE_FLAG,
            &agent_graph_writable,
        ),
    ] {
        if !names.is_empty() && listen_agent.is_none() {
            // グラフの許可も読み口に掛かるもの(コレクションの 2 つと同じ理由)。
            eprintln!(
                "uniqnode: {flag} {} is accepted only with {} (graph permission applies to the agent door, \
                 and without the agent door there is nothing to apply it to)",
                names.join(" "),
                uniqnode::install::AGENT_LISTEN_FLAG
            );
            std::process::exit(2);
        }
    }
    RunOptions {
        embed: parse_embed_options(&others),
        log,
        listen_agent,
        agent_writable,
        agent_collections,
        agent_graph,
        agent_graph_writable,
        main_allow_uid,
        main_allow_host,
    }
}

/// 読み口を持たない命令(mcp・viewer)が --listen-agent を受け取ったとき、黙って捨てずに
/// 断る(must/0022 の同型。viewer の --writable と同じ扱い)。--agent-writable だけを受け
/// 取ったときは parse_run_options が「--listen-agent があるときだけ」で先に断るので、
/// ここに来る RunOptions の agent_writable は listen_agent 無しには空である。
fn refuse_misplaced_listen_agent(role: &str, options: &RunOptions) {
    if options.listen_agent.is_some() {
        uniqnode::log_line!(
            "uniqnode: {role}: --listen-agent is not an argument of {role} (the agent door is bound \
             by serve)"
        );
        std::process::exit(2);
    }
    // 主の口の指定も serve だけのもの(同じ理由で、黙って捨てない)。
    let main_flag = match (options.main_allow_uid.is_some(), options.main_allow_host.is_empty()) {
        (true, _) => Some(uniqnode::main_door::ALLOW_UID_FLAG),
        (false, false) => Some(uniqnode::main_door::ALLOW_HOST_FLAG),
        (false, true) => None,
    };
    if let Some(flag) = main_flag {
        uniqnode::log_line!(
            "uniqnode: {role}: {flag} is not an argument of {role} (the main door belongs to serve)"
        );
        std::process::exit(2);
    }
}

/// ストアを開く常駐(serve と mcp の Local)が、ログを開く前に呼ぶ: データのディレクトリが
/// 無ければ `mkdir` だけで作り、親が無ければ何も作らずに 1 で終わる。理由は標準エラー
/// (system の unit なら journal)にだけ言う(ログの置き場がまだ無いため)。書く CLI は
/// ログを開かないので、Store::open の中の同じ検めで足りる。
///
/// ログをデータのディレクトリの中(既定の `<dir>/logs`)に置くときだけ、ここで作る。
/// `--log` で外へ置くときと `--no-log` のときは、ログがデータのディレクトリに触れないので、
/// 検めは Store::open に任せる(起動時の引数の検査で断る serve はデータのディレクトリに
/// 触れない、という API_AUTH の 1 の約束を保つ)。
fn prepare_data_dir_or_exit(dir: &str, role: &str, log: &LogOptions) {
    if !log.enabled || log.path.is_some() {
        return;
    }
    if let Err(error) = uniqnode::store::prepare_data_dir(std::path::Path::new(dir)) {
        eprintln!("uniqnode: {role}: cannot open the store: {error}");
        std::process::exit(1);
    }
}

/// ログの保存を始める(既定で有効)。既定の道が開けなければ利用者の書ける場所へ倒し、
/// --log で明示された道が開けなければ倒さずに理由を言って標準エラーだけで続ける(どちらの
/// 判断も uniqnode::log にある。should/0135)。黙って落とさない(must/0022)が、ログを
/// 書けないことは serve や mcp を止める理由にはしない: 提供できる仕事があるのに、記録の
/// 都合で断る方が損である(should/0114)。
fn start_logging(dir: &str, role: &str, options: &LogOptions) {
    if !options.enabled {
        eprintln!("uniqnode: {role}: --no-log given, so the log is not kept in a file (standard error only)");
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
            "uniqnode: {role}: logging to {} ({} bytes per generation, keeping up to {} generations)",
            path.display(),
            options.max_bytes,
            uniqnode::log::RETAINED_GENERATIONS
        ),
        Err(message) => eprintln!(
            "uniqnode: {role}: cannot keep the log in a file ({message}); \
             continuing with standard error only (--log can name another path)"
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
                        "uniqnode: --writable takes a collection name (one non-empty word without /): \
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

/// `--flag 値` の対だけを受ける命令(autodeploy・install-autodeploy・apply)の引数を読む。
/// 知らない旗と値の欠けは usage で断る。値を取らない旗は switches に挙げる。
fn parse_flag_pairs(
    rest: &[String],
    flags: &[&str],
    switches: &[&str],
) -> std::collections::BTreeMap<String, String> {
    let mut found = std::collections::BTreeMap::new();
    let mut at = 0;
    while at < rest.len() {
        let flag = rest[at].as_str();
        if switches.contains(&flag) {
            found.insert(flag.to_string(), String::new());
            at += 1;
            continue;
        }
        if !flags.contains(&flag) {
            usage();
        }
        let value = rest.get(at + 1).cloned().unwrap_or_else(|| usage());
        found.insert(flag.to_string(), value);
        at += 2;
    }
    found
}

fn parse_autodeploy_options(dir: &str, rest: &[String]) -> uniqnode::autodeploy::Options {
    let found = parse_flag_pairs(rest, &["--bin", "--state-dir", "--publish-branch"], &[]);
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    let state_dir = match (found.get("--state-dir"), &home) {
        (Some(dir), _) => std::path::PathBuf::from(dir),
        (None, Some(home)) => std::path::Path::new(home).join(uniqnode::autodeploy::STATE_SUBDIR),
        (None, None) => {
            eprintln!("uniqnode: autodeploy: HOME is not set, so the default --state-dir cannot be decided");
            std::process::exit(2);
        }
    };
    let binary = match found.get("--bin") {
        Some(binary) => std::path::PathBuf::from(binary),
        None => {
            eprintln!("uniqnode: autodeploy: name the binary to replace with --bin");
            std::process::exit(2);
        }
    };
    uniqnode::autodeploy::Options {
        repo: std::path::PathBuf::from(dir),
        binary,
        state_dir,
        publish_branch: found.get("--publish-branch").cloned(),
    }
}

fn parse_install_autodeploy_options(dir: &str, rest: &[String]) -> uniqnode::autodeploy::InstallOptions {
    let found = parse_flag_pairs(
        rest,
        &["--user", "--bin", "--publish-branch", "--unit-dir"],
        &["--no-start"],
    );
    let fail = |message: String| -> ! {
        eprintln!("uniqnode: install-autodeploy: {message}");
        std::process::exit(1);
    };
    let euid = uniqnode::install::effective_uid().unwrap_or_else(|e| fail(e));
    // デプロイ先と利用者の決め方は install --system と同じ(root で走らせ、--user か SUDO_USER)。
    let scope = uniqnode::install::scope(
        true,
        found.get("--user").cloned(),
        std::env::var("SUDO_USER").ok(),
        euid,
    )
    .unwrap_or_else(|e| fail(e));
    let account = scope.account().cloned().unwrap_or_else(|| fail("the account cannot be determined".to_string()));
    let binary = found
        .get("--bin")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| account.home.join(".local").join("bin").join("uniqnode"));
    uniqnode::autodeploy::InstallOptions {
        repo: std::path::PathBuf::from(dir),
        binary,
        publish_branch: found.get("--publish-branch").cloned(),
        unit_dir: found
            .get("--unit-dir")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(uniqnode::install::SYSTEM_UNIT_DIR)),
        start: !found.contains_key("--no-start"),
        account,
    }
}

/// install の指定。既定は home の下(node/src/install.rs の Options::defaults。--system なら
/// 実行ユーザの home)。知らない引数は黙って捨てず usage で落とす。デプロイ先(user/system と
/// 実行ユーザ)の判断は install::scope にあり、ここは環境(HOME・SUDO_USER・実効 uid)を
/// 読んで渡すだけ。
fn parse_install_options(dir: &str, rest: &[String]) -> uniqnode::install::Options {
    // 1 巡目: 引数を集める(既定はデプロイ先が決まってから組む)。
    let mut system = false;
    let mut instance: Option<String> = None;
    let mut user: Option<String> = None;
    let mut after: Vec<String> = Vec::new();
    let mut agent_listen: Option<String> = None;
    let mut agent_writable: Vec<String> = Vec::new();
    let mut agent_collections: Vec<String> = Vec::new();
    let mut agent_graph: Vec<String> = Vec::new();
    let mut agent_graph_writable: Vec<String> = Vec::new();
    let mut take_over_user_units = false;
    let mut retire_legacy_units = false;
    let mut firewall_allow: Option<String> = None;
    let mut listen: Option<String> = None;
    let mut viewer_listen: Option<String> = None;
    let mut serve_options: Option<String> = None;
    let mut backup_dir: Option<std::path::PathBuf> = None;
    let mut binary: Option<std::path::PathBuf> = None;
    let mut unit_dir: Option<std::path::PathBuf> = None;
    let mut start = true;
    let mut at = 0;
    while at < rest.len() {
        let value = || rest.get(at + 1).cloned().unwrap_or_else(|| usage());
        match rest[at].as_str() {
            "--listen" => listen = Some(value()),
            "--viewer-listen" => viewer_listen = Some(value()),
            "--serve-options" => serve_options = Some(value()),
            "--backup-dir" => backup_dir = Some(std::path::PathBuf::from(value())),
            "--bin" => binary = Some(std::path::PathBuf::from(value())),
            "--unit-dir" => unit_dir = Some(std::path::PathBuf::from(value())),
            "--user" => user = Some(value()),
            uniqnode::install::INSTANCE_FLAG => instance = Some(value()),
            "--after" => after.push(value()),
            uniqnode::install::AGENT_LISTEN_FLAG => agent_listen = Some(value()),
            uniqnode::install::AGENT_WRITABLE_FLAG => {
                // 名前の検査と「--listen-agent が要る」は install::normalize が言う。ここは
                // 集めるだけ(同じ名前の繰り返しは 1 つに畳む)。
                let collection = value();
                if !agent_writable.contains(&collection) {
                    agent_writable.push(collection);
                }
            }
            uniqnode::install::AGENT_COLLECTIONS_FLAG => {
                // 書く集合と同じ扱い(検査は install::normalize)。
                let collection = value();
                if !agent_collections.contains(&collection) {
                    agent_collections.push(collection);
                }
            }
            uniqnode::install::AGENT_GRAPH_FLAG => {
                // 検査は install::normalize(判断の家は 1 つ。should/0135)。
                let graph = value();
                if !agent_graph.contains(&graph) {
                    agent_graph.push(graph);
                }
            }
            uniqnode::install::AGENT_GRAPH_WRITABLE_FLAG => {
                let graph = value();
                if !agent_graph_writable.contains(&graph) {
                    agent_graph_writable.push(graph);
                }
            }
            uniqnode::install::FIREWALL_ALLOW_FLAG => firewall_allow = Some(value()),
            uniqnode::install::TAKE_OVER_FLAG => {
                take_over_user_units = true;
                at += 1;
                continue;
            }
            uniqnode::install::RETIRE_LEGACY_FLAG => {
                retire_legacy_units = true;
                at += 1;
                continue;
            }
            "--no-start" => {
                start = false;
                at += 1;
                continue;
            }
            "--system" => {
                system = true;
                at += 1;
                continue;
            }
            _ => usage(),
        }
        at += 2;
    }
    let fail = |message: String| -> ! {
        eprintln!("uniqnode: install: {message}");
        std::process::exit(1);
    };
    let euid = uniqnode::install::effective_uid().unwrap_or_else(|e| fail(e));
    let scope = uniqnode::install::scope(
        system,
        user,
        std::env::var("SUDO_USER").ok(),
        euid,
    )
    .unwrap_or_else(|e| fail(e));
    // 既定の置き場の根。system 単位なら実行ユーザの home(sudo は HOME を root のものに
    // 替えるので、HOME はデプロイする相手の home ではない)。
    let home = match scope.account() {
        Some(account) => account.home.clone(),
        None => match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => std::path::PathBuf::from(home),
            _ => {
                eprintln!("uniqnode: install: HOME is not set, so the default places cannot be decided");
                std::process::exit(2);
            }
        },
    };
    let mut options =
        uniqnode::install::Options::defaults(std::path::PathBuf::from(dir), &home, scope);
    if let Some(listen) = listen {
        options.listen = listen;
    }
    if let Some(viewer_listen) = viewer_listen {
        options.viewer_listen = viewer_listen;
    }
    if let Some(serve_options) = serve_options {
        options.serve_options = serve_options;
    }
    if let Some(binary) = binary {
        options.binary = binary;
    }
    if let Some(unit_dir) = unit_dir {
        options.unit_dir = unit_dir;
    }
    if let Some(instance) = instance {
        // 名の検査は install::normalize が言う(判断の家は 1 つ。should/0135)。写し先の
        // 既定も名ごとに分かれるので、--backup-dir の上書きより先に置く。
        options.set_instance(instance);
    }
    if let Some(backup_dir) = backup_dir {
        options.backup_dir = backup_dir;
    }
    options.start = start;
    options.after = after;
    options.agent_listen = agent_listen;
    options.agent_writable = agent_writable;
    options.agent_collections = agent_collections;
    options.agent_graph = agent_graph;
    options.agent_graph_writable = agent_graph_writable;
    options.take_over_user_units = take_over_user_units;
    options.retire_legacy_units = retire_legacy_units;
    options.firewall_allow = firewall_allow;
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
            let mut pdftotext = None;
            let mut serve_url = None;
            let mut at = 2;
            while at < rest.len() {
                let value = rest.get(at + 1).map(String::as_str).unwrap_or_else(|| usage());
                match rest[at].as_str() {
                    "--pdftotext" => pdftotext = Some(value),
                    "--serve-url" => serve_url = Some(value),
                    // 知らない引数は黙って捨てず usage で落とす。
                    _ => usage(),
                }
                at += 2;
            }
            match serve_url {
                // 転送する形: ストアを開かず、走っている serve へ PUT する。
                Some(url) => {
                    // PDF を抽出するのは転送先の serve(serve が PATH から引く pdftotext)
                    // で、ここで受けても効かせる先が無い。黙って捨てずに断る(must/0022 の
                    // 同型。mcp の --serve-url と --embed の扱いと同じ)。
                    if pdftotext.is_some() {
                        eprintln!(
                            "uniqnode: ingest: --serve-url and --pdftotext are not used together \
                             (the serve forwarded to extracts PDFs and finds pdftotext on its PATH)"
                        );
                        std::process::exit(2);
                    }
                    run_ingest_via_serve(dir, collection, root, url)?;
                }
                None => run_ingest(dir, collection, root, pdftotext)?,
            }
        }
        "ingest-git" => {
            let collection = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let tree = rest.get(1).map(String::as_str).unwrap_or_else(|| usage());
            let mut reference = "main";
            let mut paths: Vec<String> = Vec::new();
            let mut pdftotext = None;
            let mut serve_url = None;
            let mut at = 2;
            while at < rest.len() {
                let value = rest.get(at + 1).map(String::as_str).unwrap_or_else(|| usage());
                match rest[at].as_str() {
                    "--ref" => reference = value,
                    // 繰り返してよい(並びは足し合わせる)。
                    "--paths" => paths.extend(
                        uniqnode::ingest_git::parse_paths(value).map_err(StoreError::Invalid)?,
                    ),
                    "--pdftotext" => pdftotext = Some(value),
                    "--serve-url" => serve_url = Some(value),
                    _ => usage(),
                }
                at += 2;
            }
            if paths.is_empty() {
                // 既定の道の一覧は持たない(どの木の何を入れるかは呼び手の決めること)。
                eprintln!(
                    "uniqnode: ingest-git: give the paths to ingest with --paths (for example --paths DESIGN.md,docs/design)"
                );
                std::process::exit(2);
            }
            if serve_url.is_some() && pdftotext.is_some() {
                eprintln!(
                    "uniqnode: ingest-git: --serve-url and --pdftotext are not used together \
                     (the serve forwarded to extracts PDFs and finds pdftotext on its PATH)"
                );
                std::process::exit(2);
            }
            run_ingest_git(dir, collection, tree, reference, &paths, serve_url, pdftotext)?;
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
                "corrects: {} (re-matched {}/{}, verification={})",
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
                    "unlisted packs (neither in MANIFEST nor the last one; a fact, not an error): {}",
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
                println!("only in backup {name} (not in the source; not deleted)");
            }
            for name in &report.packs_removed {
                println!("removed {name} (not in source MANIFEST)");
            }
            for name in &report.segments_only_in_backup {
                println!("only in backup {name} (not in the source; not deleted)");
            }
            for (name, bytes) in &report.torn_tails_cut {
                println!("cut {name} ({bytes} bytes: a partly written tail truncated by verification)");
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
                return Err(StoreError::Invalid(format!("{dir} already exists")));
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
                    .ok_or_else(|| StoreError::Invalid("days must be in 1..=3650".into()))?;
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
            println!("{}", String::from_utf8(bytes).expect("c1 is UTF-8"));
        }
        "cert-sign" => {
            let seed_hex = std::fs::read_to_string(dir)?;
            let seed_bytes = uniqnode::sha2::from_hex(seed_hex.trim())
                .filter(|b| b.len() == 32)
                .ok_or_else(|| StoreError::Invalid("the key file is not 32 bytes of hex".into()))?;
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&seed_bytes);
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let mut statement = uniqnode::c1::parse(&input)
                .map_err(|e| StoreError::Invalid(format!("the input is not JSON: {e}")))?;
            uniqnode::groups::add_signature(&mut statement, &seed);
            let bytes = uniqnode::c1::to_canonical_bytes(&statement);
            println!("{}", String::from_utf8(bytes).expect("c1 is UTF-8"));
        }
        "cert-verify" => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let certificate = uniqnode::c1::parse(&input)
                .map_err(|e| StoreError::Invalid(format!("the input is not JSON: {e}")))?;
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
                    "uniqnode: embed: {flag} is not an argument of embed (reranking belongs to the search \
                     layer, which serve and mcp opening the store directly carry)"
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
                    "uniqnode: discarded the last {1} bytes of {0} (a record cut off in the middle of an append)",
                    path.display(),
                    cache.discarded_tail_bytes()
                );
            }
            println!("cache: {} ({} vectors)", path.display(), cache.vector_count());
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
            // ストアを直接開く形は、ログを開く前にデータのディレクトリを検める(親が無ければ
            // 何も作らずに断る。ログの logs/ が先にデータのディレクトリを作らないように)。
            if options.serve_url.is_none() {
                prepare_data_dir_or_exit(dir, uniqnode::log::MCP_ROLE, &options.run.log);
            }
            // 何よりも先に開く。ストアを開けない・転送先が誤っている、といった起動時の
            // 失敗こそ残したい記録である。
            start_logging(dir, uniqnode::log::MCP_ROLE, &options.run.log);
            refuse_misplaced_listen_agent(uniqnode::log::MCP_ROLE, &options.run);
            let backend = match &options.serve_url {
                // 転送する形: 走っている serve の REST へ回す。ストアを開かない
                // (排他ロックを取らない)ので、常駐したまま ingest・embed が通る。
                Some(url) => {
                    // 埋め込みも順位の取り直しも、装備するのは転送先の serve である。
                    // ここで受けても効かせる先が無いので、黙って捨てずに断る
                    // (must/0022 の同型)。
                    if let Some(flag) = options.run.embed.misplaced_equipment_flag() {
                        uniqnode::log_line!(
                            "uniqnode: mcp: --serve-url and {flag} are not used together \
                             (embedding and reranking belong to the serve forwarded to)"
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
                        search_warmer: None,
                        embedding,
                        reranker,
                        data_dir,
                        main_door: None,
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
                uniqnode::log_line!("uniqnode: mcp: standard input or output broke: {error}");
                std::process::exit(1);
            }
        }
        "serve" => {
            let address = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let options = parse_run_options(&rest[1..]);
            // ログを開く前にデータのディレクトリを検める(親が無ければ何も作らずに、標準
            // エラーにだけ理由を言って断る。APPEND_FAILURE の方針 1a)。
            prepare_data_dir_or_exit(dir, uniqnode::log::SERVE_ROLE, &options.log);
            // ストアを開くより先にログを開く。「開けない」「そのアドレスを使えない」も
            // 残したい記録である。
            start_logging(dir, uniqnode::log::SERVE_ROLE, &options.log);
            // 自動更新がバイナリを置き換えたら終わり、unit に新しい版で起こし直させる(unit が
            // UNIQNODE_SELF_RELOAD=1 を立てたときだけ。node/src/selfreload.rs)。起動の最初に張る:
            // 起動の途中で置き換えられても、走っている版との違いが分かるように。
            uniqnode::selfreload::spawn_watcher(uniqnode::log::SERVE_ROLE);
            // 主の口の境界(docs/plan/API_AUTH.md の 1)。束縛先と uid の集合の誤りは引数の誤り
            // なので、ストアを開く前に 2 で断る。
            if let Err(message) = uniqnode::main_door::check_listen(address, "serve") {
                uniqnode::log_line!("uniqnode: serve: {message}");
                std::process::exit(2);
            }
            let main_door = match uniqnode::install::effective_uid()
                .and_then(|euid| Ok((euid, uniqnode::main_door::read_overflow_uid()?)))
                .and_then(|(euid, overflow_uid)| {
                    uniqnode::main_door::MainDoor::new(
                        options.main_allow_uid.clone(),
                        euid,
                        overflow_uid,
                    )
                }) {
                Ok(door) => std::sync::Arc::new(door),
                Err(message) => {
                    uniqnode::log_line!("uniqnode: serve: {message}");
                    std::process::exit(2);
                }
            };
            // 順序は「主の口の検査 → 束縛する → 自己試験 → ストアを開く → 装備する →
            // listening on」。起動時の検査(束縛先・OS・overflowuid・束縛そのもの・自分の接続の
            // uid の判定)はすべてストアを開く前に
            // 済ませ、断るときはストアに触れずに終わる(API_AUTH の 1)。標準出力の
            // listening on の 1 行は起動スクリプトとの取り決めで、待つ側は「出たら要求を
            // 受け付ける」と信じてよい。だから束縛の後でも、ストアを開いて装備し終えるまでは
            // この行を出さず accept もしない。その間に落ちるもの(ロックを持つ別プロセス、
            // 誤った URL)は、行を出さないうちに理由を言って終わる。行を出してから開くと、
            // 待つ側は騙され、その後で exit 1 する(2026-09-05 に systemd のデプロイで観測)。
            let listener = match std::net::TcpListener::bind(address) {
                Ok(listener) => listener,
                Err(error) => {
                    uniqnode::log_line!("uniqnode: serve: cannot bind to {address}: {error}");
                    std::process::exit(1);
                }
            };
            let hooks = main_door.test_hooks_in_use();
            if !hooks.is_empty() {
                uniqnode::log_line!(
                    "uniqnode: serve: the main door's test hook is active ({}); it is for tests of debug builds \
                     and must not be used in a deployment",
                    hooks.join(", ")
                );
            }
            // 起動時の自己試験(束縛の後、ストアを開く前)。自分の接続の uid を判定できなければ、
            // 起きても全部の接続を 403 で断るだけなので、理由を言って終わる。設定の誤り
            // (AF_NETLINK を塞いだ unit など)は 2、それ以外は一時の失敗でありうるので 3 で
            // 終わる(unit はどちらも起こし直す。docs/mop/SYSTEMD.md)。
            if let Err(error) = main_door.self_test(&listener) {
                uniqnode::log_line!("uniqnode: serve: {error}");
                std::process::exit(error.exit_code());
            }
            let data_dir = std::path::PathBuf::from(dir);
            let (capacity_bytes, health_params) = uniqnode::health::read_node_config(&data_dir);
            let mut store_config = uniqnode::store::StoreConfig::new(&data_dir);
            store_config.capacity_bytes = capacity_bytes;
            let store = match uniqnode::store::Store::open(store_config) {
                Ok(s) => std::sync::Arc::new(std::sync::Mutex::new(s)),
                Err(e) => {
                    uniqnode::log_line!("uniqnode: cannot open the store: {e}");
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
            // テストや起動スクリプトが実際のポートを知れるように、束縛先を必ず表示する。
            let bound = listener.local_addr()?;
            println!("listening on {bound}");
            use std::io::Write as _;
            std::io::stdout().flush()?;
            // 標準出力の 1 行は起動スクリプトとの取り決めなので形を変えない。ログにも
            // 残すのは、後から「いつ、どのアドレスで起きたか」を読めるようにするため。
            uniqnode::log_line!("uniqnode: serve: listening on {bound}");
            let mut main_hosts = uniqnode::main_door::default_hosts(bound);
            for host in &options.main_allow_host {
                if !main_hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
                    main_hosts.push(host.clone());
                }
            }
            let gate = uniqnode::http::BrowserGate::new(&main_hosts);
            uniqnode::log_line!(
                "uniqnode: serve: the main door allows uid {} and Host {}",
                uniqnode::sock_diag::uid_list(main_door.allowed_uids()),
                gate.allowed_hosts().join(", ")
            );
            let gauge = uniqnode::http::DoorGauge::with_refusal_drain(main_door.refusal_drain());
            let context = std::sync::Arc::new(uniqnode::api::ApiContext {
                store,
                engine,
                health: Some(health),
                referrers: std::sync::Mutex::new(None),
                search: std::sync::Mutex::new(None),
                search_warmer: Some(uniqnode::api::IndexWarmer::new()),
                embedding,
                reranker,
                // ページの写しの作業ファイル置き場を組むために持つ(健全性エンジンへ
                // 渡した data_dir は移動済みなので、同じ dir から作り直す)。
                data_dir: std::path::PathBuf::from(dir),
                main_door: Some(gauge.clone()),
            });
            // 読み口(AGENT_DOOR (uuid:02f79aec-2f12-41e6-bede-1557d4719e4d))。主の口と
            // 同じ ApiContext を共有し、許可表を通った要求だけを同じ api::handle に委ねる。
            // 主の口の listening on の後に開く(束縛できなくても主の口は殺さない。
            // 裏のスレッドで再試行する)。無指定なら第 2 の口は存在しない。
            if let Some(agent_address) = options.listen_agent.clone() {
                let context = context.clone();
                // 書く口(--agent-writable の集合)。空なら読み口は読むだけ。
                let writable = options.agent_writable.clone();
                if !writable.is_empty() {
                    uniqnode::log_line!(
                        "uniqnode: serve: collections writable through the agent door: {}",
                        writable.join(", ")
                    );
                }
                // 読める集合(--agent-collections)。空なら全コレクションが読める(既定)。
                let readable = options.agent_collections.clone();
                if !readable.is_empty() {
                    uniqnode::log_line!(
                        "uniqnode: serve: collections readable through the agent door: {}",
                        readable.join(", ")
                    );
                }
                // グラフ層(--agent-graph / --agent-graph-writable)。どちらも空なら
                // 読み口からグラフは見えない。
                let allowed = uniqnode::agent_door::Allowed {
                    writable,
                    readable,
                    graphs: options.agent_graph.clone(),
                    graphs_writable: options.agent_graph_writable.clone(),
                };
                if !allowed.readable_graphs().is_empty() {
                    uniqnode::log_line!(
                        "uniqnode: serve: graphs readable through the agent door: {} (writable: {})",
                        allowed.readable_graphs().join(", "),
                        match allowed.graphs_writable.is_empty() {
                            true => "none".to_string(),
                            false => allowed.graphs_writable.join(", "),
                        }
                    );
                }
                let door: std::sync::Arc<uniqnode::http::PeerHandler> =
                    std::sync::Arc::new(move |request, peer| {
                        uniqnode::agent_door::handle(&context, request, peer, &allowed)
                    });
                uniqnode::agent_door::open(agent_address, door);
            }
            // 索引は要求を待たずに裏で温める(起動直後と、書き込みの後。api.rs の
            // start_index_warmer)。束縛はもう済んでいるので、これが listening on を遅らせる
            // ことは無い。
            uniqnode::api::start_index_warmer(context.clone());
            // 主の口: 接続ごとに uid を判定し(要求を読む前)、要求ごとにブラウザの門と道ごとの
            // 本文の型を見てから api::handle に委ねる。
            let handler: std::sync::Arc<uniqnode::http::PeerHandler> =
                std::sync::Arc::new(move |request, _peer| {
                    gate.screen(request)
                        .or_else(|| uniqnode::main_door::screen_content_type(request))
                        .unwrap_or_else(|| uniqnode::api::handle(&context, request))
                });
            let check: std::sync::Arc<uniqnode::http::ConnectionCheck> =
                std::sync::Arc::new(move |stream| main_door.check(stream));
            uniqnode::http::serve_checked(listener, gauge, check, handler);
        }
        // RAG ビューワ(VIEWER (uuid:4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847))。1 枚の HTML を
        // 出し、その頁が呼ぶ /v1/* は走っている serve へ転送する。ストアを開かないので、
        // serve が常駐したまま起こせる。
        "viewer" => {
            let address = rest.first().map(String::as_str).unwrap_or_else(|| usage());
            let options = parse_mcp_options(&rest[1..]);
            start_logging(dir, uniqnode::log::VIEWER_ROLE, &options.run.log);
            uniqnode::selfreload::spawn_watcher(uniqnode::log::VIEWER_ROLE);
            refuse_misplaced_listen_agent(uniqnode::log::VIEWER_ROLE, &options.run);
            // ビューワは書き込みの口を持たない。mcp と読み手を共有しているので字面は
            // 通るが、効かせる先の無い指定は黙って捨てずに断る(must/0022 の同型)。
            if !options.writable.is_empty() {
                uniqnode::log_line!(
                    "uniqnode: viewer: --writable is not an argument of viewer (write permission is \
                     an option of mcp)"
                );
                std::process::exit(2);
            }
            if let Some(flag) = options.run.embed.misplaced_equipment_flag() {
                uniqnode::log_line!(
                    "uniqnode: viewer: {flag} is not an argument of viewer \
                     (embedding and reranking belong to the serve forwarded to)"
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
                    uniqnode::log_line!("uniqnode: viewer: cannot bind to {address}: {error}");
                    std::process::exit(1);
                }
            };
            let bound = listener.local_addr()?;
            // serve と同じ取り決め: 実際の束縛先を標準出力の 1 行で言う(:0 で起こした
            // テストや起動スクリプトが読む)。
            println!("listening on {bound}");
            std::io::stdout().flush()?;
            uniqnode::log_line!(
                "uniqnode: viewer: listening on http://{bound} (searches are forwarded to {})",
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
                StoreError::Invalid("min_replicas must be a non-negative integer".into())
            })?;
            let seq = store.set_pin(root, min_replicas)?;
            if min_replicas > 0 && !store.own_attested_roots().contains(&root.to_string()) {
                store.set_attest(root, true)?;
            }
            println!("seq: {seq}");
        }
        // systemd にデプロイする(SYSTEMD (uuid:7de68e4a-e6a6-4930-8cc7-a56f90f522e2))。手順ごとに
        // 1 行を出し、失敗は理由を言って 1 で終わる(途中まで置いたものはそのまま残る。
        // 再実行が更新になるので、直して同じ命令を打てばよい)。
        "install" => {
            let options = parse_install_options(dir, rest);
            if let Err(message) = uniqnode::install::run(options, &mut std::io::stdout()) {
                eprintln!("uniqnode: install: {message}");
                std::process::exit(1);
            }
        }
        // 自動更新の 1 回のループ(SYSTEMD (uuid:7de68e4a-e6a6-4930-8cc7-a56f90f522e2) の「自動
        // 更新」)。デプロイしなかった回と前のバージョンへ戻した回は理由を言って 1 で終わる。
        "autodeploy" => {
            let options = parse_autodeploy_options(dir, rest);
            if let Err(message) = uniqnode::autodeploy::run(&options) {
                eprintln!("uniqnode: autodeploy: {message}");
                std::process::exit(1);
            }
        }
        "install-autodeploy" => {
            let options = parse_install_autodeploy_options(dir, rest);
            if let Err(message) =
                uniqnode::autodeploy::install_units(&options, &mut std::io::stdout())
            {
                eprintln!("uniqnode: install-autodeploy: {message}");
                std::process::exit(1);
            }
        }
        "apply" => {
            let found = parse_flag_pairs(rest, &["--log-dir"], &[]);
            let log_dir = found
                .get("--log-dir")
                .cloned()
                .unwrap_or_else(|| uniqnode::apply::DEFAULT_LOG_DIR.to_string());
            if let Err(message) = uniqnode::apply::run(
                std::path::Path::new(dir),
                std::path::Path::new(&log_dir),
                std::path::Path::new(uniqnode::install::RUNNING_BINARY),
            ) {
                eprintln!("uniqnode: apply: {message}");
                std::process::exit(1);
            }
        }
        // クラッシュ試験用: kill されるまで最速で書き続ける(fsync 済み書き込みの途中を kill -9 で
        // 裂き、回復経路を実プロセスで検証するため。node/tests/crash.rs)。利用者の命令ではない
        // ので、debug ビルドにだけ置き、使い方の一覧にも載せない。
        #[cfg(debug_assertions)]
        "flood" => {
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

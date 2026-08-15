//! uniqnode CLI。ストアの初期化・操作・検証(SPEC §10 の HTTP API はこの上に載せる)。

use std::io::{Read, Write};
use uniqnode::store::{Store, StoreConfig, StoreError};

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
           serve <dir> <addr>         HTTP API を提供する(例: 127.0.0.1:7440、:0 で自動割当)\n\
           sync <dir> <peer_addr>     相手から pull で同期する(serve 停止中のストア用。\n\
                                      serve 中は POST /v1/sync を使う)\n\
           flood <dir>                書き込み続ける(クラッシュ試験用の内部コマンド)"
    );
    std::process::exit(2);
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
        "serve" => {
            let address = rest.first().map(String::as_str).unwrap_or_else(|| usage());
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
            let context =
                uniqnode::api::ApiContext { store, engine, health: Some(health) };
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

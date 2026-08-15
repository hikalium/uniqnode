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

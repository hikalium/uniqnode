//! 自己再起動: 常駐する serve と viewer は、ディスクの上の自分のバイナリを見張り、デプロイが
//! それを置き換えたら終わる。unit の Restart= が新しいバイナリで起こし直すので、更新に sudo も
//! 手での restart も要らない(docs/mop/SYSTEMD.md の「自動更新」。lamalium の
//! project_policy/must/sudo-free-deploy-and-self-reloading-host-services.md と同じ形)。
//!
//! 見張るのは環境変数 SELF_RELOAD_ENV が "1" のときだけである。unit のテンプレートが立てる。
//! 手で走らせた serve やテストは、組み直しで終わらない。

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 見張りを有効にする環境変数(unit のテンプレートの serve と viewer が "1" を立てる)。
pub const SELF_RELOAD_ENV: &str = "UNIQNODE_SELF_RELOAD";

/// 置き換えを見つけて終わるときの終了コード。0 は意図した停止で起こし直されないので、
/// Restart=on-failure が起こし直す 0 でない値にする(EX_TEMPFAIL)。
pub const SELF_RELOAD_EXIT: i32 = 75;

/// 名が外れた実行ファイルの /proc/self/exe の readlink に Linux が付ける印。
const DELETED_SUFFIX: &str = " (deleted)";

/// 見張りの間隔。
pub const SELF_RELOAD_TICK: Duration = Duration::from_secs(5);

/// ファイルの識別(dev, inode)。rename で置き換えると inode が変わる。
pub type FileIdentity = (u64, u64);

fn identity(path: &Path) -> std::io::Result<FileIdentity> {
    let metadata = std::fs::metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}

/// 走っている版の識別。/proc/self/exe の metadata は、名前が外れた後でも走っている実体を指す。
pub fn running_identity() -> std::io::Result<FileIdentity> {
    identity(Path::new("/proc/self/exe"))
}

/// path に今ある版が、走っている版 running と違うか。path を stat できないとき
/// (置き換えの途中など)は違うと言わない。
pub fn replaced(path: &Path, running: FileIdentity) -> bool {
    matches!(identity(path), Ok(on_disk) if on_disk != running)
}

/// 環境変数が立っていれば見張りのスレッドを起こす。role はログの名乗り(serve、viewer)。
/// 見張りを張れないときは、黙らずにログに言って見張らずに進む(must/0022)。
pub fn spawn_watcher(role: &str) {
    if std::env::var(SELF_RELOAD_ENV).as_deref() != Ok("1") {
        return;
    }
    // パスは起動の最初に一度だけ取る(serve と viewer は引数を読んでログを開いた直後に呼ぶ)。
    // それでも exec からここまでの間に置き換えられていれば、readlink の名に " (deleted)" が
    // 付くので外す。外した道には新しい版があり、走っている版(/proc/self/exe)と違うので、
    // 最初の見張りで終わる。
    let path: PathBuf = match std::env::current_exe() {
        Ok(path) => match path.to_str().and_then(|text| text.strip_suffix(DELETED_SUFFIX)) {
            Some(stripped) => PathBuf::from(stripped),
            None => path,
        },
        Err(error) => {
            crate::log_line!("uniqnode: {role}: 自分のバイナリの道が分からず、自己再起動を見張らない: {error}");
            return;
        }
    };
    let running = match running_identity() {
        Ok(running) => running,
        Err(error) => {
            crate::log_line!("uniqnode: {role}: /proc/self/exe を読めず、自己再起動を見張らない: {error}");
            return;
        }
    };
    let role = role.to_string();
    std::thread::spawn(move || loop {
        if replaced(&path, running) {
            crate::log_line!(
                "uniqnode: {role}: バイナリ {} が置き換えられた。新しい版で起こし直してもらうため {SELF_RELOAD_EXIT} で終わる",
                path.display()
            );
            std::process::exit(SELF_RELOAD_EXIT);
        }
        std::thread::sleep(SELF_RELOAD_TICK);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renamed_replacement_is_seen_and_a_missing_path_is_not() {
        let dir = std::env::temp_dir().join(format!("uniqnode-selfreload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("bin");
        std::fs::write(&path, b"old").expect("write");
        let running = identity(&path).expect("stat");
        assert!(!replaced(&path, running), "同じファイルは置き換えではない");
        let staging = dir.join(".bin.new");
        std::fs::write(&staging, b"new").expect("write");
        std::fs::rename(&staging, &path).expect("rename");
        assert!(replaced(&path, running), "rename で置き換えたら見つける");
        std::fs::remove_file(&path).expect("remove");
        assert!(!replaced(&path, running), "道が無いときは終わらない");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

//! `uniqnode apply <ファイル> [--log-dir <dir>]`: ファイルに書いた uniqnode の命令を順に走らせる
//! (docs/mop/SYSTEMD.md の「自動更新」)。機械の据え付け(install と install-autodeploy)を
//! 操作者の 1 行の命令にするためのもので、vega のファイルは docs/mop/vega/vega.apply にある。
//!
//! ファイルの形: 空行で区切った塊が 1 つの命令で、塊の中は 1 行 1 引数(引用符を解釈しない。
//! `--serve-options` の値のように空白を含む引数も 1 行に書く)。`#` で始まる行は読まない。
//! 塊ごとに、走っている自分自身(/proc/self/exe)を子として起こし、最初の失敗で止まる。
//! 途中で checkout のバイナリが組み直されても、全ての塊が同じ版で走る。

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// ログの置き場の既定。
pub const DEFAULT_LOG_DIR: &str = "/var/log";

/// ファイルを塊(1 つの命令の引数列)に割る。
pub fn parse(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut blocks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        current.push(line.trim().to_string());
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    if blocks.is_empty() {
        return Err("走らせる命令が 1 つも無い".to_string());
    }
    Ok(blocks)
}

struct Log {
    file: std::fs::File,
}

impl Log {
    fn line(&mut self, text: &str) {
        let stamp = crate::clock::format_unix_time(crate::clock::unix_now());
        println!("{stamp} {text}");
        // ログに書けなくても止めない。標準出力には同じ行が出ている。
        let _ = writeln!(self.file, "{stamp} {text}");
    }
}

/// 全ての塊を走らせる。Err は最初に失敗した塊の理由。
pub fn run(file: &Path, log_dir: &Path, program: &Path) -> Result<PathBuf, String> {
    let text =
        std::fs::read_to_string(file).map_err(|e| format!("{} を読めない: {e}", file.display()))?;
    let blocks = parse(&text)?;
    let stamp = crate::clock::format_unix_time(crate::clock::unix_now()).replace([':', '-'], "");
    // 名には pid も入れ、既にある名なら断る(同じ秒の 2 回目が前のログを消さない)。
    let log_path = log_dir.join(format!("uniqnode-apply-{stamp}-{}.log", std::process::id()));
    let log_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&log_path)
        .map_err(|e| format!("{} を作れない: {e}", log_path.display()))?;
    let mut log = Log { file: log_file };
    log.line(&format!(
        "apply: {} の {} 個の命令を走らせる。ログは {}",
        file.display(),
        blocks.len(),
        log_path.display()
    ));
    for (index, block) in blocks.iter().enumerate() {
        let number = index + 1;
        log.line(&format!("apply: {number}/{}: uniqnode {}", blocks.len(), block.join(" ")));
        let (reader, writer) =
            std::io::pipe().map_err(|e| format!("子の出力の pipe を作れない: {e}"))?;
        let mut command = Command::new(program);
        command.args(block);
        command.stdout(writer.try_clone().map_err(|e| format!("pipe を複製できない: {e}"))?);
        command.stderr(writer);
        let mut child = command
            .spawn()
            .map_err(|e| format!("{} を起こせない: {e}", program.display()))?;
        // 親の手元の書き口を閉じないと、子が終わっても読み口が終わりを見ない。
        drop(command);
        // UTF-8 でないバイトがあっても読み続ける(読むのをやめると子が EPIPE で落ちる)。
        let mut reader = std::io::BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => log.line(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r'])),
                Err(e) => {
                    log.line(&format!("apply: 子の出力を読めない: {e}"));
                    break;
                }
            }
        }
        let status = child.wait().map_err(|e| format!("子を待てない: {e}"))?;
        if !status.success() {
            let message = format!(
                "{number} 番目の命令が {status} で終わった。後の命令は走らせない。ログは {}",
                log_path.display()
            );
            log.line(&format!("apply: {message}"));
            return Err(message);
        }
    }
    log.line(&format!("apply: {} 個の命令を全て終えた。ログは {}", blocks.len(), log_path.display()));
    Ok(log_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_split_on_blank_lines_and_keep_spaces_inside_an_argument() {
        let text = "# 説明\ninstall\n/data\n--serve-options\n--embed http://x --rerank http://y\n\n\n# 2 つ目\ninstall-autodeploy\n/repo\n";
        let blocks = parse(text).expect("読める");
        assert_eq!(
            blocks,
            vec![
                vec![
                    "install".to_string(),
                    "/data".to_string(),
                    "--serve-options".to_string(),
                    "--embed http://x --rerank http://y".to_string(),
                ],
                vec!["install-autodeploy".to_string(), "/repo".to_string()],
            ]
        );
    }

    #[test]
    fn a_file_without_commands_is_refused() {
        assert!(parse("# だけ\n\n").is_err());
    }
}

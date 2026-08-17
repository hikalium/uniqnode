//! 時刻の一箇所(should/0135)。現在の unix 秒を採るのも、unix 秒を UTC の文字列に
//! 描くのも、この module だけが行う。出典の取得日時(node/src/mcp.rs)と運用ログの
//! 時刻(node/src/log.rs)は同じ描き方でなければ突き合わせられない。

/// 現在の unix 秒。時計が 1970 より前を指すような機械では 0 を返す(記録の時刻が
/// 狂うだけで、止める理由にはならない)。
pub fn unix_now() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs() as i64,
        Err(_) => 0,
    }
}

/// unix 秒を UTC の日時にする(例 2026-08-17T04:05:06Z)。取得日時は LLM が読む出典の
/// 一部であり、ログの時刻は後から読む者の手掛かりであって、どちらも整数のままでは
/// 日付として読めない。外部クレートは使えない(must/0008)ので暦の計算はここに置く。
pub fn format_unix_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60
    )
}

/// 1970-01-01 からの日数を暦の年月日にする(Howard Hinnant の civil_from_days。
/// グレゴリオ暦の 400 年周期を使う閉じた式で、閏日の表を持たない)。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // 3 月始まりの年に移すと、閏日が年の最後に来て場合分けが消える。
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 暦の期待値はリテラルで書く(検査対象から導出しない。should/0137)。値は
    /// date -u -d @<秒> で突き合わせたもの。
    #[test]
    fn unix_seconds_render_as_utc_timestamps() {
        assert_eq!(format_unix_time(0), "1970-01-01T00:00:00Z");
        // 閏年の 2 月 29 日(400 年周期の閏年)。
        assert_eq!(format_unix_time(951_782_400), "2000-02-29T00:00:00Z");
        // 平年の 3 月 1 日(1900 は閏年ではない周期の側)。
        assert_eq!(format_unix_time(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(format_unix_time(1_755_000_000), "2025-08-12T12:00:00Z");
        // 1970 より前は負の秒。境界で 1 日ずれないことを見る。
        assert_eq!(format_unix_time(-1), "1969-12-31T23:59:59Z");
    }

    /// 現在時刻は 2020 年より後を指す(時計が読めていることの最小の確認)。
    #[test]
    fn the_current_time_is_after_2020() {
        assert!(
            unix_now() > 1_577_836_800,
            "現在の unix 秒が 2020-01-01 より前: {}",
            unix_now()
        );
    }
}

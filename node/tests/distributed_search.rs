//! 分散検索(kind:search)の統合テスト(DISTRIBUTED_SEARCH
//! (uuid:e577f6db-659e-4eb8-a152-3b7780e4a9d1))。2 台の実プロセスの serve を起こし、
//! 生 HTTP/1.1 で当てる(should/0138)。資材はソースとは別ファイル(should/0112)。

mod common;
use common::*;

/// 日本語文書(検索の統合テストと共用)。
const SEARCH_JA: &str = include_str!("assets/search_ja.md");
/// 英語文書(識別子 token_estimate を持つ)。
const SEARCH_EN: &str = include_str!("assets/search_en.md");

fn put_document(address: &str, collection: &str, name: &str, body: &[u8]) {
    let path = format!("/v1/collections/{collection}/documents/{name}");
    let response = simple(address, "PUT", &path, body);
    assert_eq!(response.status, 200, "{}", body_text(&response));
}

fn node_id_of(address: &str) -> String {
    let status = body_text(&simple(address, "GET", "/v1/status", b""));
    json_text_field(&status, "node_id").expect("status の node_id")
}

/// peers.json を書く(serve は要求のたびに読み直すので、再起動は要らない)。
fn write_peers(server: &Server, peers: &str) {
    std::fs::write(server.dir.join("peers.json"), format!("{{\"peers\":[{peers}]}}"))
        .expect("peers.json を書く");
}

/// peers.json のエントリ 1 件(アドレスと相手の DBノードID)。
fn peer_entry(address: &str, node_id: &str) -> String {
    format!("{{\"address\":\"{address}\",\"node_id\":\"{node_id}\"}}")
}

/// 分散検索を 1 本投げる。予算は短く採る(沈黙の確定を速くするため)。
fn distributed_search(address: &str, body: &str) -> String {
    let response = simple(address, "POST", "/v1/search", body.as_bytes());
    assert_eq!(response.status, 200, "{}", body_text(&response));
    body_text(&response)
}

/// 応答の results から、その ID の件が持つ sources を取り出す(素朴な抽出)。
fn sources_of(body: &str, id: &str) -> Option<String> {
    let entry = body.split("{\"citation\"").find(|part| part.contains(id))?;
    let rest = entry.split("\"sources\":[").nth(1)?;
    Some(rest.split(']').next()?.to_string())
}

/// 完了条件(docs/plan/RAG.md の 6): 2 台に分かれた知識が 1 本のクエリで返る。
/// 出典はどちらのDBノードから来たかを sources が持ち、決着は SPEC §7.2 の 3 値である。
#[test]
fn knowledge_split_across_two_nodes_comes_back_from_one_query() {
    let here = start_server("dsearch-here");
    let there = start_server("dsearch-there");
    // 知識を分ける: 日本語の文書はこちら、英語の文書は向こうにしか無い。
    put_document(&here.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    put_document(&there.address, "notes", "search_en.md", SEARCH_EN.as_bytes());
    let here_id = node_id_of(&here.address);
    let there_id = node_id_of(&there.address);
    write_peers(&here, &peer_entry(&there.address, &there_id));
    write_peers(&there, &peer_entry(&here.address, &here_id));

    // 散布しない要求は、これまでどおり自分の索引だけで答える(既定は変わっていない)。
    let local_only = distributed_search(&here.address, "{\"query\":\"世代の整合 token_estimate\"}");
    assert!(!local_only.contains("\"outcome\""), "散布していない応答に決着は載らない: {local_only}");
    assert!(local_only.contains("\"document\":\"search_ja\""), "{local_only}");
    assert!(!local_only.contains("\"document\":\"search_en\""), "向こうの知識は来ない: {local_only}");

    // 散布する要求。1 本の問いで両方のDBノードの知識が返る。
    let body = distributed_search(
        &here.address,
        "{\"query\":\"世代の整合 token_estimate\",\"peers\":true,\"budget_ms\":3000}",
    );
    assert!(body.contains("\"outcome\":\"found\""), "{body}");
    assert!(body.contains("\"score_semantics\":\"rrf\""), "順位の融合で答える: {body}");
    assert!(body.contains("\"document\":\"search_ja\""), "こちらの知識: {body}");
    assert!(body.contains("\"document\":\"search_en\""), "向こうの知識: {body}");
    assert!(
        body.contains("\"state\":\"answered\"") && body.contains(&there.address),
        "散布先の経過が載る: {body}"
    );
    assert!(body.contains(&format!("\"node_id\":\"{there_id}\"")), "答えた相手の名乗り: {body}");

    // 出所: こちらの件は local、向こうの件は相手のアドレスである。
    let ja_sources = sources_of(&body, "search_ja").expect("こちらの件");
    assert_eq!(ja_sources, "\"local\"", "{body}");
    let en_sources = sources_of(&body, "search_en").expect("向こうの件");
    assert_eq!(en_sources, format!("\"{}\"", there.address), "{body}");
}

/// 同じ本文のチャンクが両方のDBノードにあるときは、内容ハッシュが同じなので 1 件に
/// まとまり、両方が出所として残る(content-addressing の帰結)。
#[test]
fn the_same_chunk_on_two_nodes_fuses_into_one_result() {
    let here = start_server("dsearch-same-here");
    let there = start_server("dsearch-same-there");
    put_document(&here.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    put_document(&there.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    let here_id = node_id_of(&here.address);
    let there_id = node_id_of(&there.address);
    write_peers(&here, &peer_entry(&there.address, &there_id));
    write_peers(&there, &peer_entry(&here.address, &here_id));

    let body = distributed_search(
        &here.address,
        "{\"query\":\"世代の整合\",\"peers\":true,\"budget_ms\":3000}",
    );
    assert_eq!(body.matches("\"id\":").count(), 1, "同じチャンクは 1 件にまとまる: {body}");
    let sources = sources_of(&body, "search_ja").expect("件がある");
    assert_eq!(sources, format!("\"local\",\"{}\"", there.address), "{body}");
}

/// 応答側の双方向フィルタ: peers.json に node_id を書いていない相手には答えない。
/// 断りは沈黙として扱われ(情報ゼロ)、理由は経過に残る。
#[test]
fn a_node_that_is_not_registered_gets_no_answer() {
    let here = start_server("dsearch-unknown-here");
    let there = start_server("dsearch-unknown-there");
    put_document(&there.address, "notes", "search_en.md", SEARCH_EN.as_bytes());
    let here_id = node_id_of(&here.address);
    let there_id = node_id_of(&there.address);
    write_peers(&here, &peer_entry(&there.address, &there_id));
    // 向こうはこちらを知らない(peers.json が空)。
    write_peers(&there, "");

    let refused = distributed_search(
        &here.address,
        "{\"query\":\"token_estimate\",\"peers\":true,\"budget_ms\":2500}",
    );
    assert!(refused.contains("\"outcome\":\"timed_out\""), "{refused}");
    assert!(refused.contains("\"state\":\"silent\""), "{refused}");
    assert!(refused.contains("peers.json"), "断りの理由が経過に残る: {refused}");
    assert!(!refused.contains("\"document\":\"search_en\""), "知識は出ていない: {refused}");

    // trust_level 0 のエントリも同じく答えない(登録はしていても、答えない相手)。
    write_peers(
        &there,
        &format!(
            "{{\"address\":\"{}\",\"node_id\":\"{here_id}\",\"trust_level\":0}}",
            here.address
        ),
    );
    let untrusted = distributed_search(
        &here.address,
        "{\"query\":\"token_estimate\",\"peers\":true,\"budget_ms\":2500}",
    );
    assert!(untrusted.contains("trust_level"), "断りの理由: {untrusted}");
    assert!(!untrusted.contains("\"document\":\"search_en\""), "{untrusted}");

    // 登録すれば答える(同じ問いが通る)。
    write_peers(&there, &peer_entry(&here.address, &here_id));
    let answered = distributed_search(
        &here.address,
        "{\"query\":\"token_estimate\",\"peers\":true,\"budget_ms\":3000}",
    );
    assert!(answered.contains("\"outcome\":\"found\""), "{answered}");
    assert!(answered.contains("\"document\":\"search_en\""), "{answered}");
}

/// 署名の無い・改竄された QUERY は受けない(要求者の認証。SPEC §6.1)。
#[test]
fn an_unsigned_or_tampered_query_is_rejected() {
    let there = start_server("dsearch-signature");
    put_document(&there.address, "notes", "search_en.md", SEARCH_EN.as_bytes());

    // 署名の無い封筒。
    let unsigned = format!(
        "{{\"at\":{},\"budget_ms\":2000,\"kind\":\"search\",\"origin\":\"{}\",\
         \"payload\":{{\"query\":\"token_estimate\",\"top_k\":10}},\"query_id\":\"q1\",\"v\":1}}",
        uniqnode::clock::unix_now(),
        "ab".repeat(32),
    );
    let response = simple(&there.address, "POST", "/v1/peer/query", unsigned.as_bytes());
    assert_eq!(response.status, 401, "{}", body_text(&response));

    // 正しく署名した封筒を、本文だけ差し替える。
    let dir = unique_dir("dsearch-signature-client");
    let config = uniqnode::store::StoreConfig::new(&dir);
    let store = uniqnode::store::Store::open(config).expect("open");
    let request = uniqnode::api::SearchRequest {
        query: "token_estimate".to_string(),
        collection: None,
        top_k: 10,
        method: None,
        include_low_information: false,
    };
    let signed = uniqnode::distributed_search::query_message(&store, "q2", &request, 2_000);
    let tampered =
        String::from_utf8(signed.clone()).expect("utf-8").replace("token_estimate", "別の問い");
    let response = simple(&there.address, "POST", "/v1/peer/query", tampered.as_bytes());
    assert_eq!(response.status, 401, "{}", body_text(&response));

    // 署名は通るが、この DBノードは peers.json に無いので答えない(403 と 401 を分ける:
    // 直せるのが呼び出し側か運用者かが違う)。
    let response = simple(&there.address, "POST", "/v1/peer/query", &signed);
    assert_eq!(response.status, 403, "{}", body_text(&response));

    drop(store);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// share.collections を書いたピアには、その範囲だけを見せる。範囲の外のコレクションは
/// 空振りになり、空振りの理由(見ていない)が応答に残る。
#[test]
fn the_share_policy_limits_what_a_peer_can_search() {
    let here = start_server("dsearch-share-here");
    let there = start_server("dsearch-share-there");
    put_document(&there.address, "notes", "search_en.md", SEARCH_EN.as_bytes());
    put_document(&there.address, "secrets", "search_ja.md", SEARCH_JA.as_bytes());
    let here_id = node_id_of(&here.address);
    let there_id = node_id_of(&there.address);
    write_peers(&here, &peer_entry(&there.address, &there_id));
    write_peers(
        &there,
        &format!(
            "{{\"address\":\"{}\",\"node_id\":\"{here_id}\",\
             \"share\":{{\"collections\":[\"notes\"]}}}}",
            here.address
        ),
    );

    // 共有しているコレクションの語は返る。
    let shared = distributed_search(
        &here.address,
        "{\"query\":\"token_estimate\",\"peers\":true,\"budget_ms\":3000}",
    );
    assert!(shared.contains("\"document\":\"search_en\""), "{shared}");

    // 共有していないコレクションの語は返らない(相手は見てもいない)。
    let withheld = distributed_search(
        &here.address,
        "{\"query\":\"世代の整合\",\"peers\":true,\"budget_ms\":3000}",
    );
    assert!(!withheld.contains("\"document\":\"search_ja\""), "{withheld}");
    assert!(withheld.contains("\"state\":\"empty\""), "答えは来ている(空の言明): {withheld}");
    assert!(withheld.contains("\"outcome\":\"scope_empty\""), "{withheld}");
}

/// 沈黙(届かないピア)は情報ゼロであり、「存在しない」にはならない。予算で打ち切って
/// timed_out として返し、ローカルに当たりがあれば found のまま沈黙を残して報告する。
#[test]
fn a_silent_peer_times_out_without_denying_existence() {
    let here = start_server("dsearch-silent");
    put_document(&here.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    // 何も待ち受けていない宛先。
    write_peers(&here, "{\"address\":\"127.0.0.1:1\",\"node_id\":\"00\"}");

    let nothing = distributed_search(
        &here.address,
        "{\"query\":\"token_estimate\",\"peers\":true,\"budget_ms\":2500}",
    );
    assert!(nothing.contains("\"outcome\":\"timed_out\""), "{nothing}");
    assert!(nothing.contains("\"state\":\"silent\""), "{nothing}");

    let found = distributed_search(
        &here.address,
        "{\"query\":\"世代の整合\",\"peers\":true,\"budget_ms\":2500}",
    );
    assert!(found.contains("\"outcome\":\"found\""), "ローカルの当たりで found: {found}");
    assert!(found.contains("\"state\":\"silent\""), "沈黙は残ったまま報告される: {found}");
    assert!(found.contains("\"document\":\"search_ja\""), "{found}");
}

/// 要求側のフィルタ: min_trust_level 未満のピアへは問いを送らない(問いの文そのものが
/// 情報である)。明示の宛先を書けば、peers.json の順ではなくその宛先へ送る。
#[test]
fn the_requester_filters_its_scope_by_trust_level() {
    let here = start_server("dsearch-trust-here");
    let there = start_server("dsearch-trust-there");
    put_document(&there.address, "notes", "search_en.md", SEARCH_EN.as_bytes());
    let here_id = node_id_of(&here.address);
    let there_id = node_id_of(&there.address);
    write_peers(
        &here,
        &format!(
            "{{\"address\":\"{}\",\"node_id\":\"{there_id}\",\"trust_level\":10}}",
            there.address
        ),
    );
    write_peers(&there, &peer_entry(&here.address, &here_id));

    let filtered = distributed_search(
        &here.address,
        "{\"query\":\"token_estimate\",\"peers\":true,\"min_trust_level\":50,\"budget_ms\":2500}",
    );
    assert!(filtered.contains("\"peers\":[]"), "問いを送っていない: {filtered}");
    assert!(filtered.contains("\"outcome\":\"scope_empty\""), "{filtered}");
    assert!(!filtered.contains("\"document\":\"search_en\""), "{filtered}");

    // 閾値を下げれば同じピアへ届く。
    let sent = distributed_search(
        &here.address,
        "{\"query\":\"token_estimate\",\"peers\":true,\"min_trust_level\":10,\"budget_ms\":3000}",
    );
    assert!(sent.contains("\"document\":\"search_en\""), "{sent}");

    // 明示の宛先(peers.json の trust_level は宛先選びに使わない)。
    let explicit = distributed_search(
        &here.address,
        &format!(
            "{{\"query\":\"token_estimate\",\"peers\":[\"{}\"],\"budget_ms\":3000}}",
            there.address
        ),
    );
    assert!(explicit.contains("\"document\":\"search_en\""), "{explicit}");
}

/// 要求の組み立ての誤りは 400 で断る(黙って既定に倒さない)。
#[test]
fn a_malformed_scatter_request_is_refused() {
    let here = start_server("dsearch-malformed");
    for (body, expected) in [
        ("{\"query\":\"世代\",\"peers\":\"yes\"}", "peers は真偽値かアドレス文字列の配列"),
        ("{\"query\":\"世代\",\"peers\":[]}", "peers が空の配列"),
        ("{\"query\":\"世代\",\"peers\":true,\"budget_ms\":900000}", "budget_ms は"),
        ("{\"query\":\"世代\",\"peers\":true,\"min_trust_level\":\"高\"}", "min_trust_level は整数"),
    ] {
        let response = simple(&here.address, "POST", "/v1/search", body.as_bytes());
        assert_eq!(response.status, 400, "{body}");
        assert!(body_text(&response).contains(expected), "{}", body_text(&response));
    }
}

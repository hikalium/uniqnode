# BACKUP — バックアップと復元(封印済みセグメントの増分の写しと、fsck による検証)

<a id="e026a5e7-1ece-4f4e-b6b8-ee96c62883a2"></a>

読み手は、serve を常用しているノードのデータを失いたくない運用者と、node/src/backup.rs を
読む者。この文書は、何を写せば足りるか、どう写すか、写しから戻す手順、写しが壊れていた
ときの直し方を述べる。ストアの置き方そのもの(セグメントと MANIFEST の規律)は SPEC.md
§5 にあり、ここが決めるのはその規律を運用に写す手順だけである。

## 何を写し、何を写さないか

SPEC §5.1 の規律により、ストアのデータは「封印後不変のセグメント」と「atomic rename で
差し替える MANIFEST」だけである。写す対象はそれに、ストアのデータではないが失うと困る
設定を足したものになる。

| 項目 | 写す | 理由 |
|---|---|---|
| `packs/pack-NNNNNN.pack` | 写す | オブジェクトの本体。封印済みのものは二度と変わらない |
| `reflog/reflog-NNNNNN.log` | 写す | 署名済みの ref・pin・保持表明。同じ規律の追記ログ |
| `MANIFEST` | 写す | どのセグメントが封印済みかの記録。無いと全部が未封印と見なされ、破損を切り詰めで済ませてしまう |
| `node_key` | 写す | ノードの秘密鍵。失うと、このノードの名前空間の ref に二度と署名できない(復元したノードは別人になる) |
| `node.json`・`peers.json`・`groups.json` | 在れば写す | 容量と健全性の設定、ピアの登録、グループの鍵。作り直せるが手で書いたものなので写す |
| `derived/` | 写さない | 埋め込みのキャッシュと写しの作業ファイル。--embed 付きの serve が裏で作り直す(`uniqnode embed` でも作れる) |
| `logs/` | 写さない | 運用ログ。ストアの記録ではなく走行の記録([docs/design/LOGGING.md](#14a4e260-70af-4c52-9f19-1c116bddd004)) |
| `tmp/` | 写さない | atomic rename の作業場。中身は据えられる前の一時ファイルだけ |

写しの大きさはほぼ packs/ の大きさになる。実コーパス(仕様書 PDF、55433 オブジェクト)では
packs/ が 360 MB、derived/ が 292 MB、reflog と設定は合わせて 200 KB 未満である。

## アクティブ(未封印)なセグメントの扱い

写す。理由は 2 つある。

- 写さないと、最後の封印から後に入れたものが全部落ちる。封印の閾値は既定 256 MB なので、
  小さなノードでは何か月ぶんにもなり得る。実コーパスでも pack-000002(107 MB)は未封印で、
  写さなければ全体の 3 割が消える。
- 写しても壊れない。追記専用なので、どの瞬間の写しも「有効なレコード列 + 書き込み途中の
  尻尾」であり、レコードは `[len][crc32][payload]` の形なので(SPEC §5.2)、尻尾は写し先を
  開くときに CRC で切り詰められる。落ちるのは書きかけの 1 レコードだけで、それは次回の写しで
  完全な形が写る。

写し先を開いたときに何バイト切り詰めたかは backup 命令が `cut` の行で言う(黙って切らない)。

## 取り方: `uniqnode backup`

```
uniqnode backup <data_dir> <backup_dir>
```

- ロックを取らない。serve が走っているノードに対してそのまま走らせてよい。ストアを開く必要が
  ないのは、封印済みセグメントは不変で、未封印のものは上の理由で任意の瞬間の写しが使える
  からである。
- 増分である。写し先に同じ大きさで在る封印済みセグメントは写さない(封印後は不変で、内容は
  後の検証が読み直す)。未封印のセグメント・MANIFEST・設定は毎回まるごと写す。
- 順序が整合性を作る。MANIFEST を最初に読んで手元に留め、それが封印済みと言うセグメントを
  写し、最後に手元の MANIFEST を写し先へ据える。MANIFEST を先に読むので「封印済み」と記された
  ものは写す時点で不変であり、写しは完全である。写す途中で封印が進んだ分は次回の増分に回る。
  写し先の MANIFEST を最後に据えるので、途中で止まっても写し先は「前回の写し + 未封印の
  セグメント」として開ける。各ファイルは写し先の `tmp/` に書いて fsync してから rename で
  据えるので、据えた名前の下に不完全なファイルは現れない。
- 写し先にあって写し元に無い pack は消す。手元の MANIFEST が封印済みと言わず、写し元の
  `packs/` にも無い番号は、回収(gc。[docs/plan/PACK_GC.md](#f272eeda-8664-42da-9e5c-ef354bc3f3a7))
  で書き直された後の残骸である。残しておくと、写し先を開くときそれが未封印として走査され、
  尻切れがあれば破損と誤判定され、回収前の孤児が写し先で生き返る。MANIFEST が権威なので
  消してよく、消すのは写し先の MANIFEST を据える前(据えた後に残骸だけ残って止まる順序を
  作らない)。1 本ずつ `removed` の行で言い、黙って消さない。写し元に MANIFEST が無い(一度も
  封印していない)ときは権威が無いので消さず、`only in backup` の行で言う。reflog は消さない:
  reflog の回収は無く、封印もまだ無いので、pack の規則を対称に当てると署名済みの ref の記録を
  根拠なく消すことになる。在れば同じく `only in backup` で言う。
- 写した後に写し先をストアとして開き、fsck(全オブジェクトの再ハッシュと ref の整合)まで
  通す。検証の実装は開くときの回復と `uniqnode fsck` そのものであり、バックアップ専用の検証は
  無い(should/0135)。異常があれば理由を標準エラーに言って非 0 で終わる。
- 断る場合: 写し元に `node_key` と `packs/` が無い(ストアではない)、写し元と写し先が同じ場所、
  写し先の `node_key` が写し元と違う(別のノードの写しに混ぜようとしている)。「ストアであるか」
  の判定は node/src/store.rs の require_store_dir の 1 箇所で、fsck・status・get・refs が在る
  ストアだけを開く判定と同じものである(should/0135)。最後のものは、空の写し先に先に serve や
  init を走らせたときに起きる: 状態を作る命令は空のディレクトリを新しいノードとして初期化し、
  そこに別の `node_key` を作る。写しに何も無ければその `node_key` を消して取り直す。先に
  `uniqnode fsck` を走らせても起きない: 検査は在るストアだけを開き、ストアでない場所には何も
  作らずに「ストアのデータディレクトリではない」と言って 1 で終わる(node/tests/fsck.rs の
  a_restore_destination_checked_first_by_fsck_is_still_accepted_by_backup がこの順を通す)。

出力は写したもの・消したもの・消さずに残したものを 1 行ずつ言い、最後に集計と検証の 2 行を
出す。

```
copied packs/pack-000001.pack
copied packs/pack-000002.pack (active)
copied reflog/reflog-000001.log (active)
copied node_key
backup: sealed copied 1 unchanged 0, active 2, removed 0, settings 1, bytes 377389679, not copied: derived logs tmp
verify: objects 55433 refs 204 errors 0
```

2 回目は `unchanged packs/pack-000001.pack` になり、写すのは未封印の 2 本と設定だけになる。
回収の後の写しでは `removed packs/pack-000005.pack (not in source MANIFEST)` のように消した
残骸が並び、集計の `removed` にその本数が出る。
`not copied:` は写し元の直下にあって写さなかった項目の列挙で、derived・logs・tmp 以外の名前が
並んだら、それはこの手順が知らないファイルである。

終了コードは fsck 命令と同じ: 0 が緑、3 が検証に異常、1 が写せない・開けない。

定期的に取るなら systemd の timer から一回きりの命令として呼ぶ。unit の例は docs/mop/systemd/ の
uniqnode-backup.service と uniqnode-backup.timer(毎日 1 回)で、置き方と写し先の変え方は
[docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2) にある。写し先はローカルの
別ディスクでも、外付けでも、ネットワーク越しのマウントでもよい。

## rsync で同じことをするには

写す対象は上の表のとおりなので、rsync でも取れる。1 行で書くなら次の形になる。

```
rsync -a --include='/MANIFEST' --include='/node_key' --include='/node.json' --include='/peers.json' --include='/groups.json' --include='/packs/***' --include='/reflog/***' --exclude='*' <data_dir>/ <backup_dir>/
```

rsync はファイル一覧を名前順に処理するので、`MANIFEST` は `packs/` より先に写る(大文字が
小文字より前)。これが上で述べた「MANIFEST を先に読む」に当たる。`-a` は大きさと mtime の
一致するファイルを飛ばすので、封印済みセグメントは 2 回目から写らない。

rsync は写し先を検証しないので、取った後に必ず `uniqnode fsck <backup_dir>` を走らせる。
未封印セグメントの尻尾の切り詰めもこのときに起きる。fsck が赤なら下の「写しが壊れていたら」
へ進む。

## 復元

写しはそれ自体がストアである。戻し方は 2 つあり、どちらも最後に `uniqnode fsck` で内容まで
確かめる。

1. 写しをそのまま使う。`uniqnode fsck <backup_dir>` で緑を確かめ、`uniqnode serve <backup_dir> …`
   で起こす。ロックは場所ごとなので、元の data_dir が壊れたまま残っていても衝突しない。
2. 写し戻す。空のディレクトリ(または壊れた data_dir を脇へどけた跡)を新しい data_dir にして、
   `uniqnode backup <backup_dir> <new_data_dir>` を走らせる。backup は写し元がストアなら
   どちら向きにも使えるので、写し戻しの命令は別に無い。終わったら `uniqnode fsck <new_data_dir>`。

どちらでも、戻したノードは元と同じ node id を名乗る(`node_key` が写っているため)。自分の
名前空間の ref と保持表明はそのまま自分のものとして読める。`derived/` は無い状態から始まる
ので、意味検索を使うなら埋め込みを作り直す(--embed 付きで serve を起こせば起動直後に裏で
埋め始める。serve を起こす前に済ませるなら `uniqnode embed <new_data_dir>`)。全件は分単位
かかり、埋まるまでの検索は BM25 に劣化して degraded がそう言う。BM25 の検索と取り込み・取得は
埋め込み無しでも動く。

写し戻す先は空にしておく。backup は写し元(ここでは写し)を権威として写し先を揃えるので、
中身のある data_dir へ写すと、写しの MANIFEST に無い pack はそこに新しく書かれたものでも
`removed` の行を出して消され、reflog と設定は消されずに残って混ざり、どの時点の状態でもない
ストアになる。空であることを `uniqnode fsck <new_data_dir>` で
確かめてもよい: 空のディレクトリや存在しない道には「ストアのデータディレクトリではない」と
言って 1 で終わり、何も作らない(緑が出たら、そこには既にストアがある)。

systemd で常駐しているノード([docs/mop/SYSTEMD.md](#7de68e4a-e6a6-4930-8cc7-a56f90f522e2))
を写し戻すときは、先に unit を止める。serve がロックを持っている data_dir へは書けず、timer が
その最中に走ると壊れた写し元をそのまま写し先へ運ぶ:

```
systemctl --user stop uniqnode-backup.timer uniqnode-serve uniqnode-viewer
mv <data_dir> <data_dir>.broken-<日付>
uniqnode backup ~/uniqnode-backup <data_dir>     # 写し先の既定は install の --backup-dir
uniqnode fsck <data_dir>
systemctl --user start uniqnode-serve uniqnode-viewer uniqnode-backup.timer
```

起こした後は serve の journal に埋め込みとリランカーの行が出ること、`/v1/status` の
node_id が壊れる前と同じであることまで見る(should/0116)。

## 写しが壊れていたら

写し先の封印済みセグメントが 1 バイトでも変わると、`uniqnode fsck <backup_dir>` も次回の
`uniqnode backup` も「封印済み pack N が壊れている(バックアップからの復元が必要)」と言って
赤になる。増分の判定は大きさしか見ないので、同じ大きさのまま壊れたファイルは写し直され
ない。直すには、壊れた 1 本を写し先から消してもう一度 backup を走らせる。無いものは写し直す
ので、その 1 本だけが写り、検証が緑に戻る(node/tests/backup.rs の
a_corrupted_sealed_pack_in_the_backup_turns_fsck_red_and_is_recopied_after_removal がこの道を
そのまま通す)。

写し元のほうが壊れている(serve が起動時にこの診断を出す)ときは、写しから復元する。それが
この診断の言う「バックアップからの復元」である。

## 実測(2026-09-05)

実コーパス(55433 オブジェクト、packs/ 360 MB、serve が同じディレクトリを開いたまま)。

| 走らせ方 | 写した量 | 所要 |
|---|---|---|
| 初回(写し先が空) | 377 MB | 10.8 秒 |
| 2 回目(封印済み 1 本は不変、未封印 107 MB を写し直し) | 107 MB | 10.3 秒 |
| `uniqnode fsck <backup_dir>` だけ(写さない) | — | 10.0 秒 |

所要のほとんどは写し先の検証(開くときの全走査と fsck の再ハッシュ)で、写す量には
ほとんど比例しない(fsck だけで 10.0 秒。377 MB を写す初回との差は 1 秒に満たない)。
検証を省く指定は持たない。写しを取ったのに読めない、という事態を取った直後に知るための
ものだからである。

## 意図的に持たないもの

- 圧縮・暗号化・遠隔への転送。写し先はディレクトリであり、その先の運搬は rsync や
  ファイルシステムの仕事である。
- 世代の保持(日ごとの写しを何日ぶん残す、など)。写し先を日付ごとに分ければ済み、
  封印済みセグメントは不変なので世代間の重複はハードリンクや重複排除で潰せる。
- 検証を省く指定。上の理由による。
- 写し先だけにある設定ファイルと reflog の削除。写し元で消した `peers.json` が写し先に残って
  いたら、`only in backup` の行で言うだけで消さない。写し元に MANIFEST が無いときの pack も同じ。
  これらには「写し元に無い」を判定する権威が無く、写しから物を消すのは人の判断にしておく。
  消すのは、MANIFEST が権威として答えを持つ pack の残骸だけである(「取り方」)。

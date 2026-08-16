# 取り込みマイルストーンの手動受け入れ: 実データ投入の記録

<a id="0be862ff-9448-40ab-aa6b-31c796f6482e"></a>

日付: 2026-08-16。対象: INGEST 計画の完了条件のうち private 資材に依存する手動受け入れ
(注釈の段の一致率実測と、書き込み経路の段の総量記録)。手段: コミット 94a618f の実装で
新規ストア /work2/llm_playground_host_dir/uniqnode-store に実投入(pdftotext 22.02.0)。
ストアの node_id は c3ae05f3c24785b93fdd9c94b0e40a67a6374cd94265961c21dd7239300c6b47。

## 文書の投入(os_dev_specs_private の PDF 25 本)

`uniqnode ingest <store> specs <os_dev_specs_private>` の一回の実行で 25 本すべてが
updated になった(exit 0)。

| 文書 | chunks | new_objects |
|---|---|---|
| acpi_6_4 | 2282 | 2284 |
| armv8a_pg_1_0 | 483 | 485 |
| cdc_1_2 | 54 | 56 |
| ecm_1_2 | 40 | 42 |
| elf64_1_5_2 | 25 | 27 |
| elf_1_2 | 133 | 135 |
| hid_1_11 | 126 | 128 |
| hpet_1_0a | 55 | 57 |
| hut1_12v2 | 302 | 304 |
| ich9 | 1463 | 1465 |
| index | 1 | 3 |
| ncm_1_1 | 144 | 146 |
| pci_22 | 596 | 598 |
| pcie_20 | 992 | 994 |
| pcie_40 | 1712 | 1714 |
| rtl8139d | 100 | 102 |
| rtl8139pg | 14 | 16 |
| sdm_vol1 | 1155 | 1157 |
| sdm_vol2 | 4188 | 4182 |
| sdm_vol3 | 3513 | 3504 |
| sdm_vol4 | 896 | 896 |
| sysv_abi_0_99 | 155 | 157 |
| uefi_2_9 | 3858 | 3860 |
| usb_2_0 | 1111 | 1113 |
| usb_type_c | 614 | 616 |
| xhci_1_2 | 1083 | 1085 |
| 合計 | 25095 | 25126 |

- チャンク総数は PDF 25 本で 25094。表の合計 25095 には次項の index の 1 チャンクが入る。
- 予期との差が 1 件あった: SHA-1 台帳の index.txt は「対象外」と報告されると見込んでいたが、
  .txt は対象拡張子なのでプレーンテキストとして specs/index(チャンク 1)に取り込まれた。
  対象外と報告されたのは .git 配下の 26 ファイルだけ。実害はない(注釈は spec_id の ref
  だけを引くため index には触れない)が、コレクションを PDF だけにしたければ取り込み起点を
  *.pdf に絞るか ref を tombstone する。
- sdm_vol2 は chunks=4188 に対し new_objects=4182、sdm_vol3 は 3513 に対し 3504。文書内の
  同一本文チャンク(空白ページ等)を content-addressing が排除した実測で、書き込み経路の段が
  予告した重複コストの記録はこの表が与える。

## 注釈の投入(100 件)

承認リストは 1 行(sdm_vol2 317)で、`uniqnode ingest-annotations <store> specs
<data.md> --manual <リスト>` の一回の実行で:

- 取り込み 100 件 = 機械照合(method=token-match)99 件 + 手動承認(method=manual)1 件
  (sdm_vol2 p.317 CPUID list、一致 1/2)。
- 不一致(取り込まれず報告のみ)は 0 件。2026-08-16 の data.md 全件照合の内訳
  (INGEST 計画の前提節)と完全に一致した。
- new_objects=302。索引は s256:62b2ad3e39be47a04f209f99b800d123806ebc4cc7a5e5f31a7c6b19f26d8041
  (ref annotations/specs、seq 27)。

## 一致率の分布と安全窓

CLI が注釈ごとに印字する「一致 m/n」(タイトル側の語 n のうちページ本文に含まれた語 m)を
全 100 件について集計した。実装の追加は不要だった(受理・不一致の両方で印字済み)。

| 一致率 | 件数 | 該当 |
|---|---|---|
| 1.0 | 96 | |
| 8/9 = 0.889 | 1 | sdm_vol3 p.850 COUNTING CLOCKS |
| 10/12 = 0.833 | 1 | sdm_vol2 p.332 EAX=0x15 |
| 3/4 = 0.75 | 1 | pci_22 p.223 BAR: Base Address Register |
| 1/2 = 0.50 | 1 | sdm_vol2 p.317 CPUID list(manual 承認) |

- 判定は matched * 5 >= total * 3 の整数演算(一致率が閾値以上で通る)。
- 機械照合 99 件の最小一致率は 3/4(pci_22 p.223。個人ラベルの語 BAR が紙面に無い)。
  manual の 1 件は 1/2。したがって閾値 t を 1/2 より大きく 3/4 以下の範囲で動かしても
  内訳 99+1 は変わらない。これが安全窓 (0.50, 0.75] で、幅は 0.25。
- 現行の閾値 6 割は窓の内側にある。t = 1/2 ちょうどでは(以上判定のため)CPUID list が
  機械照合を通ってしまい、t > 3/4 では pci_22 p.223 が落ちる。閾値の決め直しは不要。

## 引用の実証(acpi_6_4 p.162 Generic Address Structure)

serve + curl で ref から当該ページのチャンクまで辿った手順と結果:

1. GET /v1/refs で ref collections/specs/acpi_6_4 →
   s256:3d5116d4c034037033448c1e93ac4a2f954dfe2ef153d1cc95ed4d3669bc0cec(seq 1)。
2. GET /v1/objects/{doc_rev} で meta = {name: acpi_6_4, media: pdf, extractor:
   "pdftotext 22.02.0"}、chunks 列 2282 個。
3. chunks 列を先頭から辿り(ページは昇順なので 162 超で打ち切り)、meta.page=162 の
   チャンクは添字 418 と 419 の 2 個。
4. 添字 418 の s256:2116f44f3900a559e1e3bad0dc29d3343d2a3b74c3c5fa38e8cee7d46143fffe の
   text に見出し行「5.2.3.2 Generic Address Structure」と本文
   「The Generic Address Structure (GAS) provides the platform with a robust means to
   describe register locations.」が載っている。

文書名(ref パスの残り acpi_6_4)・ページ(meta.page = 162)・本文断片の三つが揃い、
引用が組めた。この注釈自体の照合は一致 3/3(generic, address, structure。数字だけの語
5.2.3.2 は照合の語に入らない)。PDF のチャンクは meta.breadcrumbs を持たない(見出しの
入れ子は Markdown 取り込みだけが写す)。

## correct の実演(試験用コレクション trial)

常用の specs コレクションには訂正を入れず、テスト資材 three_pages.pdf(node/tests/assets)を
コレクション trial に取り込んで実演した。

1. trial/three_pages: chunks=3、注釈 2 件(p.1「Page one of three」と p.2「Page two of
   three」、ともに一致 4/4)を投入。
2. p.1 の annotates 辺
   s256:e45cba76597c6e62685730adcc910ef1007d17ac7afd5862e4285c4e26594c2f を誤り、p.2 の辺
   s256:3ba65482a182ed355f2da7cbd66b11b9a5e1a37dd4b40f1c7057fc8d72fa96e5 を新しい言明として
   `uniqnode correct` を発行。corrects 辺は
   s256:5c69da0ef99aaf211745920416fa7260bc996642bf55112288f28edd350f1064(再照合 一致 4/4)。
   検証記録は p.2 注釈の取り込み時のものと同一内容(同じ method と根拠行)なので
   content-addressing により同一 ID s256:4bd803c1bd658735fed384f053e441a70e3b3eef15a96d625e3a14492693d53d
   に落ちた。
3. GET /v1/objects/{p.1 の辺}/referrers が corrects 辺と新旧の索引オブジェクトの 3 件を
   返し、旧言明側から訂正が見つかることを確認した。

## 総オブジェクト数と総保存量(書き込み経路の段の記録)

GET /v1/status(投入完了後のストア): objects 25443、used_bytes 147220382、last_seq 30。

- オブジェクト数の収支: specs 取り込み 25126 + annotations/specs 302 + trial 取り込み 5 +
  annotations/trial 7 + correct 3 = 25443 で一致。
- 原本 PDF 25 本の合計は 106482898 バイト。used_bytes との差 40737484 バイト(約 39 MiB)が
  抽出テキストのチャンク・doc_rev・注釈類の分で、原本の約 38% 増に相当する。
- ディスク上の実サイズ(du)は 147453871 バイトで、ストア形式のオーバーヘッドは
  used_bytes に対して 233489 バイト(0.16%)。
- チャンクが本文を持つ設計(blob へのオフセット参照にしない)の実コストはこの 39 MiB で
  あり、この規模では問題にならない。

## 結論

INGEST 計画の完了条件のうち手動受け入れの部分は満たされた。PDF 25 本と注釈 100 件
(機械照合 99 + manual 1、不一致 0)が投入でき、引用に物理ページ番号が出て、corrects 辺が
referrers で旧言明側から引ける。閾値 6 割は安全窓 (0.50, 0.75] の内側にあり、決め直しは
不要。予期との差は index.txt が対象外にならず specs/index として入った 1 点だけである。

# 0013 Keep docs split into design/ (current reality) and plan/ (future work), linked by UUID anchors

Level: MUST
Scope: every document under `docs/`, and every reference to one (from other docs, `policy/`, code
comments/strings, host config).

## The split

- `docs/design/*.md` describes the current implementation — plus concepts that don't appear
  directly in the code but form its fundamental direction. Everything in design/ must be true of
  the tree as it is.
- `docs/plan/*.md` describes future or under-consideration work — anything not yet implemented.
- A document that would mix the two MUST be split: the implemented part goes to design/, the
  unimplemented remainder to a same-named file in plan/, the halves cross-linked. No exceptions
  for "minor" remainders — if any part is unimplemented, it lives in plan/.
- When plan work lands, move its description into the design half (and delete the stale plan text
  — must/0010); when a design fact stops being true, fix or delete it immediately (same policy).
- A plan document whose work has fully landed is deleted — the file itself, not just its
  text. Never keep a finished plan as a「履歴記録」— git is the record (must/0010). The same
  applies to individual completed items inside a living plan document: delete them on completion.
- Classification is verified against the code, never assumed from headers — headers drift
  (measured: three docs said 計画/未着手 while the tree shipped the feature).

## State vs. actions（状態と行動の分離）

- design/ は「状態」を書く場である。 システムが今どうであるかを、時制のない現在の記述として
  書く。変更履歴・進捗ログ・実装完了日付の物語・試行錯誤の経過（「実装済み(日付)」「〜を経て」
  「旧 X を置換」の類）を design/ に書いてはならない — 変更の記録は git だけが持つ。
  - 設計判断の背景として過去の事象に触れてよいのは、過去から抽出された、以後も正しい「事実」
    （例: 測定結果とその含意）に限る。プロセスの年代記を書くことは可読性を下げるため禁止する。
- plan/ は「行動」を書く場である。 次に何をするかを、完了条件つきの作業項目として書く。
  現在の状態の説明は、行動を定義するのに必要な最小限にとどめ、状態そのものは design/ への
  リンクで参照する。着地した plan 項目は削除し（must/0010）、design/ 側を現在の状態に更新する。
- どちらの側でも、古くなった情報は削除して最新の情報だけを残す（must/0010 と同一原則）。

## Cross-linking (UUID anchors, not paths)

- Every document carries a stable UUID anchor directly under its H1:
  `<a id="<uuid>"></a>` (uuidgen once, never reused, recorded where the doc is referenced).
- References use the readable form `[docs/design|plan/NAME.md](#<uuid>)` in prose, and the
  path-free form `NAME (uuid:<uuid>)` inside code comments/strings.
- When a reference targets a specific section, add an anchor under that section's heading (same
  `<a id>` form, its own UUID) and point the link there. Section anchors are added on demand —
  exactly when a cross-reference to that section exists — not speculatively. Targets that aren't
  headings (table rows, list items) keep the doc-level anchor with the section named in the link
  text (e.g. `[docs/plan/NEXT.md](#…) #2c`).
- Known limitation, accepted: UUID-only fragment links don't click-resolve across files in
  standard renderers. They are stable, grep-able IDs — `grep -r <uuid>` finds the definition and
  every reference, and survives file moves (that is the point).

Rationale: the 2026-07 reorg found the flat docs/ mixing shipped
reality with stale plans, with path-based links breaking on every move. Split + UUID anchors make
"what is true now" vs "what is intended" mechanically checkable and reference-stable.
Enforcement: `lamalium host check-docs` validates the linking conventions (anchors defined and
unique, every reference resolves, no path links into docs/, no nested-link artifacts) and runs as
step 0 of `host build` — a violation fails the deploy. Anchors and references are written in
lowercase hex: a fragment id is matched literally, so an uppercase one resolves to nothing, and
the checker reports it rather than passing over an id it cannot place in either table. The gate
also reports the documents it could not read and fails on a non-zero count, because a verdict of
zero violations only covers what was actually opened (should/0128). The design/plan classification
itself (is this text true of the tree?) stays review territory.

# 0112 Large foreign-language strings live in their own file, included at compile time

Level: SHOULD
Scope: all code in this system — agent deliverables and harness code alike. Companion to must/0008
(Rust-only tooling: `include_str!` is std, no dependency) and to the SWEEP-SPLIT machinery
([docs/design/SWEEP-SPLIT.md](#cdf29741-6a5a-4439-a9ef-fc8bd725f1c7)).

A large string in another language (HTML, CSS, JS, SQL, a template, test fixture data) does not
belong inline in a Rust source file. Put it in its own file next to the source, named for what it
is (`page.html`, `style.css`), and bind it with a compile-time include:

    pub const PAGE: &str = include_str!("page.html");

"Large" means roughly: more than ~20 lines, or anything that makes the containing `.rs` file harder
to read or edit than the asset itself. When in doubt, externalize.

Rationale — every clause is a failure we actually measured on t-site (2026-07-02):

1. Editability. An asset in its own file is edited in its own language: line-based SEARCH/REPLACE
   works on `page.html` directly, with no raw-string delimiters, no escaping, and no risk that a
   Rust-side edit cuts an HTML tag in half (a committed `/canvas>` fragment came from exactly that).
2. Size. A multi-kilobyte inline string blows the containing file past tool and context windows
   (the 17.8KB inline page kept the whole `.rs` out of the focus window and froze the loop for
   days), while the code around it is a handful of lines.
3. Tooling. `cargo fmt`/`clippy` neither format nor lint the embedded language, but they do trip
   over the container file's size and string syntax; a separate file keeps each tool on its own
   territory.
4. Correctness. `include_str!` binds at compile time: no runtime I/O, no path handling, the binary
   stays self-contained, and a missing asset is a compile error, not a 404.

Enforcement: the gate sweep externalizes oversized inline strings into asset files (SWEEP-SPLIT
Step 3c) the same way it unminifies and splits, so the spirit is mechanical for deliverables — but
write new code this way from the start instead of relying on the sweep to clean up. First-party
Rust is covered by review.

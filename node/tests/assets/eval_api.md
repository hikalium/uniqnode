# Chunker reference

## CHUNK_TOKEN_LIMIT

CHUNK_TOKEN_LIMIT is the largest chunk the chunker emits, measured in approximate tokens.
A single paragraph over that limit is the only case the chunker splits at a character
boundary.

## token_estimate

token_estimate returns the approximate token count of a text. It counts an ASCII character
as a quarter of a token and any other character as a whole one, and the chunker calls
token_estimate for every paragraph it packs.

## chunk_markdown

chunk_markdown splits a document at heading boundaries and packs the paragraphs of one
section up to the limit, so that no chunk spans two headings. The heading path becomes the
breadcrumbs of every chunk in that section.

## pdftotext

pdftotext の版は取り込みの記録に残す。抽出器の名前と版を doc_rev の meta に書き、
どの版で抽出したテキストなのかを後から辿れるようにする。版が変われば抽出結果も
変わりうるためである。

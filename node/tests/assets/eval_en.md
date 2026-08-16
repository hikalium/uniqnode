# Retrieval handbook

## Contents

- Scoring with BM25
- Embedding vectors
- Reciprocal rank fusion
- Chunk boundaries

## Scoring with BM25

Scoring with BM25 rewards a rare term, saturates repeated term frequency, and divides by a
length normalizer so that a long chunk does not win on size alone. The score is meaningful
only inside one response.

## Embedding vectors

An embedding vector places a chunk in a dense space where the cosine similarity between two
vectors stands for closeness in meaning, so a paraphrase that shares no word with the query
can still be found.

## Reciprocal rank fusion

Reciprocal rank fusion merges two ranked lists by summing one over a constant plus the rank
of an entry in each list. It needs no calibration between the scores of the two methods,
because it reads the order and never the score.

## Chunk boundaries

A chunk boundary follows a heading, so one section never merges with the next, and the
heading path travels with the chunk as its breadcrumbs.

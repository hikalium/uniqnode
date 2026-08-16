# Retrieval notes

## Scoring

The ranking function rewards rare terms and saturates repeated term frequency.

## Token estimation

The chunker weighs every token and every estimate separately in this section.

## Chunker internals

The helper token_estimate returns the approximate token count of a text.

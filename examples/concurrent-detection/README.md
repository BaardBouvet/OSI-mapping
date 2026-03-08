# Concurrent Modification Detection

Demonstrates detecting when target systems have been modified independently since the last sync, using base columns in reverse output.

## How it works

1. Setting `include_base: true` on a mapping adds `_base_` columns to reverse output
2. Base columns contain the original source values before resolution
3. Target systems compare current values with base values to detect conflicts
4. Delete views also include base columns for verification before deletion

## Note

`include_base` is a tooling concern — it doesn't change resolution, just adds extra columns to reverse output for optimistic locking.

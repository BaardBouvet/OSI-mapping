# Three-Way Merge

Demonstrates merging across three different data sources using shared identity properties.

## How it works

1. Records from different sources share identity fields (`email`, `phone`)
2. When any two records share an identity value, they're connected
3. Transitive closure finds all connected components across all three sources
4. All records in a component merge into one target entity
5. Strategies like `coalesce` aggregate values from all connected sources

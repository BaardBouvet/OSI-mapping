# Scalar Conversions

Demonstrates bidirectional value transformations between source systems using SQL expressions (date formats, field composition, data normalization).

## How it works

1. `expression` maps source → target with a SQL transform
2. `reverse_expression` maps target → source to reconstruct the original format
3. Multiple sources can map to the same target field with different priorities
4. Round-trip: data survives forward+reverse while keeping each system's format

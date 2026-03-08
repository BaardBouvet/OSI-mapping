# Vocabulary — Standard Codes

Demonstrates vocabulary pattern where different systems use different
representations for the same concept (country names vs ISO codes). Uses late
binding — raw values stored in forward transformation, lookup happens on reverse.

## Benefits

- **Single source of truth**: All country representations come from one vocabulary
- **No redundancy**: Don't need separate mappings for ISO codes vs full names
- **Extensible**: Add new representations (language names, numeric codes) to the vocabulary

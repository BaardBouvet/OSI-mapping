# Minimal Example

The simplest possible configuration showing basic data consolidation from two sources.

## How it works

1. Two source mappings (`companies` and `customers`) map to single target entity `company`
2. Target entity uses different strategies per field: `identity` for email, `last_modified` for name, `coalesce` for account
3. Records with the same email merge into one entity (identity strategy)
4. Reverse transformation regenerates source records with consolidated data

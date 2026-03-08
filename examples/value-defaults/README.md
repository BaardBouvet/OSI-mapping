# Defaults

Demonstrates handling missing data with default values and constant injection
during reverse transformation.

## How it works

1. Target fields can have `default` (literal) or `default_expression` (computed)
2. When a record is created in a new system, defaults fill missing values
3. Field mappings with `expression`/`reverse_expression` inject constant values
   (e.g. `source_system: 'CRM'`, `is_verified: true`)

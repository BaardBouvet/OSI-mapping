# Hello World

The simplest possible mapping: two systems syncing contacts by email.

## How it works

1. CRM and ERP each have contacts — with different column names (`name` vs `contact_name`)
2. `email` is the identity field — it's how records are matched across systems
3. `name` uses `coalesce` — the first non-null value wins (CRM priority 1 beats ERP priority 2)
4. After sync, both systems agree on the same name

## What the tests show

| Test | Scenario |
|---|---|
| 1 | Shared contact merges — CRM value wins, ERP gets updated |
| 2 | Contact exists only in CRM → insert into ERP |
| 3 | Contact exists only in ERP → insert into CRM |

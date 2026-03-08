# Composite Merge (Merge Groups)

Demonstrates composite merge keys using `link_group`. Fields with the same link_group must ALL match for entities to merge (AND logic). Separate groups or individual identity fields use OR logic.

## How it works

1. `link_group: "name"` on first_name + last_name — both must match as a tuple
2. `email` is standalone identity — matches alone
3. Records merge if: (first_name AND last_name match) OR (email matches)
4. Bob Smith and Bob Davis don't merge despite sharing first name

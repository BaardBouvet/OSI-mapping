# Grouped Properties

Fields with the same `group` resolve together using the newest timestamp from any field in the group. Ungrouped fields resolve independently.

## How it works

1. `group: "address"` on street + zip means they resolve as a unit
2. The group picks the source with the max timestamp across any field in the group
3. `name` has no group — resolves independently by its own timestamp
4. Two rows with the same customer_id: row 1 has newer address group, row 2 has newer name

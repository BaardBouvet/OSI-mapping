# Generated IDs

Links records from three systems using generated IDs captured in linkage tables during sync.

## How it works

1. Each system has its own user table mapped to the `user` target
2. Linkage tables record which IDs were generated during cross-system sync
3. All system IDs use identity strategy — linkage rows connect them transitively
4. Records without linkage remain isolated

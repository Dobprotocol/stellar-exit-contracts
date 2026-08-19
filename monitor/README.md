# Exit layer monitor

Indexes the four exit-layer contracts' Soroban events into an append-only log,
and rebuilds a derived snapshot from that log on every pass.

```bash
npm install
node index.js            # one pass, then exit
node index.js --reset    # forget the cursor and re-derive; the log is kept
```

Writes three files to `EXIT_DATA_DIR` (default `./.data`, gitignored):

| File | What it is |
|---|---|
| `events.jsonl` | The record. Append-only, one decoded event per line, in ledger order. |
| `state.json` | Cursor, last ledger, and whether coverage is complete. |
| `snapshot.json` | Aggregates, rebuilt from the log each pass. Disposable. |

## Why there is a monitor at all

The contracts answer questions about *now*. `get_appetite` tells you a node's
standing terms, `quote_capacity` tells you what it could field this minute,
`list` tells you who is in the queue. None of them can tell you what happened,
and two things the layer needs are only answerable from history:

* **Who the Liquidity Nodes are.** There is no node registry, for the same
  reason there is no asset registry — a node becomes real by acting. Until
  this log existed, the app discovered nodes by scraping `best_node` off live
  exits, which meant a node that posted appetite and never won a bid was
  invisible. `appetite_set` is in the log, so now it is not.
* **Anything with a time axis.** Volume, realised discounts, how long exits sit
  in the queue, fees to the treasury. A chain read has no memory.

## Design notes, and the mistakes they come from

**One request covers all four contracts.** The RPC's contract filter takes up
to five ids and the layer is exactly four, so a page is one call. The EVM
indexer asked per pool, kept a cursor per pool, and took the `min()` across
them — so every pass discarded the progress of whichever pool was ahead. That
is the treadmill this avoids.

**The cursor is saved per page, not per pass.** The same indexer saved only
when a pass completed, and the `*/5` cron killed every long scan before it
got there, so it restarted from the same ledger forever. A page is the
smallest unit of progress the RPC hands back; it is the right unit to persist.

**A pass is a loop, not a call.** `getEvents` scans a bounded window of ledgers
— about 10,000 on testnet — and returns a cursor even when the page is empty.
Reaching the tip from a cold start takes a dozen or so calls.

**The log is append-only and never rewritten.** Stellar has no reorgs; SCP
finalises a ledger when it closes. A cursor only moves forward and an event
never has to be un-indexed.

**The snapshot is rebuilt from scratch.** An incrementally-updated aggregate
drifts from its source the first time a pass dies halfway, and then nothing
derived from it can be trusted without re-deriving anyway.

**Coverage is declared, not assumed.** The RPC keeps roughly a week of events.
`coverage.complete` is true only if indexing started at or before the oldest
ledger it still holds; false means there is history nobody saw, and the counts
are a floor rather than a census.

## What the snapshot is not

It is not a substitute for reading the chain. Balances, appetites and capacity
have authoritative getters, and those getters know about state the log cannot
see — an appetite too young to back a bid, capital freed by a withdrawal.
`nodes[].depositedAtLastReport` is the sharp edge: `paid_out` reduces a node's
deposited capital without emitting a new total, so a node that has filled
anything reads high. Subtract `usdcPaidOut`, or ask `vault.get_node`.

Amounts are decimal strings in the token's own base units — USDC at 7 decimals,
the demo RWAs at 0. Nothing here divides by 10^7 for you, on purpose: an audit
log that quietly rounds is not one.

## Adding a network

`config.js` is the whole of it: point `rpcUrl` at another RPC and replace the
four contract ids. Use a separate `EXIT_DATA_DIR` per network — a log mixing
two chains cannot be untangled afterwards.

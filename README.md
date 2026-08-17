# stellar-exit-contracts

Soroban contracts for the **DobDex exit layer** — the facility that lets a holder of a
tokenized real-world asset sell their position immediately, to a Liquidity Node, at a
price the Node quotes.

Settlement is **direct**. The seller's participation tokens go to the winning Node and
USDC goes to the seller, net of the discount and the protocol fee, in one atomic Soroban
transaction. There is no synthetic asset, no mint, no collateral pool and no redemption
step: the discount is paid by the seller, to the counterparty, and nowhere else.

> Reference implementation of an earlier, different design (an RWA vault minting a
> synthetic against deposits) lives in [`Dobprotocol/Dobhooks`](https://github.com/Dobprotocol/Dobhooks)
> on EVM testnets. It is prior art, not this.

## What this is not

It is not an AMM and not a central limit order book. It is a **quote-driven dealer
market**: bids only, priced as a discount off a reference, evaluated at fill time, with
no matching engine and no time priority. A holder who is not in a hurry should use the
primary market's own listing book instead and keep the spread.

## Contracts

| Contract | Owns | Error codes | Tests |
|---|---|---|---|
| `lp_vault` | Liquidity Node capital and the standing terms each Node offers per asset | 1–99 | 17 |
| `settlement_router` | Escrow, and the single atomic transfer that clears an exit | 100–199 | 12 |
| `fifo_queue` | Exits with no acceptable bid, in a public, unjumpable line | 200–299 | 7 |
| `exit_auction` | The exit lifecycle: open, bid, accept, close, cancel | 300–399 | 15 |

Error codes are disjoint on purpose. A vault refusal that surfaces through an auction
call keeps its own number instead of decoding as the auction's error with the same
value — so a failed bid always says *which* contract said no, and why.

`testnet/faucet` is a fifth contract and deliberately not one of these. The test tokens mint
only for their admin, so nobody could try the layer with an empty wallet; the faucet holds
that admin and hands out a drip on a cooldown. It sits outside `contracts/`, uses codes
900–999, and has no business on mainnet.

### How they fit together

```
              exit_auction ─── decides who fills and at what price
                   │
      ┌────────────┼────────────┐
      ▼            ▼            ▼
  lp_vault   settlement_router   fifo_queue
  capital &   escrow + the only   the public
  standing    contract that       waiting line
  terms       moves value
```

The auction is the only caller any of the other three accept. It holds no funds itself:
the vault knows whether a Node's capital is really there, the router performs the
transfer or reverts, and the queue records who was waiting first.

### Three rules the auction enforces

* **Price-only priority.** The highest gross bid wins. No size preference, no fee tier,
  no privileged Node. Matching the standing bid is not beating it.
* **The line is respected at fill time.** While an asset has a queue, only the exit at
  its head can settle. Bidding on the others stays open — nobody buys their way past a
  seller who has been waiting.
* **The seller is never obligated.** A bid is an offer; accepting it is a separate
  signature. Cancelling costs the seller nothing and pays the bidder nothing.

Bids are **absolute USDC amounts**, not discounts. Whatever reference the seller declared
and whatever any price feed says, a Node can only ever be held to the number it named
itself — the discount in the events is derived for readability, not for settlement.

### Where pricing lives

Off-chain, in the Node. Each Node decides what it will pay and why — the asset's TRUFA
validation score, its own model, whatever the primary market is doing. The chain enforces
only what the Node committed to: a floor discount, an exposure ceiling, and enough real
USDC to honour the bid.

That is why there is no risk-scoring contract here. The score travels with the asset; it
does not need to be re-derived on-chain to price a trade.

### The two limits in `lp_vault`

They are not the same kind of limit, and conflating them is how vaults go insolvent:

* **Free balance** — `deposited − committed − pending_withdrawal`. Real USDC held by the
  contract. It is the solvency invariant, and nothing bypasses it.
* **Exposure** — a Node's self-imposed ceiling per asset. It keeps counting after a payout
  because the Node is then holding the asset, and only the Node can mark a position
  divested. A Node lying to itself here risks its own capital and can never make the
  vault insolvent.

## Build & test

```bash
cargo test                     # all four contracts, 51 tests (+11 for the faucet)
cargo test -p lp-vault         # one

stellar contract build         # wasm for deployment -> target/wasm32v1-none/release/
```

Build the wasm with `stellar contract build`, not `cargo build --target wasm32-unknown-unknown`.
Current Rust emits the reference-types proposal for that target, which the Soroban VM rejects at
upload time with `Module(Translation(... "reference-types not enabled" ...))`. The `wasm32v1-none`
target the CLI selects does not.

The contracts talk to each other through declared client interfaces rather than crate
imports, so no contract's code is compiled into another's wasm. The test suite links the
real ones and runs the whole layer end to end.

`Cargo.lock` is committed on purpose — reproducible builds are the point of SEP-55
attestation.

## Networks

| Network | ID | RPC |
|---|---|---|
| Testnet | 9 | `https://soroban-testnet.stellar.org` |
| Mainnet | 10 | `https://mainnet.sorobanrpc.com` |

Live on **testnet** since 2026-08-17 — contract IDs, wiring, and the demo state in
[`DEPLOYMENTS.md`](DEPLOYMENTS.md). Nothing is deployed to mainnet.

## License

Apache-2.0

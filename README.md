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

| Contract | Owns | Status |
|---|---|---|
| `lp_vault` | Liquidity Node capital and the standing terms each Node offers per asset | **built, 17 tests** |
| `exit_auction` | The exit lifecycle: open, bid, close | in build |
| `settlement_router` | Escrow, and the single atomic transfer that clears an exit | in build |
| `fifo_queue` | Exits with no acceptable bid, in a public, unjumpable line | in build |

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
cargo test                     # all contracts
cargo test -p lp-vault         # one

cargo build --release --target wasm32-unknown-unknown -p lp-vault
```

`Cargo.lock` is committed on purpose — reproducible builds are the point of SEP-55
attestation.

## Networks

| Network | ID | RPC |
|---|---|---|
| Testnet | 9 | `https://soroban-testnet.stellar.org` |
| Mainnet | 10 | `https://mainnet.sorobanrpc.com` |

Nothing is deployed yet.

## License

Apache-2.0

# Deployments

## Testnet (network id 9, `Test SDF Network ; September 2015`)

Deployed 2026-08-17 from commit `HEAD` of this repo, built with `stellar contract build`.

### Exit layer

| Contract | Contract ID | Wasm hash |
|---|---|---|
| `lp_vault` | `CDLCHH257PNQ5YFW2R2T2SUZAFRI525LCD75CBBAFR2XRR272Z5QC5UA` | `404b466fce70228f50b032b37b7453af11ce4f968d7ef3d206fbcaa90fcca7a9` |
| `settlement_router` | `CABQVSEKSDWC7GG2LFGZFMPIPY674BID7BRQDOGKENUVLHKJHEDES4EG` | `78bc6a8f614201325148f52027532e3f301d8cdefb1bc9b1cfe3e79828035f73` |
| `fifo_queue` | `CBBYPWMDIQHVGOQP6244IC2R6HWEDBNRBYGA2FNEIQL63QJUFY33QINF` | `c50dd6f00dc6a47aaf688833f2ffc52ba56f40f8269276545473e0b4f80e603d` |
| `exit_auction` | `CBZE6QZHCEKS23E3SIUBGILTGSPHHUCLKAPVSJT4C5Q3IHHE57QPMMZ3` | `66a8a36369dfaa3347b42dc2c5be14c1a4a90746372265fdacf6ba3ce9b5a97c` |

### Test tokens

Both are instances of the audited `participation_token` wasm
(`ff9d2b266a04c7ecd3b66f307bf8658994b79ac644013e32e308a28cadbc1c7e`, release `pt-v0.2.0`
of `stellar-distribution-contracts`). A Soroban-native token rather than a classic asset
SAC, so holders need no trustline.

| Token | Contract ID | Decimals |
|---|---|---|
| Test USDC | `CCPNESXPESH5F7WOOWYPLPTLK2PICRP7DXGNGJPPH3YJ3XZVEGKAJL2I` | 7 |
| Solar Farm Alpha (SFA) — the demo RWA | `CDPHF6JRRPAG2ZHEQCR3CG4JYBYDAEMPYIDSIWGGVUEJNRWAYG2E7VTS` | 0 |
| EV Charging Hub Beta (ECH) — second demo RWA, added 2026-08-18 | `CCY2BBNDFHRXM76SJT64BYJ4YSOBSK6V6SX6WG3S57RJINFZNWKOB53X` | 0 |

A single asset makes an asset-agnostic layer look asset-specific: with one token
on screen there is no way to tell whether a queue, an appetite or a balance is
per-asset or global. ECH exists so the app has two of them. It was deployed the
same way as SFA — same wasm, `initialize` under the deployer, 10,000 minted to
the seller, then the mint handed to the faucet — and both nodes carry a standing
appetite on it (A: 100,000 ceiling / 250 bps floor, B: 80,000 / 400 bps), so it
is biddable and not just displayable.

**These are test tokens.** They carry no value and are not the real USDC SAC. Mainnet USDC
is `CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75`.

### Faucet — testnet scaffolding, not part of the layer

`participation_token.mint` is admin-gated, so a person arriving with an empty wallet could
not obtain either token and had nothing to sell or bid with. The faucet holds the **admin of
both tokens** and hands out a drip on a cooldown to anyone who signs for it. It lives in
`testnet/faucet/`, outside `contracts/`, and its error codes are 900–999 — clear of the
layer's ranges. Nothing about it is intended for mainnet.

| Contract ID | Cooldown | Drip |
|---|---|---|
| `CAJHMAFYVO7LDRUDQFYIJYSVXR6UJL55AR7LLU6EEWDFUVQSMWO6YV4G` | 3,600s per wallet per token | 10,000 USDC · 1,000 SFA · 1,000 ECH |

`tokens()` is the tap's own list, and the app reads it rather than carrying one,
so `set_drip` is the whole of adding a token — no page change was needed for ECH.

```bash
stellar contract invoke --id CAJHMAFYVO7LDRUDQFYIJYSVXR6UJL55AR7LLU6EEWDFUVQSMWO6YV4G \
  --source you --network testnet -- \
  claim --to $(stellar keys address you) --token CCPNESXPESH5F7WOOWYPLPTLK2PICRP7DXGNGJPPH3YJ3XZVEGKAJL2I
```

A second claim inside the hour returns `Error(Contract, #904)` (`TooSoon`). `set_token_admin`
hands the mint back if the faucet ever has to be replaced — the tokens themselves can only be
initialized once, so that escape hatch is the only way to recover from a bad faucet.

### Configuration as deployed

* Admin and treasury: `GACZ5Q42IZTOIOWSJR4B5SIQ45C4C5VVXEKXRFQ4N2GUWTAN2DOLYLWZ`
* Protocol fee: **100 bps**, against a `MAX_PROTOCOL_FEE_BPS` of 500 enforced on-chain
* `min_deposit`: 1 USDC · `max_single_fill_bps`: 3000 (default) · `withdraw_timelock`: default

Wiring — the auction is the only caller the other three accept:

```
lp_vault.set_auction(exit_auction)     lp_vault.set_router(settlement_router)
settlement_router.set_auction(exit_auction)
fifo_queue.set_auction(exit_auction)
```

### Demo state on testnet

Updated 2026-08-18, after seven exits across the two demo assets.

| Role | Account | State |
|---|---|---|
| Liquidity Node A | `GCZFZW64L62XPRMGA6I5ITGPRXCLMQ63DE3VH4CRLNBFL7NFQ2AUZS3W` | 207,500 USDC deposited, 192,500 of it filled · holds 1,500 SFA + 500 ECH · appetite on SFA: 200,000 ceiling, 300 bps floor, 144,500 exposed · on ECH: 100,000 ceiling, 250 bps floor, 48,000 exposed |
| Liquidity Node B | `GBKP7AT3WUNEUAGTUIEUEQDKNTW7LFYOLFFPHEFBF5JTZFEUSZAAL7LU` | 250,000 USDC deposited, 141,850 committed against live bids · appetite on SFA: 150,000 ceiling, 450 bps floor, 66,850 exposed · on ECH: 80,000 ceiling, 400 bps floor, 75,000 exposed |
| Seller | `GDMETA6S2CSA7CRYGKTV24LBNKOBMU2UKKTRAE224J4ZRUDLMX6UPX7Q` | 7,500 SFA · 8,500 ECH · 190,575 USDC from the three settlements |

Every exit the auction has seen, and the state it is in. Amounts are USDC; the fee is 100 bps
of the gross in all three settlements.

| # | Asset | Amount | Reference | Floor | State |
|---|---|---|---|---|---|
| 1 | SFA | 1,000 | 100,000 | 94,000 | **Settled** at 96,000 to Node A — seller 95,040, treasury 960 |
| 2 | SFA | 500 | 50,000 | 47,000 | **Settled** at 48,500 to Node A — seller 48,015, treasury 485 |
| 3 | SFA | 700 | 70,000 | 63,000 | **Open** — Node B standing at 66,850, above the floor, window still running |
| 4 | SFA | 300 | 30,000 | 29,500 | **Queued** — the window closed with no bid, so it took position 0 in the FIFO |
| 5 | ECH | 1,000 | 100,000 | 92,000 | **Open** — Node B standing at 75,000, *below* the seller's floor. Nothing is force-filled |
| 6 | ECH | 500 | 50,000 | 45,000 | **Settled** at 48,000 to Node A — seller 47,520, treasury 480 |
| 7 | ECH | 150 | 15,000 | 14,000 | **Cancelled** by the seller before any bid arrived; the 150 ECH went back |

Exit #4 is the case worth understanding: no Node wanted it at 29,500 on a 30,000 reference —
a 167 bps discount, tighter than either Node's floor — so the window simply expired and the
position took a public place in line instead of being sold at a price the seller refused.
`close` only works once `ledger.timestamp() > closes_at`, and the ledger clock lags wall clock
by a few seconds; calling it exactly at the deadline returns `Error(Contract, #322)`
(`StillBidding`). Wait, then call again.

Exit #5 is the other one: a real bid on the table that the seller's floor rejects. The exit
stays open — the contract does not talk the seller down and does not talk the Node up.

### What exit #1 proved

Two refusals on the way there, both correct, and worth keeping because they are the design
working rather than failing:

* Node B bid **96,000 to match** Node A → `Error(Contract, #323)`, `exit_auction::BidTooLow`.
  Matching the standing bid is not beating it.
* Node B then bid **96,500 to beat it** → `Error(Contract, #23)`, `lp_vault::DiscountBelowFloor`.
  96,500 against a 100,000 reference is a 350 bps discount and B's own floor is 450 bps. B
  cannot be talked into paying more than its standing terms allow — not even by itself.

Consecutive `place_bid` calls, refused by two different contracts, each keeping its own
number. That is what the disjoint ranges buy.

Node A's exposure stayed at 96,000 **after** the payout. That is deliberate: A now holds the
asset, and only A can mark the position divested.

### Two more refusals, from the ECH round

Both are the vault keeping its own numbers, surfacing through an `exit_auction` call:

* Node A bid **97,500** on exit #5 → `Error(Contract, #30)`, `lp_vault::SingleFillCapExceeded`.
  `max_single_fill_bps` is 3,000, so one exit may take at most 30% of A's 255,500 capital at
  the time — 76,650. A wanting the position does not raise A's own concentration limit.
* Node B bid **48,000** on exit #6 → `Error(Contract, #22)`, `lp_vault::ExposureExceeded`.
  B had 75,000 of its 80,000 ECH ceiling already committed to its exit #5 bid. The standing
  bid is real money reserved, not an intention.

### `MIN_BACKING_AGE` gates the first bid

A node cannot back a bid until its appetite is `MIN_BACKING_AGE` (3,600s) old. Immediately
after `set_appetite`, `place_bid` fails with:

```
HostError: Error(Contract, #24)      // lp_vault::BackingTooYoung
["contract call failed", commit, [node, asset, 960000000000, 400]]
```

That is the layer working, not a deploy fault — and it is worth reading closely, because it
is the disjoint-error-range design paying off: the code is **24**, from `lp_vault`'s own
1–99 range, surfacing through an `exit_auction` call without being decoded as auction error
24. A refusal says which contract refused.

Any script that deploys and immediately settles will hit this. Set appetites, wait the hour,
then bid.

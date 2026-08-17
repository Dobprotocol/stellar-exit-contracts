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
| `CAJHMAFYVO7LDRUDQFYIJYSVXR6UJL55AR7LLU6EEWDFUVQSMWO6YV4G` | 3,600s per wallet per token | 10,000 USDC · 1,000 SFA |

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

| Role | Account | State |
|---|---|---|
| Liquidity Node A | `GCZFZW64L62XPRMGA6I5ITGPRXCLMQ63DE3VH4CRLNBFL7NFQ2AUZS3W` | 304,000 USDC deposited · 1,000 SFA held · appetite on SFA: 200,000 ceiling, 300 bps floor, 96,000 exposed |
| Liquidity Node B | `GBKP7AT3WUNEUAGTUIEUEQDKNTW7LFYOLFFPHEFBF5JTZFEUSZAAL7LU` | 250,000 USDC deposited · appetite on SFA: 150,000 ceiling, 450 bps floor, nothing exposed |
| Seller | `GDMETA6S2CSA7CRYGKTV24LBNKOBMU2UKKTRAE224J4ZRUDLMX6UPX7Q` | 9,000 SFA · 95,040 USDC from exit #1 |

Exit #1 was 1,000 SFA against a 100,000 USDC reference with a 94,000 floor. It settled at
96,000 to Node A: seller 95,040, treasury 960 (the 100 bps fee), Node A the 1,000 SFA.
`status` is now 1.

### What the settlement proved

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

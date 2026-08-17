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

**These are test tokens with an open mint, held by the deployer.** They carry no value and
are not the real USDC SAC. Mainnet USDC is `CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75`.

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
| Liquidity Node A | `GCZFZW64L62XPRMGA6I5ITGPRXCLMQ63DE3VH4CRLNBFL7NFQ2AUZS3W` | 400,000 USDC deposited · appetite on SFA: 200,000 ceiling, 300 bps floor |
| Liquidity Node B | `GBKP7AT3WUNEUAGTUIEUEQDKNTW7LFYOLFFPHEFBF5JTZFEUSZAAL7LU` | 250,000 USDC deposited · appetite on SFA: 150,000 ceiling, 450 bps floor |
| Seller | `GDMETA6S2CSA7CRYGKTV24LBNKOBMU2UKKTRAE224J4ZRUDLMX6UPX7Q` | 10,000 SFA minted; 1,000 escrowed in exit #1 |

Exit #1 is open: 1,000 SFA, reference 100,000 USDC, floor 94,000 USDC, 24h window.

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

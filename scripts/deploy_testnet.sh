#!/usr/bin/env bash
#
# Deploy the exit layer to Stellar testnet, wire it, and stand up demo state.
#
# Idempotent it is not: every run deploys a fresh set of contracts. Re-running it
# gives you new contract IDs, not an upgrade of the old ones.
#
#   ./scripts/deploy_testnet.sh
#
# Needs: stellar-cli 23.x, a funded testnet identity, and the identities named in
# ROLES below. Create and fund one with `stellar keys generate <name> --network testnet`.
set -euo pipefail

NETWORK=testnet
DEPLOYER=${DEPLOYER:-deployer}      # admin + treasury
NODE_A=${NODE_A:-sh1}
NODE_B=${NODE_B:-sh2}
SELLER=${SELLER:-sh3}

# The audited participation_token, already installed on testnet (pt-v0.2.0 of
# stellar-distribution-contracts). Soroban-native, so no trustlines.
TOKEN_WASM_HASH=ff9d2b266a04c7ecd3b66f307bf8658994b79ac644013e32e308a28cadbc1c7e

cd "$(dirname "$0")/.."

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
addr() { stellar keys address "$1"; }
inv() { stellar contract invoke --id "$1" --source "$2" --network $NETWORK -- "${@:3}"; }

# ── build ───────────────────────────────────────────────────────────────────
# Not `cargo build --target wasm32-unknown-unknown`: current Rust emits
# reference-types there and the Soroban VM rejects the upload.
say "build"
stellar contract build >/dev/null
OUT=target/wasm32v1-none/release

say "upload"
declare -A HASH
for c in lp_vault settlement_router fifo_queue exit_auction; do
  HASH[$c]=$(stellar contract upload --wasm "$OUT/$c.wasm" --source "$DEPLOYER" --network $NETWORK | tail -1)
  echo "  $c ${HASH[$c]}"
done

# ── tokens ──────────────────────────────────────────────────────────────────
say "test tokens"
ADMIN=$(addr "$DEPLOYER")
USDC=$(stellar contract deploy --wasm-hash $TOKEN_WASM_HASH --source "$DEPLOYER" --network $NETWORK | tail -1)
RWA=$(stellar contract deploy  --wasm-hash $TOKEN_WASM_HASH --source "$DEPLOYER" --network $NETWORK | tail -1)
inv "$USDC" "$DEPLOYER" initialize --admin "$ADMIN" --decimal 7 --name "USD Coin (test)"  --symbol USDC >/dev/null
inv "$RWA"  "$DEPLOYER" initialize --admin "$ADMIN" --decimal 0 --name "Solar Farm Alpha" --symbol SFA  >/dev/null
echo "  USDC $USDC"
echo "  RWA  $RWA"

# ── deploy ──────────────────────────────────────────────────────────────────
say "deploy"
VAULT=$(stellar   contract deploy --wasm-hash "${HASH[lp_vault]}"          --source "$DEPLOYER" --network $NETWORK | tail -1)
ROUTER=$(stellar  contract deploy --wasm-hash "${HASH[settlement_router]}" --source "$DEPLOYER" --network $NETWORK | tail -1)
QUEUE=$(stellar   contract deploy --wasm-hash "${HASH[fifo_queue]}"        --source "$DEPLOYER" --network $NETWORK | tail -1)
AUCTION=$(stellar contract deploy --wasm-hash "${HASH[exit_auction]}"      --source "$DEPLOYER" --network $NETWORK | tail -1)
echo "  vault   $VAULT"
echo "  router  $ROUTER"
echo "  queue   $QUEUE"
echo "  auction $AUCTION"

say "initialize"
inv "$VAULT"   "$DEPLOYER" initialize --admin "$ADMIN" --usdc "$USDC" --min_deposit 10000000 >/dev/null
inv "$QUEUE"   "$DEPLOYER" initialize --admin "$ADMIN" >/dev/null
inv "$ROUTER"  "$DEPLOYER" initialize --admin "$ADMIN" --vault "$VAULT" --usdc "$USDC" \
                                      --treasury "$ADMIN" --protocol_fee_bps 100 >/dev/null
inv "$AUCTION" "$DEPLOYER" initialize --admin "$ADMIN" --vault "$VAULT" --router "$ROUTER" --queue "$QUEUE" >/dev/null

# The auction is the only caller the other three accept.
say "wire"
inv "$VAULT"  "$DEPLOYER" set_auction --auction "$AUCTION" >/dev/null
inv "$VAULT"  "$DEPLOYER" set_router  --router  "$ROUTER"  >/dev/null
inv "$ROUTER" "$DEPLOYER" set_auction --auction "$AUCTION" >/dev/null
inv "$QUEUE"  "$DEPLOYER" set_auction --auction "$AUCTION" >/dev/null

# ── demo state ──────────────────────────────────────────────────────────────
say "demo state"
A=$(addr "$NODE_A"); B=$(addr "$NODE_B"); S=$(addr "$SELLER")
inv "$USDC" "$DEPLOYER" mint --to "$A" --amount 5000000000000 >/dev/null   # 500k
inv "$USDC" "$DEPLOYER" mint --to "$B" --amount 3000000000000 >/dev/null   # 300k
inv "$RWA"  "$DEPLOYER" mint --to "$S" --amount 10000 >/dev/null           # 10k shares

inv "$VAULT" "$NODE_A" deposit --node "$A" --amount 4000000000000 >/dev/null
inv "$VAULT" "$NODE_B" deposit --node "$B" --amount 2500000000000 >/dev/null
inv "$VAULT" "$NODE_A" set_appetite --node "$A" --asset "$RWA" \
     --max_exposure 2000000000000 --min_discount_bps 300 --active true >/dev/null
inv "$VAULT" "$NODE_B" set_appetite --node "$B" --asset "$RWA" \
     --max_exposure 1500000000000 --min_discount_bps 450 --active true >/dev/null

inv "$AUCTION" "$SELLER" open_exit --seller "$S" --asset "$RWA" --amount 1000 \
     --reference_usdc 1000000000000 --min_accept_usdc 940000000000 --duration 86400 >/dev/null

cat <<EOF

Done.

  lp_vault           $VAULT
  settlement_router  $ROUTER
  fifo_queue         $QUEUE
  exit_auction       $AUCTION
  USDC (test)        $USDC
  RWA  (test)        $RWA

Exit #1 is open. Bidding on it fails with Error(Contract, #24) — lp_vault's
BackingTooYoung — until the appetites set above are MIN_BACKING_AGE (1h) old.
That is the layer working, not a broken deploy. Wait the hour, then:

  stellar contract invoke --id $AUCTION --source $NODE_A --network testnet -- \\
    place_bid --node $A --exit_id 1 --usdc_amount 960000000000
EOF

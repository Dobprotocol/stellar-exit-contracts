'use strict';

/* Everything the monitor needs to know about the chain it is watching.
   The contract ids are the testnet deployment recorded in DEPLOYMENTS.md;
   point RPC_URL and the four ids at another network and nothing else changes. */

const path = require('path');

const env = process.env;

module.exports = {
  rpcUrl: env.EXIT_RPC_URL || 'https://soroban-testnet.stellar.org',
  networkId: Number(env.EXIT_NETWORK_ID || 9),

  /* One filter, four contracts, one request. Asking per contract is what put
     the EVM indexer on a treadmill: n requests per pass, each with its own
     cursor, and a `min()` across them that threw away the progress of the
     ones that were ahead. The RPC accepts up to five contract ids in a single
     contract filter, and the exit layer is exactly four. */
  contracts: {
    CDLCHH257PNQ5YFW2R2T2SUZAFRI525LCD75CBBAFR2XRR272Z5QC5UA: 'lp_vault',
    CABQVSEKSDWC7GG2LFGZFMPIPY674BID7BRQDOGKENUVLHKJHEDES4EG: 'settlement_router',
    CBBYPWMDIQHVGOQP6244IC2R6HWEDBNRBYGA2FNEIQL63QJUFY33QINF: 'fifo_queue',
    CBZE6QZHCEKS23E3SIUBGILTGSPHHUCLKAPVSJT4C5Q3IHHE57QPMMZ3: 'exit_auction',
  },

  /* Named so the snapshot can say "Solar Farm Alpha" instead of a C-address.
     Purely cosmetic: an asset absent from here is still indexed, it just
     shows as its contract id. The layer has no asset registry on purpose. */
  assetNames: {
    CDPHF6JRRPAG2ZHEQCR3CG4JYBYDAEMPYIDSIWGGVUEJNRWAYG2E7VTS: 'SFA',
    CCY2BBNDFHRXM76SJT64BYJ4YSOBSK6V6SX6WG3S57RJINFZNWKOB53X: 'ECH',
    CCPNESXPESH5F7WOOWYPLPTLK2PICRP7DXGNGJPPH3YJ3XZVEGKAJL2I: 'USDC',
  },

  /* Outside the git tree by default. The log is operational data: it must
     survive a `git pull` on prod and must never turn a deploy into a merge
     conflict. */
  dataDir: env.EXIT_DATA_DIR || path.join(__dirname, '.data'),

  /* The RPC answers a getEvents call by scanning a bounded window of ledgers
     — around 10,000 on testnet — and hands back a cursor even when it found
     nothing. So a pass is a loop, not a call, and this caps how long that
     loop may run before the next cron tick would overlap it. */
  maxPagesPerPass: Number(env.EXIT_MAX_PAGES || 400),
  pageLimit: Number(env.EXIT_PAGE_LIMIT || 200),
  requestTimeoutMs: Number(env.EXIT_TIMEOUT_MS || 30000),
};

module.exports = {
  apps: [{
    name: 'exit-monitor',
    script: 'index.js',
    cwd: __dirname,
    /* A cron job, not a daemon. The script runs one pass and exits; PM2 starts
       it again on the next tick. `autorestart` MUST stay false — with it on,
       PM2 relaunches the moment the process exits and the cron schedule turns
       into a busy loop against the RPC. Same shape as evm-sync, and the same
       five-minute cadence as the Stellar mainnet sync. */
    autorestart: false,
    cron_restart: '*/5 * * * *',
    env: {
      NODE_ENV: 'production',
      EXIT_RPC_URL: 'https://soroban-testnet.stellar.org',
      EXIT_NETWORK_ID: '9',
      /* Outside /opt/stellar-exit-contracts on purpose. */
      EXIT_DATA_DIR: '/var/lib/dobdex-exit-monitor',
    },
    max_memory_restart: '200M',
  }],
};

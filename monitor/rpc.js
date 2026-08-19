'use strict';

const cfg = require('./config');

async function call(method, params) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), cfg.requestTimeoutMs);
  try {
    const res = await fetch(cfg.rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params: params || {} }),
      signal: ctrl.signal,
    });
    if (!res.ok) throw new Error(`${method}: HTTP ${res.status}`);
    const body = await res.json();
    if (body.error) {
      const err = new Error(`${method}: ${body.error.message}`);
      err.rpcCode = body.error.code;
      err.rpcMessage = body.error.message;
      throw err;
    }
    return body.result;
  } finally {
    clearTimeout(timer);
  }
}

const getLatestLedger = () => call('getLatestLedger');

/**
 * One page of events for all four contracts.
 *
 * `startLedger` and `cursor` are mutually exclusive — the RPC rejects a
 * request carrying both, which is why the caller passes exactly one.
 */
function getEvents({ startLedger, cursor }) {
  const pagination = { limit: cfg.pageLimit };
  if (cursor) pagination.cursor = cursor;
  const params = {
    filters: [{ type: 'contract', contractIds: Object.keys(cfg.contracts) }],
    pagination,
    xdrFormat: 'base64',
  };
  if (!cursor) params.startLedger = startLedger;
  return call('getEvents', params);
}

/* The retention window moves, and asking for a ledger that fell out of it is
   an error, not an empty page. The cheapest way to learn the current floor is
   to ask for one event at the tip: every getEvents reply carries oldestLedger. */
async function retention() {
  const { sequence } = await getLatestLedger();
  const r = await call('getEvents', {
    startLedger: sequence,
    filters: [{ type: 'contract', contractIds: Object.keys(cfg.contracts) }],
    pagination: { limit: 1 },
  });
  return { latest: r.latestLedger || sequence, oldest: r.oldestLedger };
}

module.exports = { call, getLatestLedger, getEvents, retention };

'use strict';

const cfg = require('./config');

/* The snapshot is a pure function of the log and is rebuilt from scratch every
 * pass. That is deliberate: an incrementally-updated aggregate drifts from its
 * source the first time a pass dies halfway, and then nothing on the page can
 * be trusted without re-deriving it anyway. Rebuilding costs a linear scan of
 * a file measured in tens of thousands of lines — cheaper than the class of
 * bug it removes.
 *
 * What it does NOT do is replace a chain read. Balances, appetites and
 * capacity have authoritative getters on the vault, and those getters know
 * about state the log cannot see (an appetite whose `backed_at` has not aged,
 * a free balance moved by a withdrawal). Everything here is history: counts,
 * volumes, timings, and the roster of who has ever acted — which is the one
 * thing a contract read genuinely cannot give you, because the layer has no
 * registry of nodes or assets by design.
 */

const big = (v) => { try { return BigInt(v || 0); } catch { return 0n; } };
const s = (v) => v.toString();

function blankAsset(id) {
  return {
    asset: id,
    name: cfg.assetNames[id] || null,
    exitsOpened: 0,
    settled: 0,
    cancelled: 0,
    queued: 0,
    refunded: 0,
    bids: 0,
    usdcSettled: 0n,
    tokensSettled: 0n,
    feesPaid: 0n,
    discountSum: 0,
    discountN: 0,
    queue: [],
    nodes: new Set(),
  };
}

function blankNode(addr) {
  return {
    node: addr,
    firstSeen: null,
    lastSeen: null,
    deposits: 0,
    withdrawals: 0,
    /* The running total the vault reported in the last deposit/withdraw event
       this node emitted. It is NOT the current balance: `paid_out` reduces a
       node's deposited capital and carries no new total, so a node that has
       filled anything reads high here. Node A is the live example — 400,000
       last reported, 192,500 paid out, 207,500 actually in the vault. Call
       `vault.get_node` for the number that settles arguments. */
    depositedAtLastReport: '0',
    bidsPlaced: 0,
    bidsOutbid: 0,
    fillsWon: 0,
    usdcPaidOut: 0n,
    appetites: {},
  };
}

function build(events, meta) {
  const assets = new Map();
  const nodes = new Map();
  const exits = new Map();
  let feesTotal = 0n;
  let usdcTotal = 0n;
  let flagged = 0;

  const asset = (id) => {
    if (!id) return null;
    if (!assets.has(id)) assets.set(id, blankAsset(id));
    return assets.get(id);
  };
  const node = (addr, ts) => {
    if (!addr) return null;
    if (!nodes.has(addr)) nodes.set(addr, blankNode(addr));
    const n = nodes.get(addr);
    if (!n.firstSeen) n.firstSeen = ts;
    n.lastSeen = ts;
    return n;
  };
  const exit = (a, id, ts) => {
    const key = `${a}:${id}`;
    if (!exits.has(key)) {
      exits.set(key, {
        key, asset: a, assetName: cfg.assetNames[a] || null, exitId: Number(id),
        state: 'unknown', seller: null, amount: null, referenceUsdc: null,
        minAcceptUsdc: null, closesAt: null, openedAt: ts, endedAt: null,
        bestBid: null, bestNode: null, bestDiscountBps: null, bidCount: 0,
        queuePosition: null, settlement: null,
      });
    }
    return exits.get(key);
  };

  for (const e of events) {
    /* An event that failed to decode, came from the wrong contract, or carries
       a name this build does not know is counted and then skipped. Feeding it
       into the totals would be guessing at what it meant. */
    if (e.decodeError || e.mismatch || e.unknown || !e.name) { flagged++; continue; }

    const d = e.data || {};
    const A = d.asset ? asset(d.asset) : null;
    if (A && d.node) A.nodes.add(d.node);

    switch (e.name) {
      case 'deposit': {
        const n = node(d.node, e.ts);
        n.deposits++; n.depositedAtLastReport = s(big(d.total_deposited));
        break;
      }
      case 'withdraw': {
        const n = node(d.node, e.ts);
        n.withdrawals++; n.depositedAtLastReport = s(big(d.total_deposited));
        break;
      }
      case 'withdraw_requested':
      case 'withdraw_cancelled':
        node(d.node, e.ts);
        break;
      case 'appetite_set': {
        const n = node(d.node, e.ts);
        n.appetites[d.asset] = {
          asset: d.asset,
          name: cfg.assetNames[d.asset] || null,
          maxExposure: s(big(d.max_exposure)),
          minDiscountBps: Number(d.min_discount_bps || 0),
          active: !!d.active,
          setAt: e.ts,
        };
        break;
      }
      case 'committed':
      case 'released':
        node(d.node, e.ts);
        break;
      case 'paid_out': {
        const n = node(d.node, e.ts);
        n.usdcPaidOut += big(d.amount);
        break;
      }
      case 'opened': {
        const x = exit(d.asset, d.exit_id, e.ts);
        x.state = 'open';
        x.seller = d.seller;
        x.amount = s(big(d.amount));
        x.referenceUsdc = s(big(d.reference_usdc));
        x.minAcceptUsdc = s(big(d.min_accept_usdc));
        x.closesAt = Number(d.closes_at || 0);
        x.openedAt = e.ts;
        if (A) A.exitsOpened++;
        break;
      }
      case 'escrowed': {
        const x = exit(d.asset, d.exit_id, e.ts);
        x.escrowed = s(big(d.amount));
        break;
      }
      case 'bid': {
        const x = exit(d.asset, d.exit_id, e.ts);
        x.bidCount++;
        x.bestBid = s(big(d.usdc_amount));
        x.bestNode = d.node;
        x.bestDiscountBps = Number(d.discount_bps || 0);
        const n = node(d.node, e.ts);
        n.bidsPlaced++;
        if (A) { A.bids++; A.discountSum += Number(d.discount_bps || 0); A.discountN++; }
        /* `outbid` names whoever just lost the lead — the only place the log
           records a bid being displaced, since the release event is anonymous
           about why. */
        if (d.outbid) node(d.outbid, e.ts).bidsOutbid++;
        break;
      }
      case 'settled': {
        const x = exit(d.asset, d.exit_id, e.ts);
        x.state = 'settled';
        x.endedAt = e.ts;
        x.settlement = {
          node: d.node,
          seller: d.seller,
          tokenAmount: s(big(d.token_amount)),
          usdcGross: s(big(d.usdc_gross)),
          protocolFee: s(big(d.protocol_fee)),
          usdcNet: s(big(d.usdc_net)),
          remaining: s(big(d.remaining)),
        };
        if (A) {
          A.settled++;
          A.usdcSettled += big(d.usdc_gross);
          A.tokensSettled += big(d.token_amount);
          A.feesPaid += big(d.protocol_fee);
        }
        feesTotal += big(d.protocol_fee);
        usdcTotal += big(d.usdc_gross);
        node(d.node, e.ts).fillsWon++;
        break;
      }
      case 'cancelled': {
        const x = exit(d.asset, d.exit_id, e.ts);
        x.state = 'cancelled'; x.endedAt = e.ts;
        if (A) A.cancelled++;
        break;
      }
      case 'refunded': {
        const x = exit(d.asset, d.exit_id, e.ts);
        if (x.state !== 'settled' && x.state !== 'cancelled') x.state = 'refunded';
        x.refunded = s(big(d.amount));
        if (A) A.refunded++;
        break;
      }
      case 'queued': {
        const x = exit(d.asset, d.exit_id, e.ts);
        x.state = 'queued';
        x.queuePosition = Number(d.position || 0);
        if (A) { A.queued++; if (!A.queue.includes(Number(d.exit_id))) A.queue.push(Number(d.exit_id)); }
        break;
      }
      case 'dequeued': {
        const x = exit(d.asset, d.exit_id, e.ts);
        x.queuePosition = null;
        if (x.state === 'queued') x.state = 'left_queue';
        if (A) A.queue = A.queue.filter((i) => i !== Number(d.exit_id));
        break;
      }
      default:
        flagged++;
    }
  }

  /* A queued exit that later settled or was cancelled leaves the line without
     a `dequeued` in every path, so the line is re-derived from exit state
     rather than trusted as a running list. Order is preserved: the queue is
     FIFO and the log is in ledger order. */
  for (const A of assets.values()) {
    A.queue = A.queue.filter((id) => {
      const x = exits.get(`${A.asset}:${id}`);
      return x && x.state === 'queued';
    });
    A.queue.forEach((id, i) => {
      const x = exits.get(`${A.asset}:${id}`);
      if (x) x.queuePosition = i;
    });
  }

  const assetList = [...assets.values()].map((a) => ({
    asset: a.asset,
    name: a.name,
    exitsOpened: a.exitsOpened,
    settled: a.settled,
    cancelled: a.cancelled,
    refunded: a.refunded,
    queuedEver: a.queued,
    bids: a.bids,
    usdcSettled: s(a.usdcSettled),
    tokensSettled: s(a.tokensSettled),
    feesPaid: s(a.feesPaid),
    avgDiscountBps: a.discountN ? Math.round(a.discountSum / a.discountN) : null,
    queueDepth: a.queue.length,
    queue: a.queue,
    nodesSeen: [...a.nodes],
  })).sort((x, y) => (y.exitsOpened - x.exitsOpened));

  const nodeList = [...nodes.values()].map((n) => ({
    ...n,
    usdcPaidOut: s(n.usdcPaidOut),
    appetites: Object.values(n.appetites),
  })).sort((x, y) => (y.fillsWon - x.fillsWon) || (y.bidsPlaced - x.bidsPlaced));

  const exitList = [...exits.values()].sort(
    (x, y) => (x.asset === y.asset ? x.exitId - y.exitId : (x.asset < y.asset ? -1 : 1)),
  );

  return {
    generatedAt: new Date().toISOString(),
    networkId: cfg.networkId,
    contracts: cfg.contracts,
    coverage: {
      firstLedger: meta.firstLedger,
      lastLedger: meta.lastLedger,
      chainLedger: meta.chainLedger,
      oldestRetained: meta.oldestRetained,
      /* True only if indexing started at or before the oldest ledger the RPC
         still holds. False means there is history the RPC has already dropped
         and this log never saw — the counts below are a floor, not a census.
         Saying so is the whole point of carrying the field. */
      complete: meta.complete,
      events: meta.total,
      flagged,
    },
    protocol: {
      settlements: exitList.filter((x) => x.state === 'settled').length,
      usdcSettled: s(usdcTotal),
      feesToTreasury: s(feesTotal),
      nodesSeen: nodeList.length,
      assetsSeen: assetList.length,
      openExits: exitList.filter((x) => x.state === 'open').length,
      queuedExits: exitList.filter((x) => x.state === 'queued').length,
    },
    assets: assetList,
    nodes: nodeList,
    exits: exitList,
  };
}

module.exports = { build };

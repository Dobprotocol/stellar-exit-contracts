'use strict';

/* One pass over the exit layer's events, then exit.
 *
 * It is a cron job rather than a daemon for the same reason the EVM indexer
 * is: a process that must stay up is a process that must be watched, and a
 * pass that dies takes nothing with it if the cursor is already on disk. PM2
 * restarts it every five minutes; `autorestart` must stay false or the two
 * schedules fight.
 *
 * Stellar has no reorgs — SCP finalises a ledger when it closes — so a cursor
 * only ever moves forward and an event never has to be un-indexed. That is why
 * the log can be strictly append-only.
 */

const fs = require('fs');
const path = require('path');

const cfg = require('./config');
const rpc = require('./rpc');
const { decodeEvent } = require('./decode');
const snapshot = require('./snapshot');

const LOG = path.join(cfg.dataDir, 'events.jsonl');
const STATE = path.join(cfg.dataDir, 'state.json');
const SNAP = path.join(cfg.dataDir, 'snapshot.json');

const log = (...a) => console.log(new Date().toISOString(), ...a);

/* Write to a sibling temp file and rename. rename(2) is atomic within a
   filesystem, so a reader — the API serving the dashboard — never catches a
   half-written snapshot, and a crash mid-write cannot corrupt the old one. */
function writeAtomic(file, text) {
  const tmp = `${file}.tmp`;
  fs.writeFileSync(tmp, text);
  fs.renameSync(tmp, file);
}

function readState() {
  try { return JSON.parse(fs.readFileSync(STATE, 'utf8')); } catch { return null; }
}

function saveState(st) {
  writeAtomic(STATE, JSON.stringify(st, null, 2));
}

function readLog() {
  if (!fs.existsSync(LOG)) return [];
  const out = [];
  for (const line of fs.readFileSync(LOG, 'utf8').split('\n')) {
    if (!line) continue;
    try { out.push(JSON.parse(line)); } catch { /* truncated tail; the pass that wrote it will re-append */ }
  }
  return out;
}

async function pass() {
  fs.mkdirSync(cfg.dataDir, { recursive: true });

  const { latest, oldest } = await rpc.retention();
  let st = readState();

  if (!st) {
    /* Cold start. The RPC only keeps about a week of events, so "from the
       beginning" means "from the oldest ledger it still has" — and the state
       records which, because whether the log is complete is not something a
       later reader can work out on its own. */
    const from = Number(process.env.EXIT_START_LEDGER || oldest);
    st = {
      startedFrom: from,
      oldestRetainedAtStart: oldest,
      complete: from <= oldest,
      cursor: null,
      lastLedger: from - 1,
      lastEventId: null,
      events: 0,
      passes: 0,
    };
    log(`cold start: from ledger ${from} (retention ${oldest}–${latest}, complete=${st.complete})`);
  } else if (st.cursor === null && st.lastLedger < oldest) {
    log(`WARN cursor fell out of retention (${st.lastLedger} < ${oldest}); resuming at ${oldest} with a gap`);
    st.complete = false;
    st.lastLedger = oldest - 1;
  }

  const seen = new Set(readLog().map((e) => e.id));
  let pages = 0;
  let added = 0;
  const fresh = [];

  while (pages < cfg.maxPagesPerPass) {
    let res;
    try {
      res = await rpc.getEvents(
        st.cursor ? { cursor: st.cursor } : { startLedger: Math.max(st.lastLedger + 1, oldest) },
      );
    } catch (err) {
      /* A cursor the RPC no longer accepts — retention moved past it while we
         were away — is recoverable: drop it and re-enter by ledger. Anything
         else is left to fail the pass, because a pass that pretends to have
         succeeded is worse than one that visibly did not. */
      if (/cursor|ledger range/i.test(err.rpcMessage || err.message || '')) {
        log(`WARN ${err.message}; dropping cursor and re-entering by ledger`);
        st.cursor = null;
        /* Only a re-entry that has to skip forward loses history. Re-entering
           at a ledger we had already reached costs nothing, so coverage stays
           whatever it was — flipping the flag on every recoverable hiccup
           would make an honest field meaningless. */
        if (st.lastLedger < oldest - 1) {
          st.complete = false;
          st.lastLedger = oldest - 1;
        }
        saveState(st);
        pages++;
        continue;
      }
      throw err;
    }

    pages++;
    const batch = [];
    for (const raw of res.events || []) {
      if (seen.has(raw.id)) continue;
      seen.add(raw.id);
      const rec = decodeEvent(raw, cfg.contracts);
      batch.push(rec);
      fresh.push(rec);
      if (rec.ledger > st.lastLedger) st.lastLedger = rec.ledger;
      st.lastEventId = rec.id;
    }

    if (batch.length) {
      fs.appendFileSync(LOG, batch.map((r) => JSON.stringify(r)).join('\n') + '\n');
      added += batch.length;
    }

    st.cursor = res.cursor || st.cursor;
    st.events = (st.events || 0) + batch.length;

    /* Saved per page, not per pass. The EVM indexer saved only at the end, so
       every cron tick that killed a long scan threw away everything it had
       just read and the next one started from the same place — the treadmill.
       A page is the smallest unit of progress the RPC gives us; it is the
       right unit to persist. */
    saveState(st);

    const scanned = st.cursor ? Number(BigInt(st.cursor.split('-')[0]) >> 32n) : st.lastLedger;
    if (scanned >= (res.latestLedger || latest) - 1) { st.lastLedger = Math.max(st.lastLedger, scanned); break; }
    if (!res.cursor) break;
  }

  st.passes = (st.passes || 0) + 1;
  st.updatedAt = new Date().toISOString();
  st.chainLedger = latest;
  st.oldestRetained = oldest;
  saveState(st);

  const all = readLog();
  const meta = {
    firstLedger: all.length ? all[0].ledger : null,
    lastLedger: st.lastLedger,
    chainLedger: latest,
    oldestRetained: oldest,
    complete: !!st.complete,
    total: all.length,
  };
  writeAtomic(SNAP, JSON.stringify(snapshot.build(all, meta), null, 2));

  log(`pass done: ${pages} page(s), ${added} new event(s), ${all.length} total, ledger ${st.lastLedger}/${latest}`);
  if (added && fresh.length) {
    const names = {};
    for (const f of fresh) names[f.name || 'undecoded'] = (names[f.name || 'undecoded'] || 0) + 1;
    log('  new:', Object.entries(names).map(([k, v]) => `${k}×${v}`).join(' '));
  }
}

if (process.argv.includes('--reset')) {
  /* Only the derived files. The log is the record; if it has to go, that is a
     decision for a person with `rm`, not a flag. */
  for (const f of [STATE, SNAP]) if (fs.existsSync(f)) fs.unlinkSync(f);
  log('state and snapshot cleared; the log was left alone');
}

pass().catch((err) => {
  console.error(new Date().toISOString(), 'pass failed:', err && err.stack ? err.stack : err);
  process.exit(1);
});

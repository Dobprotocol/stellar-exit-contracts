'use strict';

const { xdr, scValToNative, Address } = require('@stellar/stellar-sdk');
const { TOPIC_FIELDS, EVENT_SOURCE } = require('./events');

/* i128 comes back as a BigInt and JSON.stringify throws on those. Amounts are
   kept as decimal strings rather than Numbers: a 7-decimal USDC balance is
   already 15 digits, and the point of an audit log is that it does not quietly
   round. Consumers that want to divide can; the log will not do it for them. */
function plain(v) {
  if (typeof v === 'bigint') return v.toString();
  if (v instanceof Uint8Array) return Buffer.from(v).toString('hex');
  if (Array.isArray(v)) return v.map(plain);
  if (v && typeof v === 'object') {
    const out = {};
    for (const k of Object.keys(v)) out[k] = plain(v[k]);
    return out;
  }
  return v;
}

/* scValToNative turns an ScAddress into its string form already, but a topic
   that is an address arrives as a bare ScVal and some SDK paths hand back an
   object instead. Normalising here keeps every address in the log comparable
   as a string. */
function native(b64) {
  const v = scValToNative(xdr.ScVal.fromXDR(b64, 'base64'));
  if (v && typeof v === 'object' && !Array.isArray(v) && typeof v.toString === 'function' && v.constructor && v.constructor.name === 'Address') {
    return v.toString();
  }
  return plain(v);
}

/**
 * Turn one RPC event into a flat record.
 *
 * Never throws: an event this build cannot read is still a fact that happened,
 * so it is written with `name: null` and its raw topics kept. Losing it would
 * be worse than not understanding it.
 */
function decodeEvent(raw, contractNames) {
  const rec = {
    id: raw.id,
    ledger: raw.ledger,
    ts: raw.ledgerClosedAt,
    tx: raw.txHash,
    contract: raw.contractId,
    source: contractNames[raw.contractId] || null,
    name: null,
    data: {},
  };

  try {
    const topics = (raw.topic || []).map(native);
    rec.name = typeof topics[0] === 'string' ? topics[0] : null;

    const fields = TOPIC_FIELDS[rec.name] || [];
    fields.forEach((f, i) => { rec.data[f] = topics[i + 1]; });
    if (topics.length - 1 > fields.length) rec.extraTopics = topics.slice(fields.length + 1);

    const body = raw.value ? native(raw.value) : null;
    if (body && typeof body === 'object' && !Array.isArray(body)) Object.assign(rec.data, body);
    else if (body !== null) rec.data.value = body;

    /* An event whose name we know but whose address is not the contract that
       declares it is either a rename we have not transcribed or something
       impersonating the layer. Flagged, kept, and excluded from the snapshot. */
    const expected = EVENT_SOURCE[rec.name];
    if (expected && rec.source && expected !== rec.source) rec.mismatch = expected;
    if (!expected) rec.unknown = true;
  } catch (err) {
    rec.decodeError = String(err && err.message ? err.message : err);
    rec.rawTopics = raw.topic;
    rec.rawValue = raw.value;
  }

  return rec;
}

module.exports = { decodeEvent, plain };

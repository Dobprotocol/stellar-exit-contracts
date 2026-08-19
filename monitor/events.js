'use strict';

/* The event catalogue, transcribed from the `#[contractevent]` structs.
 *
 * soroban-sdk puts the event name in topic[0] as the snake_case of the struct
 * name, then every field marked `#[topic]` in declaration order, and packs the
 * remaining fields into the value as a map keyed by field name. The map is
 * self-describing, so only the topics need a schema here — and they need one
 * because position is all the wire gives us.
 *
 * Keeping this list next to the contracts means a new event that nobody
 * transcribed shows up as `unknown` in the log rather than being silently
 * dropped. The log keeps the raw topics either way.
 */

const TOPIC_FIELDS = {
  // lp_vault — contracts/lp_vault/src/events.rs
  deposit: ['node'],
  withdraw: ['node'],
  withdraw_requested: ['node'],
  withdraw_cancelled: ['node'],
  appetite_set: ['node', 'asset'],
  committed: ['node', 'asset'],
  released: ['node', 'asset'],
  paid_out: ['node', 'asset'],

  // exit_auction — contracts/exit_auction/src/lib.rs
  opened: ['asset'],
  bid: ['asset'],
  cancelled: ['asset'],

  // fifo_queue — contracts/fifo_queue/src/lib.rs
  queued: ['asset'],
  dequeued: ['asset'],

  // settlement_router — contracts/settlement_router/src/lib.rs
  escrowed: ['asset'],
  settled: ['asset'],
  refunded: ['asset'],
};

/* Which contract each event may legitimately come from. The layer's whole
   error-code design is "a refusal says which contract refused"; the same
   should hold for what it emits, so an event arriving from the wrong address
   is recorded and flagged rather than folded into the totals. */
const EVENT_SOURCE = {
  deposit: 'lp_vault',
  withdraw: 'lp_vault',
  withdraw_requested: 'lp_vault',
  withdraw_cancelled: 'lp_vault',
  appetite_set: 'lp_vault',
  committed: 'lp_vault',
  released: 'lp_vault',
  paid_out: 'lp_vault',
  opened: 'exit_auction',
  bid: 'exit_auction',
  cancelled: 'exit_auction',
  queued: 'fifo_queue',
  dequeued: 'fifo_queue',
  escrowed: 'settlement_router',
  settled: 'settlement_router',
  refunded: 'settlement_router',
};

module.exports = { TOPIC_FIELDS, EVENT_SOURCE };

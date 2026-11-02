// Provider callbacks retain ownership of questions; answering and cancellation settle them once.
import { randomUUID, createHash } from 'node:crypto';

const COMPLETED_LIMIT = 128;

/** Correlate live callbacks and bounded idempotent delivery receipts without a UI expiry timer. */
export class Requests {
  constructor(emit, lifecycle = () => {}) { this.emit = emit; this.lifecycle = lifecycle; this.pending = new Map(); this.completed = new Map(); }
  ask(fields, signal) {
    if (signal?.aborted) return Promise.reject(new Error('The request was cancelled.'));
    const id = randomUUID();
    return new Promise((resolve, reject) => {
      const cancel = () => this.cancel(id, 'The request was cancelled.');
      this.pending.set(id, { fields, resolve, reject, cleanup: () => signal?.removeEventListener('abort', cancel) });
      this.lifecycle('start', id, fields);
      signal?.addEventListener('abort', cancel, { once: true });
      this.emit('request', { id, ...fields });
    });
  }
  answer(id, answer) {
    const fingerprint = createHash('sha256').update(JSON.stringify(answer)).digest('hex');
    const old = this.completed.get(id);
    if (old) return old === fingerprint ? { ok: true } : { ok: false, closed: true, reason: 'This request already received a different answer.' };
    const request = this.pending.get(id);
    if (!request) return { ok: false, closed: true, reason: 'That request is no longer pending.' };
    this.pending.delete(id); request.cleanup();
    this.lifecycle('end', id, request.fields);
    this.completed.set(id, fingerprint);
    while (this.completed.size > COMPLETED_LIMIT) this.completed.delete(this.completed.keys().next().value);
    request.resolve(answer);
    this.emit('resolved', { id, answered: true, answer });
    return { ok: true };
  }
  cancel(id, reason) {
    const request = this.pending.get(id);
    if (!request) return;
    this.pending.delete(id); request.cleanup();
    this.lifecycle('end', id, request.fields);
    request.reject(new Error(reason));
    this.emit('resolved', { id, answered: false, reason });
  }
  cancelAll(reason, thread) {
    for (const [id, request] of this.pending) {
      if (!thread || request.fields.input?.threadId === thread) this.cancel(id, reason);
    }
  }
}

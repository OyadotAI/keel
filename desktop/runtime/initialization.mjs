// Request launch-only context from the daemon only when a provider session must be created.
import { randomUUID } from 'node:crypto';

const INITIALIZATION_WAIT_MS = 60_000;

/** Own one bounded launch request so warm turns avoid rebuilding discarded provider context. */
export class Initialization {
  constructor(emit) { this.emit = emit; this.epoch = 0; }

  /** Refresh launch restrictions for each new provider process, including replacements. */
  async launch(query) {
    if (this.pending) throw new Error('Provider initialization is already pending.');
    const epoch = this.epoch;
    const assertLaunch = () => { if (epoch !== this.epoch) throw this.cancellation; };
    const id = randomUUID();
    const launch = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.cancel('Provider initialization timed out.'), INITIALIZATION_WAIT_MS);
      this.pending = { id, resolve, reject, timer };
      this.emit('initialize', { id });
    });
    assertLaunch();
    if (typeof launch?.system !== 'string' || (query.provider === 'claude' && typeof launch.mcp?.mcpServers?.keel?.url !== 'string') || (query.provider === 'codex' &&
      (!Array.isArray(launch.codexArgs) || !launch.codexArgs.length || launch.codexArgs.some((arg) => typeof arg !== 'string')))) {
      throw new Error('Provider initialization did not include launch restrictions.');
    }
    return { ...query, system: launch.system, codexArgs: launch.codexArgs, mcp: launch.mcp, assertLaunch };
  }

  /** Correlate private daemon replies; a stale reply cannot initialize another provider. */
  complete(message) {
    if (!this.pending || message.id !== this.pending.id) return;
    const pending = this.pending; this.pending = undefined; clearTimeout(pending.timer);
    if (message.error) pending.reject(new Error(message.error));
    else pending.resolve(message.launch);
  }

  /** Stop waiting before any provider can launch after interruption or helper shutdown. */
  cancel(reason, interrupted = false) {
    this.epoch++;
    this.cancellation = new Error(reason);
    if (interrupted) this.cancellation.name = 'AbortError';
    if (!this.pending) return;
    const pending = this.pending; this.pending = undefined; clearTimeout(pending.timer);
    pending.reject(this.cancellation);
  }
}

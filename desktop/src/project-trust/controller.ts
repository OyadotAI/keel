// One explicit, project-bound trust decision owns connection, submission and recovery.
import type { Endpoint } from "../api";

/** Transient consent stays tied to its original project even when the active lane changes. */
export interface TrustRequest {
  project: string;
  phase: "connecting" | "ready" | "saving" | "failed";
  endpoint?: Endpoint;
  error?: string;
}

/** The owner supplies project lifetime and I/O so consent races can be checked without a browser. */
export interface TrustDependencies {
  read(): TrustRequest | null;
  put(request: TrustRequest | null): void;
  exists(project: string): boolean;
  ensure(project: string): Promise<Endpoint>;
  grant(endpoint: Endpoint): Promise<unknown>;
  confirmed(project: string): void;
}

const message = (error: unknown) => error instanceof Error ? error.message : String(error);
const CLOSED_PROJECT = "This project is no longer open. Reopen it to change trust.";

/** Share explicit consent, duplicate prevention and stale-result handling across all entry points. */
export function createTrustController(deps: TrustDependencies) {
  async function prepare(request: TrustRequest) {
    try {
      if (!deps.exists(request.project)) throw new Error(CLOSED_PROJECT);
      const endpoint = await deps.ensure(request.project);
      if (deps.read() !== request) return;
      if (!deps.exists(request.project)) throw new Error(CLOSED_PROJECT);
      deps.put({ ...request, phase: "ready", endpoint });
    } catch (error) {
      if (deps.read() === request) deps.put({ ...request, phase: "failed", error: message(error) });
    }
  }

  function open(project: string) {
    if (deps.read()?.phase === "saving") return;
    const request: TrustRequest = { project, phase: "connecting" };
    deps.put(request);
    void prepare(request);
  }

  async function confirm() {
    const request = deps.read();
    if (request?.phase !== "ready" || !request.endpoint) return;
    if (!deps.exists(request.project)) {
      deps.put({ ...request, phase: "failed", endpoint: undefined, error: CLOSED_PROJECT });
      return;
    }
    const saving: TrustRequest = { ...request, phase: "saving", error: undefined };
    deps.put(saving);
    try {
      const endpoint = await deps.ensure(request.project);
      if (!deps.exists(request.project)) throw new Error(CLOSED_PROJECT);
      if (await deps.grant(endpoint) !== true) throw new Error("Keel did not confirm project trust. Try again.");
      if (deps.read() !== saving) return;
      deps.confirmed(request.project);
      deps.put(null);
    } catch (error) {
      if (deps.read() === saving) deps.put({ ...request, error: message(error) });
    }
  }

  return {
    open,
    confirm,
    retry() { const request = deps.read(); if (request?.phase === "failed") open(request.project); },
    dismiss() { if (deps.read()?.phase !== "saving") deps.put(null); },
  };
}

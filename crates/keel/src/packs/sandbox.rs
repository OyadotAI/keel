//! sandbox: a code-runner service, like E2B / Piston ═══════════════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", SANDBOX_APP.into()),
        ("backend/src/store.ts", SANDBOX_STORE.into()),
        ("backend/src/executor.ts", SANDBOX_EXECUTOR.into()),
        ("backend/src/worker.ts", SANDBOX_WORKER.into()),
        ("backend/src/app.test.ts", SANDBOX_TEST.into()),
        ("backend/migrations/0002_sandbox.sql", SANDBOX_SQL.into()),
        ("frontend/app/page.tsx", f(SANDBOX_PAGE)),
        ("CLAUDE.md", f(SANDBOX_CLAUDE_MD)),
        ("AGENTS.md", f(SANDBOX_AGENTS_MD)),
        ("README.md", f(SANDBOX_README_MD)),
        (
            ".claude/agents/isolation.md",
            SANDBOX_AGENT_ISOLATION.into(),
        ),
    ]
}

const SANDBOX_SQL: &str = r##"create table if not exists runs (
  id uuid primary key,
  language text not null,
  version text not null,
  files jsonb not null,               -- [{name, content}]
  stdin text not null default '',
  args jsonb not null default '[]',
  run_timeout int not null,           -- ms
  run_memory_limit bigint not null,   -- bytes
  state text not null default 'queued',   -- queued | running | done | failed
  run jsonb,                          -- {stdout, stderr, code, signal, output}
  ms int,
  error text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists runs_ready on runs (state, created_at);
create table if not exists artifacts (
  id uuid primary key,
  run_id uuid not null references runs(id),
  name text not null,
  bytes bytea not null,
  expires_at timestamptz not null
);
create index if not exists artifacts_expiry on artifacts (expires_at);
"##;

const SANDBOX_STORE: &str = r##"import { db } from "./db";

export type RunFile = { name: string; content: string };
export type RunResult = { stdout: string; stderr: string; code: number | null; signal: string | null; output: string };
export type Job = { id: string; language: string; version: string; files: RunFile[]; stdin: string; args: string[]; run_timeout: number; run_memory_limit: number };
export type Run = Job & { state: string; run: RunResult | null; ms: number | null; error: string | null; created_at: string };
export type Artifact = { id: string; run_id: string; name: string; bytes: Uint8Array; expires_at: string };

export interface Store {
  enqueue(job: Job): Promise<void>;
  claim(): Promise<Job | null>;
  finish(id: string, r: { run: RunResult; ms: number } | { error: string }, artifacts: { name: string; bytes: Uint8Array; expires_at: Date }[]): Promise<void>;
  get(id: string): Promise<Run | null>;
  list(limit: number): Promise<Run[]>;
  artifacts(runId: string): Promise<Omit<Artifact, "bytes">[]>;
  artifact(id: string, now: Date): Promise<Artifact | null>;
  expire(now: Date): Promise<number>;
}

export function pgStore(): Store {
  return {
    enqueue: async (j) => { await db`insert into runs (id, language, version, files, stdin, args, run_timeout, run_memory_limit) values (${j.id}, ${j.language}, ${j.version}, ${db.json(j.files as never)}, ${j.stdin}, ${db.json(j.args as never)}, ${j.run_timeout}, ${j.run_memory_limit})`; },
    claim: async () => (await db<Job[]>`update runs set state = 'running', updated_at = now()
      where id = (select id from runs where state = 'queued' order by created_at limit 1 for update skip locked) returning *`)[0] ?? null,
    finish: async (id, r, artifacts) => db.begin(async (tx) => {
      if ("error" in r) await tx`update runs set state = 'failed', error = ${r.error}, updated_at = now() where id = ${id}`;
      else await tx`update runs set state = 'done', run = ${tx.json(r.run as never)}, ms = ${r.ms}, updated_at = now() where id = ${id}`;
      for (const a of artifacts) await tx`insert into artifacts (id, run_id, name, bytes, expires_at) values (${crypto.randomUUID()}, ${id}, ${a.name}, ${a.bytes as never}, ${a.expires_at})`;
    }),
    get: async (id) => (await db<Run[]>`select * from runs where id = ${id}`)[0] ?? null,
    list: async (limit) => db<Run[]>`select * from runs order by created_at desc limit ${limit}`,
    artifacts: async (runId) => db`select id, run_id, name, expires_at from artifacts where run_id = ${runId} and expires_at > now()`,
    artifact: async (id, now) => (await db<Artifact[]>`select * from artifacts where id = ${id} and expires_at > ${now}`)[0] ?? null,
    expire: async (now) => (await db`delete from artifacts where expires_at <= ${now}`).count,
  };
}

export function memoryStore(): Store {
  const runs = new Map<string, Run>(); const arts = new Map<string, Artifact>();
  return {
    enqueue: async (j) => { runs.set(j.id, { ...j, state: "queued", run: null, ms: null, error: null, created_at: new Date().toISOString() }); },
    claim: async () => { const r = [...runs.values()].find((r) => r.state === "queued"); if (!r) return null; r.state = "running"; return r; },
    finish: async (id, r, artifacts) => {
      const run = runs.get(id)!;
      if ("error" in r) { run.state = "failed"; run.error = r.error; } else { run.state = "done"; run.run = r.run; run.ms = r.ms; }
      for (const a of artifacts) { const aid = crypto.randomUUID(); arts.set(aid, { id: aid, run_id: id, name: a.name, bytes: a.bytes, expires_at: a.expires_at.toISOString() }); }
    },
    get: async (id) => runs.get(id) ?? null,
    list: async (limit) => [...runs.values()].reverse().slice(0, limit),
    artifacts: async (runId) => [...arts.values()].filter((a) => a.run_id === runId && new Date(a.expires_at) > new Date()).map(({ bytes: _b, ...a }) => a),
    artifact: async (id, now) => { const a = arts.get(id); return a && new Date(a.expires_at) > now ? a : null; },
    expire: async (now) => { let n = 0; for (const [k, a] of arts) if (new Date(a.expires_at) <= now) { arts.delete(k); n++; } return n; },
  };
}
"##;

const SANDBOX_EXECUTOR: &str = r##"import { spawn } from "node:child_process";
import { mkdtemp, writeFile, readdir, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Job, RunResult, RunFile } from "./store";

// Each job runs in a throwaway container: no network, a memory cap, a CPU share, a pid cap, a
// read-only root and a bind-mounted work dir that is the only writable place. Whatever the code
// leaves in /work/out comes back as artifacts. The executor is a function so the worker and the
// tests take a fake one; `dockerArgv` is pure so the isolation flags can be asserted without Docker.

export type Executed = { run: RunResult; ms: number; outputs: { name: string; bytes: Uint8Array }[] };
export type Executor = (job: Job) => Promise<Executed>;

/// language → image and how to run the first file. Add a language here.
export const LANGUAGES: Record<string, { image: string; version: string; cmd: (file: string) => string[] }> = {
  python: { image: "python:3.12-alpine", version: "3.12", cmd: (f) => ["python3", f] },
  javascript: { image: "node:22-alpine", version: "22", cmd: (f) => ["node", f] },
};

export const LIMITS = { timeout: { def: 3000, max: 30_000 }, memory: { def: 128 * 1024 * 1024, max: 512 * 1024 * 1024 }, cpus: process.env.RUN_CPUS ?? "0.5", pids: 64, artifactTtlMs: 60 * 60_000 };

export function dockerArgv(job: Job, workDir: string): string[] {
  const lang = LANGUAGES[job.language];
  if (!lang) throw new Error(`unsupported language ${job.language}`);
  return [
    "docker", "run", "--rm", "-i",
    "--network", "none",
    "--memory", String(job.run_memory_limit), "--memory-swap", String(job.run_memory_limit),
    "--cpus", LIMITS.cpus, "--pids-limit", String(LIMITS.pids),
    "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
    "--user", "65534:65534", "-v", `${workDir}:/work`, "-w", "/work",
    lang.image, ...lang.cmd(job.files[0].name), ...job.args,
  ];
}

export function dockerExecutor(): Executor {
  return async (job) => {
    const dir = await mkdtemp(join(tmpdir(), "run-"));
    try {
      for (const f of job.files) await writeFile(join(dir, safeName(f)), f.content);
      const t0 = Date.now();
      const run = await spawnCaptured(dockerArgv(job, dir), job.stdin, job.run_timeout);
      const ms = Date.now() - t0;
      const outputs: Executed["outputs"] = [];
      const outDir = join(dir, "out");
      if (await stat(outDir).catch(() => null)) for (const name of await readdir(outDir)) outputs.push({ name, bytes: new Uint8Array(await readFile(join(outDir, name))) });
      return { run, ms, outputs };
    } finally { await rm(dir, { recursive: true, force: true }); }
  };
}

function safeName(f: RunFile): string {
  if (!/^[\w.-]+$/.test(f.name)) throw new Error(`bad file name ${f.name}`);
  return f.name;
}

const CAP = 64 * 1024; // per stream; Piston caps output the same way

function spawnCaptured(argv: string[], stdin: string, timeoutMs: number): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    const p = spawn(argv[0], argv.slice(1), { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    // The timeout is the docker client's, not the container's; --rm plus SIGKILL takes both down.
    const timer = setTimeout(() => p.kill("SIGKILL"), timeoutMs);
    p.stdout.on("data", (d) => { if (stdout.length < CAP) stdout += d; });
    p.stderr.on("data", (d) => { if (stderr.length < CAP) stderr += d; });
    p.on("error", reject);
    p.on("close", (code, signal) => { clearTimeout(timer); resolve({ stdout, stderr, code, signal, output: stdout + stderr }); });
    p.stdin.end(stdin);
  });
}

/// Tests and `make check`: pretends every program prints its first file.
export function fakeExecutor(outputs: { name: string; bytes: Uint8Array }[] = []): Executor {
  return async (job) => ({ run: { stdout: job.files[0].content, stderr: "", code: 0, signal: null, output: job.files[0].content }, ms: 1, outputs });
}
"##;

const SANDBOX_WORKER: &str = r##"import { pgStore, type Store } from "./store";
import { dockerExecutor, LIMITS, type Executor } from "./executor";

// The worker: claim one queued run, execute it, store the result and its artifacts. Its own
// process (`bun run worker`) so it scales on queue depth; a run that throws is marked failed,
// never retried — the code is the user's, and running it twice is not safer.

export async function tick(store: Store, exec: Executor, now = new Date()): Promise<boolean> {
  const job = await store.claim();
  if (!job) return false;
  try {
    const r = await exec(job);
    const expires_at = new Date(now.getTime() + LIMITS.artifactTtlMs);
    await store.finish(job.id, { run: r.run, ms: r.ms }, r.outputs.map((o) => ({ ...o, expires_at })));
  } catch (e) {
    await store.finish(job.id, { error: e instanceof Error ? e.message : String(e) }, []);
  }
  return true;
}

if (import.meta.main) {
  const store = pgStore(); const exec = dockerExecutor();
  console.log(JSON.stringify({ level: "info", msg: "worker started" }));
  let stopping = false;
  process.on("SIGTERM", () => { stopping = true; });
  let lastSweep = 0;
  while (!stopping) {
    if (Date.now() - lastSweep > 60_000) { await store.expire(new Date()); lastSweep = Date.now(); }
    if (!(await tick(store, exec))) await new Promise((r) => setTimeout(r, 500));
  }
}
"##;

const SANDBOX_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { LANGUAGES, LIMITS } from "./executor";

// Piston's surface: POST /api/v2/execute {language, version, files, stdin, args, run_timeout,
// run_memory_limit} → {language, version, run:{stdout, stderr, code, signal, output}}. Here the
// run is queued (POST /api/runs → 202) and polled (GET /api/runs/:id), because a container
// start is not a request-path thing; /api/v2/execute stays as a blocking alias for Piston clients.
// Limits are clamped at the boundary: the executor never sees a value above LIMITS.

const Body = z.object({
  language: z.string().refine((l) => l in LANGUAGES, "unsupported language"),
  version: z.string().default("*"),
  files: z.array(z.object({ name: z.string().regex(/^[\w.-]+$/), content: z.string().max(200_000) })).min(1).max(20),
  stdin: z.string().max(200_000).default(""),
  args: z.array(z.string().max(1000)).max(50).default([]),
  run_timeout: z.number().int().min(100).max(LIMITS.timeout.max).default(LIMITS.timeout.def),
  run_memory_limit: z.number().int().min(16 * 1024 * 1024).max(LIMITS.memory.max).default(LIMITS.memory.def),
});

export function createApp(store: Store) {
  const enqueue = async (body: unknown) => {
    const p = Body.safeParse(body);
    if (!p.success) return { error: p.error.flatten() };
    const id = crypto.randomUUID();
    await store.enqueue({ id, ...p.data, version: LANGUAGES[p.data.language].version });
    return { id };
  };

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/runtimes", (c) => c.json(Object.entries(LANGUAGES).map(([language, l]) => ({ language, version: l.version, image: l.image }))))

    .post("/api/runs", async (c) => {
      const r = await enqueue(await c.req.json().catch(() => ({})));
      return "error" in r ? c.json({ error: r.error }, 400) : c.json({ id: r.id, state: "queued" }, 202);
    })
    .get("/api/runs", async (c) => c.json({ runs: (await store.list(50)).map(({ files: _f, stdin: _s, ...r }) => r) }))
    .get("/api/runs/:id", async (c) => {
      const r = await store.get(c.req.param("id"));
      if (!r) return c.json({ error: { message: "no such run", code: "not_found" } }, 404);
      return c.json({ ...r, artifacts: await store.artifacts(r.id) });
    })
    .get("/api/artifacts/:id", async (c) => {
      const a = await store.artifact(c.req.param("id"), new Date());
      if (!a) return c.json({ error: { message: "no such artifact or expired", code: "not_found" } }, 404);
      return c.body(new Uint8Array(a.bytes).slice().buffer as ArrayBuffer, 200, { "content-type": "application/octet-stream", "content-disposition": `attachment; filename="${a.name}"` });
    })

    // Piston-compatible: block until the worker has run it, up to the run's own timeout plus slack.
    .post("/api/v2/execute", async (c) => {
      const r = await enqueue(await c.req.json().catch(() => ({})));
      if ("error" in r) return c.json({ message: "invalid request", error: r.error }, 400);
      const deadline = Date.now() + LIMITS.timeout.max + 10_000;
      while (Date.now() < deadline) {
        const run = await store.get(r.id);
        if (run?.state === "done") return c.json({ language: run.language, version: run.version, run: run.run });
        if (run?.state === "failed") return c.json({ message: run.error ?? "failed" }, 500);
        await new Promise((res) => setTimeout(res, 200));
      }
      return c.json({ message: "no worker picked up the run", code: "timeout" }, 504);
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const SANDBOX_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { dockerArgv, fakeExecutor, LIMITS } from "./executor";
import { tick } from "./worker";

const post = (app: ReturnType<typeof createApp>, body: unknown) => app.request("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const py = { language: "python", files: [{ name: "main.py", content: "print('hi')" }] };

describe("sandbox", () => {
  test("the container gets no network and every limit", () => {
    const argv = dockerArgv({ id: "r", language: "python", version: "3.12", files: [{ name: "main.py", content: "" }], stdin: "", args: ["--x"], run_timeout: 1000, run_memory_limit: 64 << 20 }, "/tmp/w");
    expect(argv.slice(0, 3)).toEqual(["docker", "run", "--rm"]);
    expect(argv).toContain("--network"); expect(argv[argv.indexOf("--network") + 1]).toBe("none");
    expect(argv[argv.indexOf("--memory") + 1]).toBe(String(64 << 20));
    expect(argv[argv.indexOf("--cpus") + 1]).toBe(LIMITS.cpus);
    expect(argv[argv.indexOf("--pids-limit") + 1]).toBe(String(LIMITS.pids));
    expect(argv).toContain("--read-only"); expect(argv).toContain("no-new-privileges");
    expect(argv.slice(-4)).toEqual(["python:3.12-alpine", "python3", "main.py", "--x"]);
  });

  test("queue → worker → result, in Piston's shape", async () => {
    const store = memoryStore(); const app = createApp(store);
    const res = await post(app, py);
    expect(res.status).toBe(202);
    const { id } = await res.json();
    expect((await (await app.request(`/api/runs/${id}`)).json()).state).toBe("queued");
    expect(await tick(store, fakeExecutor())).toBe(true);
    expect(await tick(store, fakeExecutor())).toBe(false);
    const run = await (await app.request(`/api/runs/${id}`)).json();
    expect(run.state).toBe("done"); expect(run.version).toBe("3.12");
    expect(run.run).toEqual({ stdout: "print('hi')", stderr: "", code: 0, signal: null, output: "print('hi')" });
  });

  test("limits above the ceiling and unknown languages are refused at the boundary", async () => {
    const app = createApp(memoryStore());
    expect((await post(app, { ...py, run_timeout: LIMITS.timeout.max + 1 })).status).toBe(400);
    expect((await post(app, { ...py, run_memory_limit: LIMITS.memory.max * 2 })).status).toBe(400);
    expect((await post(app, { ...py, language: "cobol" })).status).toBe(400);
    expect((await post(app, { ...py, files: [{ name: "../etc/passwd", content: "" }] })).status).toBe(400);
  });

  test("a crashing executor marks the run failed, never retried", async () => {
    const store = memoryStore(); const app = createApp(store);
    const { id } = await (await post(app, py)).json();
    await tick(store, async () => { throw new Error("docker: not found"); });
    const run = await (await app.request(`/api/runs/${id}`)).json();
    expect(run.state).toBe("failed"); expect(run.error).toContain("docker");
    expect(await tick(store, fakeExecutor())).toBe(false);
  });

  test("output files are artifacts that expire", async () => {
    const store = memoryStore(); const app = createApp(store);
    const { id } = await (await post(app, py)).json();
    await tick(store, fakeExecutor([{ name: "plot.png", bytes: new Uint8Array([1, 2, 3]) }]), new Date(Date.now() - LIMITS.artifactTtlMs - 1));
    const stale = await (await app.request(`/api/runs/${id}`)).json();
    expect(stale.artifacts).toEqual([]);
    const { id: id2 } = await (await post(app, py)).json();
    await tick(store, fakeExecutor([{ name: "plot.png", bytes: new Uint8Array([1, 2, 3]) }]));
    const fresh = await (await app.request(`/api/runs/${id2}`)).json();
    expect(fresh.artifacts.length).toBe(1);
    const dl = await app.request(`/api/artifacts/${fresh.artifacts[0].id}`);
    expect(dl.status).toBe(200); expect(new Uint8Array(await dl.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]));
    expect(await store.expire(new Date())).toBe(1);
    expect((await app.request("/api/runs/nope")).status).toBe(404);
  });
});
"##;

const SANDBOX_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Run = { state: string; ms: number | null; error: string | null; run: { stdout: string; stderr: string; code: number | null; signal: string | null } | null; artifacts: { id: string; name: string }[] };

// A playground: pick a language, paste code, run, poll. The code runs in a container with no
// network — start the worker (`bun run worker` in backend/) with Docker available.
export default function Home() {
  const [language, setLanguage] = useState("python");
  const [code, setCode] = useState("import sys\nprint('hello from', sys.version.split()[0])\n");
  const [id, setId] = useState<string | null>(null);
  const [run, setRun] = useState<Run | null>(null);

  async function submit() {
    setRun(null);
    const res = await fetch("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ language, files: [{ name: language === "python" ? "main.py" : "main.js", content: code }] }) });
    setId(res.ok ? (await res.json()).id : null);
  }

  useEffect(() => {
    if (!id) return;
    const t = setInterval(async () => {
      const r: Run = await (await fetch(`/api/runs/${id}`)).json();
      setRun(r);
      if (r.state === "done" || r.state === "failed") clearInterval(t);
    }, 500);
    return () => clearInterval(t);
  }, [id]);

  return (
    <main>
      <h1>{{NAME}} — sandbox</h1>
      <p>Piston-compatible: <code>{`curl -X POST http://localhost:8000/api/v2/execute -H 'content-type: application/json' -d '{"language":"python","files":[{"name":"main.py","content":"print(1+1)"}]}'`}</code></p>
      <p>
        <select value={language} onChange={(e) => setLanguage(e.target.value)}><option value="python">python 3.12</option><option value="javascript">node 22</option></select>
        {" "}<button onClick={submit} disabled={!!run && run.state !== "done" && run.state !== "failed"}>Run</button>
      </p>
      <textarea value={code} onChange={(e) => setCode(e.target.value)} rows={10} style={{ width: "100%", fontFamily: "monospace" }} />
      {run && (
        <section>
          <p>state <code>{run.state}</code>{run.ms != null && <> · {run.ms} ms</>}{run.run && <> · exit <code>{run.run.code ?? run.run.signal}</code></>}</p>
          {run.error && <pre style={{ color: "crimson" }}>{run.error}</pre>}
          {run.run?.stdout && <pre>{run.run.stdout}</pre>}
          {run.run?.stderr && <pre style={{ color: "crimson" }}>{run.run.stderr}</pre>}
          {run.artifacts?.length > 0 && <ul>{run.artifacts.map((a) => <li key={a.id}><a href={`/api/artifacts/${a.id}`}>{a.name}</a></li>)}</ul>}
        </section>
      )}
    </main>
  );
}
"##;

const SANDBOX_CLAUDE_MD: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a code-runner service in the shape of Piston and E2B: POST source files, a worker
runs them in a throwaway Docker container with no network and hard limits, and the result — stdout,
stderr, exit code, whatever the program wrote to `/work/out` — is readable by id. "Done" means a
`runs` row in `done` or `failed`, its `run` JSON in Piston's shape, and its artifacts stored with an
expiry. The request path never starts a container; that is what the queue is for.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes: `Body` schema with the clamps, queue/poll, artifact download, the Piston-compatible blocking alias. Exports `AppType`. |
| `backend/src/store.ts` | `Store` interface, `pgStore` (`SKIP LOCKED` claim, `finish` in one transaction) and `memoryStore`. |
| `backend/src/executor.ts` | `LANGUAGES`, `LIMITS`, `dockerArgv` (pure), `dockerExecutor` (spawn, capture, collect `/work/out`), `fakeExecutor`. |
| `backend/src/worker.ts` | `tick`: claim one run, execute, store; the artifact expiry sweep. The `bun run worker` process. |
| `backend/src/app.test.ts` | Five tests over `memoryStore` and `fakeExecutor`; no Docker. |
| `backend/migrations/0002_sandbox.sql` | `runs`, `artifacts`. |
| `frontend/app/page.tsx` | A playground: language, code, run, poll. |

### Request path

1. `POST /api/runs` — body parsed by `Body`: language in `LANGUAGES`, 1–20 files with safe names, `run_timeout ≤ 30 000 ms`, `run_memory_limit ≤ 512 MiB`. Anything above is `400`, not clamped down silently.
2. Insert as `queued` with `version` taken from `LANGUAGES`, `202 {id, state}`.
3. `worker.ts tick()` claims one queued run (`for update skip locked`).
4. `dockerExecutor` writes the files into a `mkdtemp` directory and builds `dockerArgv`: `--network none`, `--memory` and `--memory-swap` equal (no swap), `--cpus`, `--pids-limit 64`, `--read-only`, `--cap-drop ALL`, `no-new-privileges`, user `65534`, the work dir bind-mounted at `/work`.
5. `spawnCaptured` feeds `stdin`, caps each stream at 64 KiB, and `SIGKILL`s the docker client at `run_timeout`.
6. Files under `/work/out` are read back as artifacts; the temp directory is removed in `finally`.
7. `store.finish` writes the result and the artifacts (with `expires_at = now + 1h`) in one transaction. An executor exception writes `failed` with the message; never retried.
8. `GET /api/runs/:id` returns the row plus unexpired artifacts; `GET /api/artifacts/:id` streams one as an attachment.

`POST /api/v2/execute` does 1–2, then polls the store every 200 ms until the worker has finished, up to `LIMITS.timeout.max + 10 s`, and answers in Piston's `{language, version, run}` shape.

### Data model

| Table | Column | Why |
|---|---|---|
| `runs` | `files jsonb`, `stdin`, `args` | The whole job, so a worker needs nothing but the row. Stripped from the list route. |
| | `run_timeout`, `run_memory_limit` | Stored as validated, so the executor trusts them. |
| | `run jsonb` | `{stdout, stderr, code, signal, output}` — Piston's shape, verbatim. |
| | `error` | Executor failure (docker missing, bad image), distinct from a program that exited non-zero. |
| `artifacts` | `bytes bytea`, `expires_at` | Output files, in Postgres for now; the sweep deletes by `expires_at`. |
| `runs_ready`, `artifacts_expiry` | indexes | The claim and the sweep. |

## Invariants

1. **The container has no network.** `--network none` is in `dockerArgv` unconditionally. Guarded by `the container gets no network and every limit`.
2. **Every limit is on the command line**: memory (and swap equal to it), cpus, pids, read-only root, all capabilities dropped, no new privileges, unprivileged user. Same test asserts each flag and value.
3. **Limits are clamped at the boundary, not in the executor.** `Body` refuses values above `LIMITS`; `dockerArgv` never sees one. Guarded by `limits above the ceiling and unknown languages are refused at the boundary`.
4. **File names cannot traverse.** `^[\w.-]+$` in `Body` and again in `safeName`; `../etc/passwd` is `400`. Same test.
5. **Only known languages run.** `language` must be a key of `LANGUAGES`; `cobol` is `400`. Same test.
6. **A run is executed at most once.** The claim is `SKIP LOCKED`, and a throwing executor marks `failed` — never back to `queued`. Guarded by `a crashing executor marks the run failed, never retried` (a second `tick` finds nothing).
7. **Results are in Piston's shape.** `run.run` is `{stdout, stderr, code, signal, output}` and `version` is the language's. Guarded by `queue → worker → result, in Piston's shape`.
8. **Artifacts expire.** Listed and downloadable only while `expires_at > now`; `expire()` deletes them. Guarded by `output files are artifacts that expire`.
9. **Output is bounded.** 64 KiB per stream in `spawnCaptured`; a program printing forever cannot fill the worker's memory. Not unit-tested (needs a process); keep `CAP` when touching it.
10. **The work directory is always removed**, in `finally`, whether the run succeeded, timed out or threw.
11. **The request path never spawns.** Only `worker.ts` calls the executor; `/api/v2/execute` waits on the store.

## Extending it

**Add a language.** One entry in `LANGUAGES`: `image`, `version`, `cmd(firstFile)`. Pull the image on the worker host. Add it to the `argv.slice(-4)` style assertion in the first test. The page's `<select>` is hard-coded — add the option. No migration.

**Add a compile step** (C, Rust, Go). Extend the entry with `compile?: (file) => string[]`, run it first in `dockerExecutor` in the same work dir (it is the only writable path), and record a `compile` result beside `run`, as Piston does. Migration: `alter table runs add column compile jsonb`. Test: `dockerArgv` for the compile argv, and a `fakeExecutor` that returns both.

**Raise a limit.** `LIMITS` is the single source; the schema reads `LIMITS.*.max`. Change the number, and the boundary test's `max + 1` moves with it. `RUN_CPUS` is already an env var.

**Move artifacts to object storage.** Keep `Store.finish`'s signature; have `pgStore` upload `bytes` and store a key. `GET /api/artifacts/:id` becomes a signed redirect. `expire()` deletes both. The `memoryStore` path is unchanged, so the tests still run without S3.

**Per-caller quotas.** Add a principal (API key) to `Body`'s context, a `runs.owner` column, and a count-per-window check before `enqueue`. Rate-limit here, at the boundary; the worker should never decide.

**Packages / dependencies.** Not supported: the container has no network, so `pip install` cannot run inside it. Build a language image that already contains the packages, and register it as another `LANGUAGES` entry (`python-data`, say).

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres; API and worker. |
| `RUN_CPUS` | no | `--cpus` for every container; default `0.5`. |
| `PORT` | no | API port, default 8000. |

The worker needs a Docker daemon (`docker` on `PATH` and access to the socket) and the images pulled: `python:3.12-alpine`, `node:22-alpine`.

**Processes.** `backend` is stateless. `worker` (`bun run worker`) runs one job at a time and sweeps expired artifacts once a minute; run more workers for throughput. Not in the image yet: add `src/worker.ts` to the Dockerfile's build line and a Deployment with the Docker socket mounted (see Ceilings before you do).

**What is per-replica.** Nothing: queue, results and artifacts are all Postgres. Two workers both sweep; `delete … where expires_at <= now` is idempotent.

**Failure modes.**

| What | Caller sees |
|---|---|
| Program exits non-zero | `done`, `run.code` non-zero, stderr in `run.stderr`. |
| Program exceeds `run_timeout` | `done`, `run.signal = "SIGKILL"`, `code = null`, partial output. |
| Out of memory | `done`, `code = 137`, usually empty stdout. |
| `docker` missing or image absent | `failed`, `error` from the spawn or docker's stderr. |
| No worker running | `/api/runs/:id` stays `queued`; `/api/v2/execute` answers `504 timeout` after 40 s. |
| Artifact requested after 1h | `404 no such artifact or expired`. |

**Metrics and logs.** `runs.ms` per language; queue depth (`state = 'queued'`); `failed` count by `error` prefix (`docker:` means the host, not the user); artifact bytes per hour (Postgres growth). The worker logs one line at start; add one per run if you need timings without a query.

## Ceilings

- **One container per run, cold.** Alpine images start in ~200–500 ms; that is the floor on latency. Piston keeps a warm process tree; E2B keeps a Firecracker VM. Pre-pull and consider `--init` and a pool before optimising elsewhere.
- **The timeout kills the docker client**, and the container is expected to die with it (`--rm`). Confirm on your daemon with `docker ps` after a timed-out run; if containers survive, name them (`--name run-<id>`) and `docker kill` in the timeout handler.
- **Artifacts live in Postgres.** Fine to a few MB per run; object storage after that.
- **`/api/v2/execute` holds a request** up to 40 s and polls the store 5×/s. It is there for Piston clients; new clients should queue and poll.
- **The worker needs the Docker socket.** On Kubernetes that is root on the node. The upgrade path is running each job as a Kubernetes Job with a gVisor or Kata runtime class — `dockerArgv` becomes a pod spec; nothing else changes.
- **No authentication, no quotas.** Anyone who can reach `:8000` gets 0.5 CPU × 30 s per request, as often as they like.
- **Two languages, one version each.** `version` in the body is accepted (`*`) and ignored.
- **The work dir is `mkdtemp` (mode 0700, the worker's uid) and the container runs as 65534.** On Docker Desktop the bind mount is mapped and writes work; on a Linux host `nobody` cannot create `out/` and artifacts silently never appear. `chmod 0777` the work dir in `dockerExecutor` (it is deleted after the run) or run the container as the worker's uid. The program must `mkdir out` itself either way.
- **No compile stage**, no packages, no interactive stdin, no persistent sandbox.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const SANDBOX_AGENTS_MD: &str = r##"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run it.

## Run

    make demo                              # postgres + redis, migrate, seed, API on :8000, page on :3000
    make check                             # typecheck both halves, bun test the backend — no Docker needed
    docker pull python:3.12-alpine node:22-alpine
    cd backend && bun run worker           # runs jobs; needs a Docker daemon

## Routes

Queue and poll:

    curl -s -X POST localhost:8000/api/runs -H 'content-type: application/json' \
      -d '{"language":"python","files":[{"name":"main.py","content":"import sys\nprint(sys.version.split()[0])\nimport os; os.makedirs(\"out\", exist_ok=True); open(\"out/hello.txt\",\"w\").write(\"hi\")"}],"stdin":"","args":[],"run_timeout":3000}'
    # 202 {"id":"9b1e…","state":"queued"}

    curl -s localhost:8000/api/runs/9b1e…
    # {"id":"9b1e…","language":"python","version":"3.12","state":"done","ms":412,
    #  "run":{"stdout":"3.12.7\n","stderr":"","code":0,"signal":null,"output":"3.12.7\n"},
    #  "artifacts":[{"id":"c4d0…","run_id":"9b1e…","name":"hello.txt","expires_at":"…"}]}

    curl -s -o hello.txt localhost:8000/api/artifacts/c4d0…       # attachment; 404 after an hour

Piston-compatible, blocking:

    curl -s -X POST localhost:8000/api/v2/execute -H 'content-type: application/json' \
      -d '{"language":"javascript","files":[{"name":"main.js","content":"console.log(1+1)"}]}'
    # {"language":"javascript","version":"22","run":{"stdout":"2\n","stderr":"","code":0,"signal":null,"output":"2\n"}}
    # 504 {"message":"no worker picked up the run","code":"timeout"} when no worker is running

Refused at the boundary (`400`, zod's flattened errors):

    -d '{"language":"cobol",…}'                       # unsupported language
    -d '{…,"run_timeout":60000}'                       # above LIMITS.timeout.max
    -d '{…,"files":[{"name":"../x","content":""}]}'   # bad file name

Runtimes and list:

    curl -s localhost:8000/api/runtimes    # [{"language":"python","version":"3.12","image":"python:3.12-alpine"}, …]
    curl -s localhost:8000/api/runs        # {"runs":[…]} latest 50, without files and stdin

## Tests

`backend/src/app.test.ts`, `bun test`, no Docker:

- **`memoryStore()`** stands in for Postgres.
- **`fakeExecutor(outputs?)`** pretends every program prints its first file, and returns the artifacts you give it.
- **`dockerArgv` is pure**, so the isolation flags are asserted as strings without running anything.
- **`tick(store, exec, now)`** takes the clock, so artifact expiry is tested by handing it a time an hour ago.

To add a test: `createApp(memoryStore())`, `post(app, body)`, `tick(store, fakeExecutor(...))`, then read `/api/runs/:id`. A test that needs Docker belongs in a separate, opt-in file (`executor.docker.test.ts`, skipped unless `DOCKER=1`), not in the gate.
"##;

const SANDBOX_README_MD: &str = r##"# {{NAME}}

Run untrusted code in a throwaway container with no network and hard limits, from one HTTP call.
Piston's API shape, your own service.

## What you get

- `POST /api/runs` → queued; `GET /api/runs/:id` → `{stdout, stderr, code, signal, output}` plus any files the program wrote to `out/`.
- `POST /api/v2/execute` — Piston's endpoint and response, blocking, so existing Piston clients work unchanged.
- Every run in a fresh container: `--network none`, memory and swap capped, `--cpus`, `--pids-limit 64`, read-only root, no capabilities, `no-new-privileges`, user `nobody`. Killed at `run_timeout`.
- Limits refused above the ceiling at the boundary (30 s, 512 MiB, 20 files, 200 KB per file), never silently clamped.
- Artifacts downloadable for an hour, then swept.
- A worker that scales separately from the API, and tests that run without Docker.
- Python 3.12 and Node 22, one line each to add more.

## Five minutes

    make demo
    docker pull python:3.12-alpine && (cd backend && bun run worker)

    curl -s -X POST localhost:8000/api/v2/execute -H 'content-type: application/json' \
      -d '{"language":"python","files":[{"name":"main.py","content":"print(sum(range(10)))"}]}'
    # {"language":"python","version":"3.12","run":{"stdout":"45\n","stderr":"","code":0,"signal":null,"output":"45\n"}}

    curl -s -X POST localhost:8000/api/v2/execute -H 'content-type: application/json' \
      -d '{"language":"python","files":[{"name":"main.py","content":"import urllib.request\nurllib.request.urlopen(\"https://example.com\")"}]}'
    # run.code 1, stderr ends "… Temporary failure in name resolution" — there is no network

    curl -s -X POST localhost:8000/api/v2/execute -H 'content-type: application/json' \
      -d '{"language":"python","files":[{"name":"main.py","content":"while True: pass"}],"run_timeout":1000}'
    # run.signal "SIGKILL", code null, after one second

    curl -s -X POST localhost:8000/api/runs -H 'content-type: application/json' \
      -d '{"language":"javascript","files":[{"name":"main.js","content":"const fs=require(\"fs\");fs.mkdirSync(\"out\",{recursive:true});fs.writeFileSync(\"out/a.json\",\"[1]\")"}]}'
    # {"id":"…","state":"queued"} — then GET /api/runs/<id> lists a.json under artifacts

Open `http://localhost:3000` for a playground.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | liveness |
| GET | `/api/health/ready` | none | readiness |
| GET | `/api/runtimes` | none | languages, versions, images |
| POST | `/api/runs` | none | queue a run → `202 {id, state}` |
| GET | `/api/runs` | none | latest 50 (no files/stdin) |
| GET | `/api/runs/:id` | none | the run and its unexpired artifacts |
| GET | `/api/artifacts/:id` | none | download; 404 once expired |
| POST | `/api/v2/execute` | none | Piston-compatible, blocks until done (≤ 40 s) |

Request body (both POSTs): `language`, `files[{name, content}]`, optional `stdin`, `args`, `run_timeout` (ms, ≤ 30000), `run_memory_limit` (bytes, ≤ 536870912), `version` (accepted, ignored).

## Compared with Piston / E2B

**Same shape.** Piston's `/api/v2/execute` request and `{language, version, run: {stdout, stderr, code, signal, output}}` response; `run_timeout` and `run_memory_limit` names; output capped per stream; a runtimes listing. E2B's idea of files coming back out of a run.

**Better here, specifically.**
- A queue between the request and the container: the API never blocks on Docker, and workers scale on queue depth.
- The isolation flags are one pure function (`dockerArgv`) with a test that asserts every one of them — a change that drops `--network none` fails `make check`.
- Ceilings are refused, not clamped: a caller asking for 60 s gets a 400 that says so, not 30 s and a surprise.
- Artifacts: anything under `out/` comes back with an expiry and a download route.
- One TypeScript codebase, typed to the page, tests without Docker, and the stack's manifests, migrations and secrets handling.

**Not here yet.**
- Piston has ~50 languages with selectable versions and a compile stage (`compile_timeout`, `compile_memory_limit`); this has two languages, one version each, run only.
- No package installation (Piston's `/api/v2/packages`); the container has no network, so dependencies must be baked into an image.
- No interactive sessions, no WebSocket streaming of output, no stdin after start.
- E2B's persistent sandboxes, filesystem API, SDKs and long-lived processes — none. A run here is one process, start to exit.
- Docker isolation, not Firecracker or gVisor. Good against ordinary code; not a boundary you should sell to strangers without a stronger runtime.
- No authentication, API keys or quotas.

## Production

- **Environments.** `DATABASE_URL`, `RUN_CPUS`. Separate databases per environment.
- **Processes.** `backend` (stateless, HPA) and `worker` (needs Docker). Add `src/worker.ts` to the Dockerfile's `bun build` line and a `k8s/base/worker.yaml` with no Service or HTTP probes. The worker needs `/var/run/docker.sock` or a DinD sidecar — both mean node-level privilege; see Roadmap.
- **Probes.** `/api/health`, `/api/health/ready` on the API. For the worker, alert on the oldest `queued` run's age.
- **Migrations.** `0002_sandbox.sql`; the migrate init container runs it before each rollout. `artifacts` grows by output volume — the sweep runs every minute from the worker.
- **Secrets.** None specific to this service; `DATABASE_URL` through `.env.age` → `k8s/secrets.yaml`.
- **Images.** Pre-pull `python:3.12-alpine` and `node:22-alpine` on every worker node, or the first run of each pays the pull.
- **Auth.** None. Put it behind ingress auth or an API key before exposing it; every request is 0.5 CPU for up to 30 s.
- **What pages you.** `queued` older than 60 s (no worker, or Docker down); `failed` with `error like 'docker%'` (host problem, not user code); artifact table size.

## Roadmap

- Kubernetes Jobs with a gVisor/Kata runtime class instead of the Docker socket.
- Compile stage and more languages; images with packages baked in.
- Artifacts to object storage.
- API keys and per-key quotas.
- Named containers and explicit `docker kill` on timeout.
- Streaming output over WebSocket.
"##;

const SANDBOX_AGENT_ISOLATION: &str = r##"---
name: isolation
description: Run on any change to executor.ts, worker.ts, the Body schema in app.ts, or LIMITS. Reads the change as someone who wants to escape the container, exhaust the host, or read another run's files, and reports only what works.
tools: Read, Grep, Glob, Bash
---

You are the attacker whose code is about to run here. Report only what gets you out, gets you
more than your share, or gets you someone else's data — each as `path:line — what — the payload — the fix`.

Check:
1. **`dockerArgv` flags.** `--network none`; `--memory` and `--memory-swap` equal (unequal means swap); `--cpus`; `--pids-limit`; `--read-only`; `--cap-drop ALL`; `--security-opt no-new-privileges`; `--user 65534:65534`; `--rm`. A flag made conditional is a flag that is off for some input.
2. **The bind mount.** `-v ${workDir}:/work` only. No second `-v`, no `--privileged`, no `--device`, no `/var/run/docker.sock`.
3. **File names.** `Body` regex `^[\w.-]+$` and `safeName` agree. `.` and `..` match `[\w.-]+` — confirm `writeFile(join(dir, ".."))` cannot escape (it targets the temp dir's parent as a file and fails, but check the current code still errors rather than writing).
4. **Limits at the boundary.** `Body` maxes read `LIMITS`; no code path enqueues without `Body.safeParse` (`/api/v2/execute` and `/api/runs` share `enqueue`).
5. **Output caps.** `CAP` per stream in `spawnCaptured`; `stdin` bounded by `Body` (200 000). A change that concatenates unbounded `data` events is a host OOM.
6. **Timeout actually kills.** The timer sends `SIGKILL` to the docker client. Run `docker ps` after a `while True: pass` run with `run_timeout: 1000` — a container still alive is a finding; the fix is `--name run-<id>` plus `docker kill` in the timer.
7. **Temp dir removal.** `rm(dir, {recursive: true, force: true})` in `finally`. Artifacts are read before it; a change that reads after leaks nothing but fails every run.
8. **Artifact reads.** Only `/work/out` is read back, via `readdir` (one level). A symlink in `out/` to `/etc/passwd` on the worker host: the container user cannot create one outside `/work`, but confirm `readFile` follows nothing outside `dir` — resolve and check, or use `lstat`.
9. **Artifact bytes are bounded.** They are not (no size check on `outputs`). A program writing 1 GB to `out/` puts 1 GB into Postgres. Flag this if the change touches artifacts; the fix is a size cap before `store.finish`.
10. **No retry.** `tick`'s catch marks `failed`; nothing sets `queued` again. A retry runs the user's code twice.
11. **The API never spawns.** `grep spawn\|execFile backend/src/app.ts` is empty.
12. **Expiry is enforced on read**, not only by the sweep: `artifact(id, now)` and `artifacts(runId)` both compare `expires_at`.
13. **Writable work dir.** `mkdtemp` is 0700 owned by the worker; the container user is 65534. On Linux the program cannot write `out/` — confirm with a run that writes an artifact on the target host, and `chmod` the dir in `dockerExecutor` if it comes back empty.
14. **Worker privilege.** If the change adds a Kubernetes manifest for the worker, the Docker socket mount is node root — say so, and point at Jobs with a sandboxed runtime class.

Run `cd backend && bun test`. End with `isolation: N findings` and, if 0, which of the above you read.
"##;

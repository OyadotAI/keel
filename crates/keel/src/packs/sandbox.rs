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

import { beforeEach, expect, it, vi } from "vitest";
import { install } from "./setup-install";
import { open } from "./streams";
vi.mock("./streams", () => ({ open: vi.fn() }));
beforeEach(() => vi.clearAllMocks());
const ep = { port: 1234, token: "secret" };
it("installs at project scope and waits for confirmed success", async () => {
  const done = install(ep, "test", "official");
  const [url, token, events, end] = vi.mocked(open).mock.calls[0];
  expect(url).toContain("name=test&marketplace=official&commit=true");
  expect(token).toBe("secret");
  events([{ event: "done", data: "0" }]); end();
  await expect(done).resolves.toBeUndefined();
});
it("keeps the installer failure instead of discarding its reason", async () => {
  const done = install(ep, "test", "official");
  const [, , events, end] = vi.mocked(open).mock.calls[0];
  events([{ event: "line", data: "Project is busy" }, { event: "done", data: "1" }]); end();
  await expect(done).resolves.toBe("Project is busy");
});
it("reports HTTP errors and incomplete streams", async () => {
  const failed = install(ep, "test", "official");
  vi.mocked(open).mock.calls[0][3]("Unauthorized");
  await expect(failed).resolves.toBe("Unauthorized");
  const incomplete = install(ep, "test", "official");
  vi.mocked(open).mock.calls[1][3]();
  await expect(incomplete).resolves.toMatch(/without confirming/);
});

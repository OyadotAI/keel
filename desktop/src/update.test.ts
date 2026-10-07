import { beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ check: vi.fn(), invoke: vi.fn(), relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: mocks.relaunch }));

beforeEach(() => { vi.resetModules(); vi.resetAllMocks(); });

it("downloads once, then stops agents, installs and relaunches only on request", async () => {
  const order: string[] = [];
  const download = vi.fn(async () => { order.push("download"); });
  const install = vi.fn(async () => { order.push("install"); });
  mocks.check.mockResolvedValue({ version: "9.0.0", download, install });
  mocks.invoke.mockImplementation(async () => { order.push("stop"); });
  mocks.relaunch.mockImplementation(async () => { order.push("relaunch"); });
  const update = await import("./update");
  await update.check();
  await update.check();
  expect(mocks.check).toHaveBeenCalledTimes(1);
  expect(order).toEqual(["download"]);
  await Promise.all([update.restart(), update.restart(), update.check()]);
  expect(order).toEqual(["download", "stop", "install", "relaunch"]);
  expect(mocks.invoke).toHaveBeenCalledWith("stop_all");
});

it("never stops agents or installs when the download failed", async () => {
  const install = vi.fn();
  mocks.check.mockResolvedValue({ version: "9.0.0", download: vi.fn().mockRejectedValue(new Error("offline")), install });
  const update = await import("./update");
  await update.check();
  await update.restart();
  expect(mocks.invoke).not.toHaveBeenCalled();
  expect(install).not.toHaveBeenCalled();
});

it("allows a fresh check after installation fails, without relaunching", async () => {
  const install = vi.fn().mockRejectedValueOnce(new Error("read-only volume")).mockResolvedValue(undefined);
  mocks.check.mockResolvedValue({ version: "9.0.0", download: vi.fn(), install });
  const update = await import("./update");
  await update.check();
  await update.restart();
  expect(mocks.relaunch).not.toHaveBeenCalled();
  await update.restart();
  expect(install).toHaveBeenCalledTimes(1);
  await update.check();
  await update.restart();
  expect(mocks.relaunch).toHaveBeenCalledTimes(1);
});

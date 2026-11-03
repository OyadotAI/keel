// Settings keeps project identity ahead of preferences and remains useful without a daemon.
import { beforeEach, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
const fake = vi.hoisted(() => ({ active: undefined as string | undefined, order: [] as string[], lanes: {} as Record<string, { project: string }>, projects: {} as Record<string, { name: string }>, trusted: {} }));
vi.mock("../store", () => ({ useStore: Object.assign((select: (state: typeof fake) => unknown) => select(fake), { getState: () => fake }), projectTrust: {} }));
vi.mock("./UpdateButton", () => ({ UpdateButton: () => null }));
vi.mock("./AppearanceSetting", () => ({ AppearanceSetting: () => <h2>Appearance</h2> }));
vi.mock("./NotificationSetting", () => ({ NotificationSetting: () => <h2>Notifications</h2> }));
vi.mock("./InterfaceSetting", () => ({ InterfaceSetting: () => <h2>Chat interface</h2> }));
vi.mock("./BrowserPreference", () => ({ BrowserPreference: () => <h2>Browser for coding agents</h2> }));
vi.mock("./SettingsServices", () => ({ until: () => "", ClaudeSection: () => <p>Connected</p>, LicenseSection: () => <h2>License</h2>, UpdatesSection: () => <button>Check for updates</button> }));
import { Settings } from "./Settings";

beforeEach(() => { fake.active = undefined; fake.order = []; fake.lanes = {}; fake.projects = {}; });

it("keeps project trust, all application groups and updates visible without an open project", () => {
  const html = renderToStaticMarkup(<Settings />);
  expect(html).toContain("Open a project to manage trust and permission rules.");
  for (const label of ["Appearance", "Notifications", "Chat interface", "Browser for coding agents", "Claude Code", "License", "Check for updates", "Done"]) expect(html).toContain(label);
  expect(html.indexOf('id="settings-trust"')).toBeLessThan(html.indexOf('id="settings-preferences"'));
  expect(html).not.toContain(">Trust this project…</button>");
});

it("identifies the new project while connecting and exposes no stale grant controls", () => {
  fake.active = "lane";
  fake.lanes = { lane: { project: "/scratch/new-project" } };
  fake.projects = { "/scratch/new-project": { name: "New project" } };
  const html = renderToStaticMarkup(<Settings />);
  expect(html).toContain("New project");
  expect(html).toContain("/scratch/new-project");
  expect(html).toContain("Connecting to this project…");
  expect(html).not.toContain(">Trust this project…</button>");
});

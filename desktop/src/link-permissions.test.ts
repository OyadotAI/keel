/** The native opener needs URL scope as well as permission to execute its command. */
import { expect, it } from "vitest";
import capability from "../src-tauri/capabilities/default.json?raw";

it("permits browser links without permitting arbitrary URL schemes", () => {
  const permissions = JSON.parse(capability).permissions;
  const opener = permissions.find((p: string | { identifier: string }) => typeof p !== "string" && p.identifier === "opener:allow-open-url");
  expect(opener?.allow).toEqual([{ url: "http://*" }, { url: "https://*" }]);
});

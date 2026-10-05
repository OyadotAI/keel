import { expect, test } from "vitest";
import { urlIn, whyStopped } from "./devlog";

test("the URL a dev server prints is found, the newest first", () => {
  expect(urlIn(["  ▲ Next.js 16", "  - Local:        http://localhost:3000", "ready"])).toBe("http://localhost:3000");
  expect(urlIn(["  ➜  Local:   \u001b[36mhttp://localhost:5173/\u001b[39m"])).toBe("http://localhost:5173/");
  expect(urlIn(["see https://nextjs.org/docs"])).toBeUndefined();
});

test("a missing dependency is named as one", () => {
  expect(whyStopped(["> next dev", "sh: next: command not found"])).toBe("Its dependencies are not installed.");
  expect(whyStopped(["Error: listen EADDRINUSE: address already in use :::3000"])).toBe("Its port is already taken by another process.");
  expect(whyStopped([])).toBeUndefined();
});

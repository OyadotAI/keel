import { expect, it, vi } from "vitest";
import { getBlob, query } from "./api";

// The daemon reads a bare `+` as a space; "C++" must arrive as "C++".
it("encodes a plus so the daemon does not read it as a space", () => {
  expect(query({ prompt: "C++ & a=b" })).toBe("?prompt=C%2B%2B%20%26%20a%3Db");
  expect(query({ a: undefined, b: "", c: 1 })).toBe("?c=1");
});

it("loads local preview images with header authentication and keeps tokens out of URLs", async () => {
  const fetch = vi.fn().mockResolvedValue(new Response(new Blob(["image"], { type: "image/png" })));
  vi.stubGlobal("fetch", fetch);
  try {
    const blob = await getBlob({ port: 1234, token: "private-image-token" }, "/api/raw", { path: "assets/a b.png", wt: "feature" });
    expect(blob.type).toBe("image/png");
    const [url, options] = fetch.mock.calls[0];
    expect(url).toContain("path=assets%2Fa%20b.png");
    expect(url).toContain("wt=feature");
    expect(url).not.toContain("private-image-token");
    expect(options.headers.Authorization).toBe("Bearer private-image-token");
    expect(options.signal).toBeInstanceOf(AbortSignal);
  } finally { vi.unstubAllGlobals(); }
});

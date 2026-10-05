import { expect, test } from "vitest";
import { prompt, type Pin } from "./pins";

const pin = (over: Partial<Pin>): Pin => ({
  id: "1",
  note: "make it 320px wide",
  selector: "main > button.cta",
  exact: true,
  tag: "button",
  text: "Get started",
  rect: { x: 0, y: 0, width: 240, height: 40 },
  sources: [{ kind: "inspector", file: "src/components/Hero.tsx", line: 42 }],
  styles: {},
  print: "abc",
  url: "http://localhost:3000/",
  ...over,
});

test("a pin becomes its note, its element and its ranked source", () => {
  const p = prompt([pin({})]);
  expect(p).toContain("1. make it 320px wide");
  expect(p).toContain("<button> `main > button.cta` — \"Get started\"");
  expect(p).toContain("src/components/Hero.tsx:42 (inspector)");
  expect(p).toContain("Do not copy or fork a component");
});

test("a guess reads as a guess, and no hint says so", () => {
  const p = prompt([pin({ exact: false, sources: [], note: "" })]);
  expect(p).toContain("(not unique — the closest selector found)");
  expect(p).toContain("no hint — find it from the selector and text");
  expect(p).toContain("(no note — ask what to change)");
});

test("nothing from the page reaches the terminal as a control sequence or a second line", () => {
  const p = prompt([pin({ text: "hi\u001b[201~\nrm -rf ~\r", selector: "a\nb", sources: [{ kind: "component", name: "X\u0007" }] })]);
  // eslint-disable-next-line no-control-regex -- asserting there are none.
  expect(p).not.toMatch(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/);
  expect(p).toContain('"hi [201~ rm -rf ~"');
  expect(p).toContain("`a b`");
});

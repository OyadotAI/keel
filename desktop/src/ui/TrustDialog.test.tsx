// Static dialog contracts retain the full permission disclosure and safe action ordering.
import { expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
vi.mock("../store", () => ({ projectTrust: {}, useStore: () => undefined }));
import { TrustDialogView } from "./TrustDialog";
import type { TrustRequest } from "../project-trust";

const render = (phase: TrustRequest["phase"], error?: string) => renderToStaticMarkup(<TrustDialogView request={{ project: "/scratch/long project", phase, error }} name="Example" confirm={() => undefined} retry={() => undefined} close={() => undefined} />);

it("identifies the captured project and explains every standing permission boundary", () => {
  const html = render("ready");
  for (const text of ["Trust this project?", "Example", "/scratch/long project", "installs, deletes, network calls and Keel Browser actions", "official marketplace", "Explicit deny rules still apply", "Questions and plan decisions", "Stop trusting", "Settings → Project trust", "Read, Edit and Write path deny rules do not apply"]) expect(html).toContain(text);
  expect(html).toContain('role="dialog" aria-modal="true"');
  expect(html.indexOf(">Keep asking</button>")).toBeLessThan(html.indexOf(">Trust this project</button>"));
});

it("connecting and saving disable grants, while ready and failed writes remain actionable", () => {
  expect(render("connecting")).toContain('<button class="primary" disabled="">Trust this project</button>');
  expect(render("saving")).toContain('<button disabled="">Keep asking</button>');
  expect(render("saving")).toContain('<button class="primary" disabled="">Trusting…</button>');
  expect(render("saving")).toContain('<p tabindex="0" role="status">Saving project trust…</p>');
  expect(render("ready", "Disconnected")).toContain('role="alert"');
  expect(render("ready", "Disconnected")).toContain('<button class="primary">Trust this project</button>');
  expect(render("failed", "Connection refused")).toContain(">Try again</button>");
});

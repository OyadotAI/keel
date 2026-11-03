// Settings navigation exposes trust first and keeps every settings group mounted without tabs.
import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { SettingsLayout } from "./SettingsLayout";

it("puts trust before all preferences and retains every labelled section in the same page", () => {
  const html = renderToStaticMarkup(<SettingsLayout trust={<h2 id="settings-trust">Project trust</h2>} preferences={<input aria-label="Preference draft" defaultValue="retained" />} claude={<p>Login output</p>} license={<input aria-label="License key" defaultValue="unsaved" />} updates={<button>Check for updates</button>} />);
  expect(html).toContain('aria-label="Settings sections"');
  for (const id of ["trust", "preferences", "claude", "license"]) expect(html).toContain(`href="#settings-${id}"`);
  expect(html.indexOf('id="settings-trust"')).toBeLessThan(html.indexOf('id="settings-preferences"'));
  for (const text of ["Login output", "License key", "unsaved", "Check for updates", "Preference draft", "retained"]) expect(html).toContain(text);
  expect(html).not.toContain('role="tab"');
  expect(html).toContain('id="settings-preferences" tabindex="-1"');
});

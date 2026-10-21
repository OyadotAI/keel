// The lane's Browser tab, driven by the agent. Evaluated once per tool call as
// `(<this>)(<op>)`; it returns a string synchronously, because `eval_with_callback` serialises the
// value at once and a Promise arrives as "". Waiting happens on the Rust side.
//
// Everything it draws carries `data-keel`, as the preview's picker does, so the page never reads
// Keel's marks back as its own content.
(op) => {
  const MAX_TEXT = 20000;
  const MAX_ELEMENTS = 300;
  const say = (ok, text) => JSON.stringify({ ok, text });

  const visible = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    const s = getComputedStyle(el);
    return s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0;
  };
  const role = (el) => el.getAttribute("role") || ({ A: "link", BUTTON: "button", SELECT: "select", TEXTAREA: "textbox" })[el.tagName] || (el.tagName === "INPUT" ? (el.type || "text") : el.tagName.toLowerCase());
  // A password's value never leaves the page; neither does what the person typed into any field.
  const name = (el) => {
    const label = el.getAttribute("aria-label") || (el.labels && el.labels[0] && el.labels[0].innerText) || el.getAttribute("placeholder") || el.getAttribute("title") || el.getAttribute("name") || "";
    const text = el.tagName === "INPUT" || el.tagName === "TEXTAREA" ? "" : (el.innerText || "");
    return (label || text || el.getAttribute("alt") || "").replace(/\s+/g, " ").trim().slice(0, 80);
  };
  const find = (n) => document.querySelector(`[data-keel-ref="${Number(n)}"]`);
  // One green that holds on a light page and a dark one alike: the page cannot read Keel's theme.
  const GREEN = "#1f7a52";
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const mark = (el) => {
    const r = el.getBoundingClientRect();
    const box = document.createElement("div");
    box.setAttribute("data-keel", "browser-mark");
    box.style.cssText = `position:fixed;left:${r.left - 3}px;top:${r.top - 3}px;width:${r.width + 6}px;height:${r.height + 6}px;border:2px solid ${GREEN};box-shadow:0 0 0 4px rgba(31,122,82,.22);background:rgba(31,122,82,.10);border-radius:6px;pointer-events:none;z-index:2147483647`;
    document.documentElement.appendChild(box);
    const frames = still
      ? [{ opacity: 0 }, { opacity: 1, offset: 0.15 }, { opacity: 1, offset: 0.7 }, { opacity: 0 }]
      : [{ opacity: 0, transform: "scale(1.06)" }, { opacity: 1, transform: "scale(1)", offset: 0.15 }, { opacity: 1, offset: 0.7 }, { opacity: 0 }];
    box.animate(frames, { duration: 1500, easing: "ease-out" }).onfinish = () => box.remove();
  };
  const missing = (n) => say(false, `No element ${n} on the page now. The page changed since your last browser_read; read it again.`);

  try {
    return run();
  } catch (e) {
    return say(false, `The page refused that: ${String(e && e.message ? e.message : e).slice(0, 200)}`);
  }

  function run() {
  switch (op.op) {
    case "read": {
      document.querySelectorAll("[data-keel-ref]").forEach((el) => el.removeAttribute("data-keel-ref"));
      const els = [...document.querySelectorAll("a[href],button,input:not([type=hidden]),select,textarea,[role=button],[role=link],[role=tab],[role=menuitem],[role=checkbox],[contenteditable=true],[onclick]")].filter(visible).slice(0, MAX_ELEMENTS);
      const lines = els.map((el, i) => {
        el.setAttribute("data-keel-ref", String(i + 1));
        const href = (el.getAttribute("href") || "").slice(0, 200);
        const options = el.tagName === "SELECT" ? ` options: ${[...el.options].slice(0, 20).map((o) => `"${o.text.trim().slice(0, 40)}"`).join(", ")}` : "";
        const extra = el.tagName === "INPUT" && (el.type === "checkbox" || el.type === "radio") ? (el.checked ? " (checked)" : " (not checked)") : el.tagName === "A" ? ` → ${href}` : options;
        return `[${i + 1}] ${role(el)} "${name(el)}"${extra}`;
      });
      // Show the analysis: each element the agent can now name gets its number, for a moment, so
      // the person sees what was read and what [n] the agent means when it clicks.
      document.querySelectorAll('[data-keel="browser-ref"]').forEach((b) => b.remove());
      const layer = document.createElement("div");
      layer.setAttribute("data-keel", "browser-ref");
      layer.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
      const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
      els.forEach((el, i) => {
        const r = el.getBoundingClientRect();
        if (r.width < 6 || r.height < 6 || r.bottom < 0 || r.top > innerHeight || r.right < 0 || r.left > innerWidth) return;
        const tag = document.createElement("span");
        tag.textContent = String(i + 1);
        tag.style.cssText = `position:fixed;left:${clamp(r.left - 6, 2, innerWidth - 22)}px;top:${clamp(r.top - 6, 2, innerHeight - 18)}px;font:600 10px/16px -apple-system,system-ui,sans-serif;font-variant-numeric:tabular-nums;color:#fff;background:${GREEN};height:16px;min-width:16px;padding:0 5px;box-sizing:border-box;border-radius:8px;text-align:center;box-shadow:0 0 0 1.5px #fff,0 1px 3px rgba(0,0,0,.35)`;
        const box = document.createElement("span");
        box.style.cssText = `position:fixed;left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;outline:1.5px solid rgba(31,122,82,.9);outline-offset:1px;background:rgba(31,122,82,.08);border-radius:4px`;
        layer.append(box, tag);
      });
      document.documentElement.appendChild(layer);
      layer.animate([{ opacity: 0 }, { opacity: 1, offset: 0.05 }, { opacity: 1, offset: 0.75 }, { opacity: 0 }], { duration: 2400 }).onfinish = () => layer.remove();
      const body = (document.body && document.body.innerText) || "";
      const text = body.length > MAX_TEXT ? `${body.slice(0, MAX_TEXT)}\n… ${body.length - MAX_TEXT} more characters` : body;
      return say(true, `# ${document.title}\n${location.href}\n\n${text}\n\n## Elements\n${lines.join("\n") || "(none)"}`);
    }
    case "click": {
      const el = find(op.target);
      if (!el) return missing(op.target);
      el.scrollIntoView({ block: "center" });
      mark(el);
      el.click();
      return say(true, `Clicked [${op.target}] ${role(el)} "${name(el)}".`);
    }
    case "type": {
      const el = find(op.target);
      if (!el) return missing(op.target);
      el.scrollIntoView({ block: "center" });
      mark(el);
      el.focus();
      if (el.tagName === "SELECT") {
        // Typing into a select chooses the option whose text or value it names.
        const want = op.text.trim().toLowerCase();
        const option = [...el.options].find((o) => o.text.trim().toLowerCase() === want || o.value.toLowerCase() === want);
        if (!option) return say(false, `[${op.target}] has no option with that text or value; read the page for its options.`);
        el.value = option.value;
        el.dispatchEvent(new Event("input", { bubbles: true }));
        el.dispatchEvent(new Event("change", { bubbles: true }));
        return say(true, `Chose "${option.text.trim()}" in [${op.target}].`);
      }
      if (el.isContentEditable) {
        el.textContent = op.text;
      } else {
        // The native setter, so a framework that tracks the value sees the change.
        const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        const set = Object.getOwnPropertyDescriptor(proto, "value");
        if (set && set.set) set.set.call(el, op.text); else el.value = op.text;
      }
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      if (op.submit) {
        const form = el.form || el.closest("form");
        if (form && form.requestSubmit) form.requestSubmit();
        else el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      }
      return say(true, `Typed ${op.text.length} characters into [${op.target}] "${name(el)}"${op.submit ? " and submitted" : ""}.`);
    }
    case "key": {
      const el = document.activeElement || document.body;
      const key = op.key === "Space" ? " " : op.key;
      // Cancelable, so a page that handles the key itself (an autocomplete choosing a suggestion,
      // a chat box sending) can say so, and nothing below acts a second time.
      const handled = !el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
      el.dispatchEvent(new KeyboardEvent("keyup", { key, bubbles: true, cancelable: true }));
      // A synthetic key has no default action, so the browser's own is done here, only where the
      // browser itself would do it: Enter in a text field submits its form; Enter on a native
      // button or link, and Space on a button or checkbox, presses it.
      if (!handled && op.key === "Enter" && el.tagName === "INPUT" && /^(text|search|email|url|tel|password|number)$/.test(el.type) && el.form && el.form.requestSubmit) {
        el.form.requestSubmit();
        return say(true, "Pressed Enter, which submitted the form.");
      }
      if (!handled && ((op.key === "Enter" && /^(A|BUTTON)$/.test(el.tagName)) || (op.key === "Space" && (el.tagName === "BUTTON" || (el.tagName === "INPUT" && /^(checkbox|radio|button|submit)$/.test(el.type)))))) {
        el.click();
        return say(true, `Pressed ${op.key} on ${role(el)} "${name(el)}".`);
      }
      return say(true, handled ? `Pressed ${op.key}; the page handled it.` : `Pressed ${op.key}. Only the page's own key handlers see a key from here: Tab does not move focus and Backspace does not delete. Click an element, or type into a field, instead.`);
    }
    case "scroll": {
      window.scrollBy(0, (op.direction === "up" ? -1 : 1) * Math.round(innerHeight * 0.9));
      return say(true, `Scrolled ${op.direction}; now ${Math.round(scrollY)} of ${Math.max(0, document.documentElement.scrollHeight - innerHeight)} px.`);
    }
    case "back": {
      history.back();
      return say(true, "Went back.");
    }
    case "forward": {
      history.forward();
      return say(true, "Went forward.");
    }
    case "reload": {
      location.reload();
      return say(true, "Reloaded.");
    }
    default:
      return say(false, `Unknown op ${op.op}.`);
  }
  }
}

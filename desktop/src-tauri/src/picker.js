// The design picker, installed by the host into every frame of the preview (preview.rs). It aims,
// pins and re-locates elements; it never edits the page. Everything it draws carries `data-keel`
// so it never reports itself as part of the page.
//
// Messages in (window.__keelPicker): {arm}, {disarm}, {clear}, {probe: [{id, selector}]}, {pins},
// {work: {on, file}} — the lane's turn is editing the frontend: ripple what re-renders until it ends.
// Messages out (preview_msg): {type: "pick", pin}, {type: "probe", results}, {type: "armed", on}.
(() => {
  if (window.__keelPickerInstalled) return;
  window.__keelPickerInstalled = true;
  // Only the top frame speaks to Keel: a pick inside an iframe is forwarded up with its offset.
  const top = window === window.top;
  let armed = false;
  let hovered = null;
  let pins = [];
  const send = (msg) => {
    if (top) window.__TAURI_INTERNALS__?.invoke("preview_msg", { msg }).catch(() => {});
    else window.parent.postMessage({ __keel: msg }, "*");
  };
  // Picks travel up from any depth: each frame composes its child frame's offset and passes the
  // pick to its own parent, until the top one speaks to Keel. Only from a frame this page actually
  // holds, and only while picking — anything else that posts a "pick" is a page speaking for the
  // picker. (Two levels down used to do nothing at all: the middle frame relayed neither way.)
  window.addEventListener("message", (e) => {
    const m = e.data && e.data.__keel;
    if (!m || m.type !== "pick" || !armed) return;
    const from = [...document.querySelectorAll("iframe")].find((f) => f.contentWindow === e.source);
    if (!from || !m.pin || typeof m.pin !== "object") return;
    const r = from.getBoundingClientRect();
    const rect = m.pin.rect || {};
    const pin = { ...m.pin, rect: { ...rect, x: (rect.x || 0) + r.x, y: (rect.y || 0) + r.y }, frame: m.pin.frame || from.src || "(inline frame)" };
    send({ type: "pick", pin });
  });
  // A child frame picks while the top one does: told by its parent, and by nobody else, and it
  // tells its own frames in turn.
  if (!top)
    window.addEventListener("message", (e) => {
      if (e.source !== window.parent || !e.data || typeof e.data.__keelArm !== "boolean") return;
      if (e.data.__keelArm) {
        armed = true;
        document.documentElement.style.cursor = "crosshair";
      } else disarm();
      tellFrames(e.data.__keelArm);
    });
  const tellFrames = (on) => {
    for (const f of document.querySelectorAll("iframe")) f.contentWindow?.postMessage({ __keelArm: on }, "*");
  };

  const layer = () => {
    let el = document.getElementById("__keel_layer");
    if (!el) {
      el = document.createElement("div");
      el.id = "__keel_layer";
      el.setAttribute("data-keel", "");
      el.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
      (document.body || document.documentElement).appendChild(el);
    }
    return el;
  };
  const box = (id, rect, color, label) => {
    let el = layer().querySelector(`[data-id="${id}"]`);
    if (!el) {
      el = document.createElement("div");
      el.setAttribute("data-keel", "");
      el.setAttribute("data-id", id);
      el.innerHTML = '<span data-keel style="position:absolute;left:-1px;top:-20px;font:600 11px/18px -apple-system,system-ui,sans-serif;padding:0 6px;border-radius:4px;white-space:nowrap;color:#fff"></span>';
      layer().appendChild(el);
    }
    el.style.cssText = `position:fixed;left:${rect.x}px;top:${rect.y}px;width:${rect.width}px;height:${rect.height}px;outline:2px solid ${color};background:${color}14;border-radius:2px`;
    const tag = el.firstChild;
    tag.textContent = label;
    tag.style.background = color;
    if (rect.y < 22) tag.style.top = `${rect.height + 2}px`;
    return el;
  };
  const unbox = (id) => layer().querySelector(`[data-id="${id}"]`)?.remove();

  // ── What an element is ──────────────────────────────────────────────────
  const HASHED = /^(css|sc|jsx|svelte|emotion|chakra)-[a-z0-9]+$|^[a-zA-Z]+_[a-zA-Z0-9]{5,}$|^_[a-zA-Z0-9]{5,}$/;
  const esc = (s) => (window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/[^\w-]/g, "\\$&"));
  const unique = (sel) => {
    try {
      return document.querySelectorAll(sel).length === 1;
    } catch {
      return false;
    }
  };
  /// Widened until it matches exactly one node; `exact: false` when it never gets there.
  function selector(el) {
    if (el.id && unique(`#${esc(el.id)}`)) return { css: `#${esc(el.id)}`, exact: true };
    const testid = el.getAttribute("data-testid");
    if (testid && unique(`[data-testid="${testid}"]`)) return { css: `[data-testid="${testid}"]`, exact: true };
    const parts = [];
    for (let node = el; node && node.nodeType === 1 && node !== document.documentElement; node = node.parentElement) {
      let part = node.tagName.toLowerCase();
      // Tailwind's classes are kept and escaped; framework hash classes are dropped.
      const classes = [...node.classList].filter((c) => !HASHED.test(c)).slice(0, 4);
      if (classes.length) part += classes.map((c) => `.${esc(c)}`).join("");
      const parent = node.parentElement;
      if (parent) {
        const same = [...parent.children].filter((c) => c.tagName === node.tagName);
        if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
      }
      parts.unshift(part);
      const css = parts.join(" > ");
      if (unique(css)) return { css, exact: true };
      if (parts.length >= 8) break;
    }
    return { css: parts.join(" > "), exact: false };
  }
  /// Where it came from, best first. React 19 has no `_debugSource`, so the owning component is
  /// the common answer; a ranked guess is honest where a silent one is not.
  function sources(el) {
    const out = [];
    const add = (kind, file, line, name) => {
      if (out.length < 5 && !out.some((s) => s.kind === kind && s.file === file && s.name === name)) out.push({ kind, file, line, name });
    };
    for (let node = el, depth = 0; node && depth < 6; node = node.parentElement, depth++) {
      const path = node.getAttribute && (node.getAttribute("data-inspector-relative-path") || node.getAttribute("data-source-file"));
      if (path) add("inspector", path, Number(node.getAttribute("data-inspector-line") || node.getAttribute("data-source-line")) || undefined);
      const svelte = node.__svelte_meta && node.__svelte_meta.loc;
      if (svelte) add("svelte", svelte.file, svelte.line + 1);
      const vue = node.__vueParentComponent;
      if (vue && vue.type) add("vue", vue.type.__file, undefined, vue.type.name || vue.type.__name);
      const key = Object.keys(node).find((k) => k.startsWith("__reactFiber$"));
      if (key) {
        for (let f = node[key], hops = 0; f && hops < 12; f = f.return, hops++) {
          if (f._debugSource) add("react", f._debugSource.fileName, f._debugSource.lineNumber);
          const t = f.type;
          const name = t && typeof t !== "string" && (t.displayName || t.name);
          if (name && !/^(Fragment|Suspense|Provider|Consumer)$/.test(name)) add("component", undefined, undefined, name);
          if (out.length >= 3) break;
        }
      }
      if (out.length >= 3) break;
    }
    const testid = el.closest && el.closest("[data-testid]");
    if (testid) add("testid", undefined, undefined, testid.getAttribute("data-testid"));
    return out;
  }
  const STYLES = ["display", "position", "width", "height", "margin", "padding", "color", "background-color", "font-size", "font-weight", "line-height", "border", "border-radius", "gap", "flex-direction", "justify-content", "align-items", "opacity", "box-shadow", "text-align"];
  function styles(el) {
    const cs = getComputedStyle(el);
    return Object.fromEntries(STYLES.map((p) => [p, cs.getPropertyValue(p)]));
  }
  /// What "did the edit reach the page" is compared on: the element's markup, its computed style
  /// and its size. Identical before and after a turn that was asked to change it means the edit
  /// went somewhere that does not render here.
  function print(el) {
    const r = el.getBoundingClientRect();
    const text = el.outerHTML.slice(0, 20000) + JSON.stringify(styles(el)) + `${Math.round(r.width)}x${Math.round(r.height)}`;
    let h = 0x811c9dc5;
    for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193);
    return (h >>> 0).toString(16);
  }
  const rect = (el) => {
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  };
  const describe = (el) => {
    const s = selector(el);
    return {
      selector: s.css,
      exact: s.exact,
      tag: el.tagName.toLowerCase(),
      text: (el.innerText || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 120),
      rect: rect(el),
      sources: sources(el),
      styles: styles(el),
      print: print(el),
      url: location.href,
    };
  };
  const label = (el) => {
    const r = el.getBoundingClientRect();
    const s = sources(el)[0];
    const hint = s ? ` · ${s.file ? s.file.split("/").pop() : s.name}` : "";
    return `${el.tagName.toLowerCase()} ${Math.round(r.width)}×${Math.round(r.height)}${hint}`;
  };
  const isOurs = (el) => !el || (el.closest && el.closest("[data-keel]"));

  // ── Aiming ──────────────────────────────────────────────────────────────
  function aim(el) {
    hovered = el;
    if (el) box("hover", rect(el), "#157a13", label(el));
    else unbox("hover");
  }
  const move = (e) => {
    if (!armed) return;
    const el = document.elementFromPoint(e.clientX, e.clientY);
    if (!isOurs(el) && el !== hovered) aim(el);
  };
  const click = (e) => {
    if (!armed || isOurs(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
    const el = hovered || e.target;
    send({ type: "pick", pin: describe(el) });
  };
  const key = (e) => {
    if (!armed) return;
    if (e.key === "Escape") {
      disarm();
      if (top) tellFrames(false);
      send({ type: "armed", on: false });
    } else if (e.key === "ArrowUp" && hovered && hovered.parentElement && hovered.parentElement !== document.documentElement) {
      e.preventDefault();
      aim(hovered.parentElement);
    } else if (e.key === "ArrowDown" && hovered && hovered.firstElementChild) {
      e.preventDefault();
      aim(hovered.firstElementChild);
    } else if (e.key === "Enter" && hovered) {
      e.preventDefault();
      send({ type: "pick", pin: describe(hovered) });
    }
  };
  // Capture phase, so the page's own handlers never see a click meant as a pick.
  for (const [t, f] of [["mousemove", move], ["click", click], ["keydown", key]]) window.addEventListener(t, f, true);
  for (const t of ["mousedown", "mouseup", "pointerdown", "pointerup"])
    window.addEventListener(t, (e) => armed && !isOurs(e.target) && (e.preventDefault(), e.stopPropagation()), true);

  function disarm() {
    armed = false;
    aim(null);
    document.documentElement.style.cursor = "";
  }
  function drawPins() {
    for (const el of layer().querySelectorAll('[data-id^="pin-"]')) el.remove();
    pins.forEach((p, i) => {
      let el = null;
      try {
        el = document.querySelector(p.selector);
      } catch {}
      if (el) box(`pin-${p.id}`, rect(el), "#2f5fb3", `${i + 1}`);
    });
  }
  let redraw = 0;
  const later = () => {
    cancelAnimationFrame(redraw);
    redraw = requestAnimationFrame(drawPins);
  };
  window.addEventListener("scroll", later, true);
  window.addEventListener("resize", later);

  // ── The agent at work ───────────────────────────────────────────────────
  // While the lane's turn runs and has written a frontend file, every region the page actually
  // re-renders gets a small swarm of dots walking its outline. Observed from the DOM, never
  // inferred from the file: no framework knowledge, and nothing drawn that did not happen.
  const ACCENT = "#2f5fb3";
  const ANTS = 10;
  const LAP_MS = 1600;
  const SWARM_MS = 2600;
  const REGIONS = 6;
  const DONE_MS = 1800;
  const calm = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
  let watcher = null;
  let dirty = new Set();
  let flush = 0;
  const swarmed = new WeakMap();
  const make = (css, parent = layer()) => {
    const el = document.createElement("div");
    el.setAttribute("data-keel", "");
    el.style.cssText = css;
    parent.appendChild(el);
    return el;
  };
  const fade = (el, ms) => el.animate([{ opacity: 1 }, { opacity: 0 }], { duration: ms, fill: "forwards" }).finished.then(() => el.remove(), () => el.remove());
  /// Dots that walk the rect's perimeter, spread along it by negative delays, then fade.
  function swarm(r) {
    const box = make(`position:fixed;left:${r.x}px;top:${r.y}px;width:${r.width}px;height:${r.height}px;outline:1.5px dashed ${ACCENT}99;background:${ACCENT}0d;border-radius:3px`);
    if (!calm) {
      const { width: w, height: h } = r;
      const p = 2 * (w + h);
      const at = (x, y, offset) => ({ transform: `translate(${x - 2.5}px,${y - 2.5}px)`, offset });
      const lap = [at(0, 0, 0), at(w, 0, w / p), at(w, h, (w + h) / p), at(0, h, (2 * w + h) / p), at(0, 0, 1)];
      for (let i = 0; i < ANTS; i++) {
        const ant = make(`position:absolute;left:0;top:0;width:5px;height:5px;border-radius:50%;background:${ACCENT};box-shadow:0 0 4px ${ACCENT}`, box);
        ant.animate(lap, { duration: LAP_MS, iterations: Infinity, delay: (-i * LAP_MS) / ANTS });
      }
    }
    setTimeout(() => fade(box, 400), SWARM_MS);
  }
  const visible = (r) => r.width > 2 && r.height > 2 && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth;
  function ripple() {
    const now = Date.now();
    const els = [...dirty].filter((el) => el.isConnected && ![...dirty].some((o) => o !== el && o.contains(el)));
    dirty = new Set();
    let n = 0;
    for (const el of els) {
      if (n >= REGIONS) break;
      if (now - (swarmed.get(el) || 0) < SWARM_MS) continue;
      const r = el.getBoundingClientRect();
      if (!visible(r)) continue;
      swarmed.set(el, now);
      swarm(r);
      n++;
    }
  }
  const note = (records) => {
    for (const m of records) {
      for (const node of m.type === "childList" && (m.target === document.body || m.target === document.documentElement) ? m.addedNodes : [m.target]) {
        const el = node.nodeType === 1 ? node : node.parentElement;
        if (el && !isOurs(el) && el !== document.body && el !== document.documentElement) dirty.add(el);
      }
    }
    cancelAnimationFrame(flush);
    flush = requestAnimationFrame(ripple);
  };
  /// The pill at the bottom of the page: who is working on what, then that it finished.
  function pill(text, done) {
    layer().querySelector('[data-id="work"]')?.remove();
    const el = make(`position:fixed;left:50%;bottom:16px;transform:translateX(-50%);display:flex;align-items:center;gap:8px;padding:6px 12px;border-radius:999px;background:#1d2433e6;color:#fff;font:500 12px/16px -apple-system,system-ui,sans-serif;box-shadow:0 4px 16px #0003;white-space:nowrap;max-width:80vw;overflow:hidden;text-overflow:ellipsis`);
    el.setAttribute("data-id", "work");
    const dots = make("display:flex;gap:3px", el);
    for (let i = 0; i < 3; i++) {
      const d = make(`width:5px;height:5px;border-radius:50%;background:${done ? "#5ec46a" : "#8fb0ff"}`, dots);
      if (!done && !calm) d.animate([{ transform: "translateY(0)", opacity: 0.4 }, { transform: "translateY(-3px)", opacity: 1 }, { transform: "translateY(0)", opacity: 0.4 }], { duration: 900, iterations: Infinity, delay: i * 150 });
    }
    el.appendChild(document.createTextNode(text));
    if (done) setTimeout(() => fade(el, 400), DONE_MS);
  }
  function work(m) {
    if (m.on) {
      pill(m.file ? `Agent editing ${String(m.file).split("/").pop()}` : "Agent working", false);
      if (!watcher) {
        watcher = new MutationObserver(note);
        watcher.observe(document.documentElement, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ["class", "src", "href"] });
      }
    } else {
      watcher?.disconnect();
      watcher = null;
      dirty = new Set();
      if (layer().querySelector('[data-id="work"]')) pill("Done", true);
    }
  }

  if (!top) return;
  window.__keelPicker = (m) => {
    if (m.work) work(m.work);
    if (m.arm) {
      armed = true;
      document.documentElement.style.cursor = "crosshair";
      tellFrames(true);
    }
    if (m.disarm) {
      disarm();
      tellFrames(false);
    }
    if (m.pins) {
      pins = m.pins;
      drawPins();
    }
    if (m.clear) {
      pins = [];
      drawPins();
    }
    if (m.probe) {
      // Located again now, not where they were at pick time: anything that scrolled or reflowed in
      // between would otherwise be compared at a square of the viewport that now holds something else.
      const results = m.probe.map((p) => {
        let el = null;
        try {
          el = document.querySelector(p.selector);
        } catch {}
        return el ? { id: p.id, found: true, print: print(el), rect: rect(el) } : { id: p.id, found: false };
      });
      send({ type: "probe", results });
    }
  };
})();

// Browser-owned WK diagnostic runtime, generated from Oya browser/src/preload/debug-webkit.ts.
"use strict";
(() => {
  // browser/src/shared/browser-debug.ts
  var BROWSER_DEBUG = {
    REGISTER: "oya-debug-document",
    REQUEST: "oya-debug-request",
    RESPONSE: "oya-debug-response",
    MESSAGE: "oya-debug-owner",
    MAX_EXPRESSION: 32768,
    MAX_JSON: 65536,
    MAX_REPLY_BYTES: 262144,
    MAX_URL: 4096,
    MAX_FRAMES: 128,
    MAX_DEPTH: 16,
    MAX_ELEMENTS: 300,
    MAX_TEXT: 12e3,
    REQUEST_MS: 1500,
    EVAL_MS: 5e3,
    OWNER_MS: 1e3,
    OPERATION_MS: 13e3,
    MAX_PENDING: 128,
    MAX_LABEL: 120,
    MAX_OPTIONS: 8,
    PROFILE_ID_CHARS: 20,
    MAX_ERROR: 1e3,
    TOKEN_WORDS: 4,
    CENTRE: 2,
    MAX_ELEMENT_ID: 4294967295,
    LINE_RESERVE: 32,
    UTF8_MASK: 192,
    UTF8_CONTINUATION: 128
  };

  // browser/src/preload/debug-geometry.ts
  function debugRect(rect) {
    return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
  }
  function debugHit(x, y) {
    let hit = document.elementFromPoint(x, y);
    for (let depth = 0; hit?.shadowRoot && depth < BROWSER_DEBUG.MAX_DEPTH; depth++) {
      const inner = hit.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    return hit;
  }
  function debugOwners() {
    const roots = [document], owners = [];
    for (let index = 0; index < roots.length && index < BROWSER_DEBUG.MAX_FRAMES; index++) scanOwners(roots[index], roots, owners);
    return owners;
  }
  function scanOwners(root, roots, owners) {
    for (const node of root.querySelectorAll("*")) {
      if (node.shadowRoot) roots.push(node.shadowRoot);
      if (node.matches("iframe,frame")) owners.push(node);
    }
  }
  function debugPoint(node) {
    for (const rect of node.getClientRects()) {
      const point = centre(rect);
      if (point && reaches(node, point)) return point;
    }
    throw Error("The element is covered or outside its viewport. No click was sent; use browser_hit_test.");
  }
  function centre(rect) {
    const left = Math.max(0, rect.left), top = Math.max(0, rect.top);
    const right = Math.min(innerWidth, rect.right), bottom = Math.min(innerHeight, rect.bottom);
    return right <= left || bottom <= top ? null : { x: (left + right) / BROWSER_DEBUG.CENTRE, y: (top + bottom) / BROWSER_DEBUG.CENTRE };
  }
  function reaches(node, point) {
    let hit = debugHit(point.x, point.y);
    for (let depth = 0; hit && depth < BROWSER_DEBUG.MAX_DEPTH; depth++) {
      if (hit === node || node.contains(hit)) return true;
      const root = hit.getRootNode();
      hit = root instanceof ShadowRoot ? root.host : null;
    }
    return false;
  }
  function requireAxisAligned(owner) {
    for (let node = owner; node; node = node.parentElement) requireTransform(node);
  }
  function requireTransform(node) {
    const transform = getComputedStyle(node).transform;
    if (transform === "none") return;
    const matrix = new DOMMatrix(transform);
    if (!matrix.is2D || matrix.b || matrix.c || matrix.a <= 0 || matrix.d <= 0)
      throw Error("Rotated or perspective frame geometry is unsupported");
  }
  function frameBox(owner) {
    const rect = owner.getBoundingClientRect(), style = getComputedStyle(owner);
    const sx = rect.width / owner.offsetWidth, sy = rect.height / owner.offsetHeight;
    const left = rect.left + (owner.clientLeft + parseFloat(style.paddingLeft)) * sx;
    const top = rect.top + (owner.clientTop + parseFloat(style.paddingTop)) * sy;
    if (![left, top, sx, sy].every(Number.isFinite) || !sx || !sy) throw Error("Frame geometry unavailable");
    return { sx, sy, left, top };
  }
  function debugProject(owner, point, inverse = false) {
    if (!owner.isConnected) throw Error("Frame owner was detached");
    requireAxisAligned(owner);
    const box = frameBox(owner);
    return inverse ? { x: (point.x - box.left) / box.sx, y: (point.y - box.top) / box.sy } : { x: box.left + point.x * box.sx, y: box.top + point.y * box.sy };
  }

  // browser/src/preload/debug-controls.ts
  var ROLES = /* @__PURE__ */ new Set([
    "button",
    "link",
    "textbox",
    "combobox",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "checkbox",
    "radio",
    "switch",
    "slider",
    "spinbutton",
    "searchbox",
    "gridcell",
    "treeitem"
  ]);
  function debugInteractive(node) {
    return node.matches(
      "a[href],button,input:not([type=hidden]),select,textarea,summary,[onclick],[oncontextmenu],[draggable=true]"
    ) || ROLES.has(node.getAttribute("role") ?? "") || node.hasAttribute("contenteditable") && node.isContentEditable;
  }
  function text(value) {
    return (value ?? "").replace(/\s+/g, " ").trim().slice(0, BROWSER_DEBUG.MAX_LABEL);
  }
  function ariaName(node) {
    const direct = text(node.getAttribute("aria-label"));
    if (direct) return direct;
    const root = node.getRootNode();
    return text(
      (node.getAttribute("aria-labelledby") ?? "").split(/\s+/).map((id) => root.getElementById?.(id)?.textContent ?? "").join(" ")
    );
  }
  function nativeName(node) {
    const field = node;
    return text(
      field.labels?.length ? [...field.labels].map((label) => label.textContent).join(" ") : node.closest("label")?.textContent
    );
  }
  function debugLabel(node) {
    const named = ariaName(node) || nativeName(node);
    if (named) return named;
    const button = node instanceof HTMLInputElement && ["submit", "reset", "button"].includes(node.type);
    const content = node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement ? button ? node.value : "" : node.innerText;
    return text(content || node.getAttribute("title") || node.getAttribute("placeholder") || node.getAttribute("name"));
  }
  function debugFieldFacts(node) {
    const facts = {};
    if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) {
      facts.value = node.type === "password" ? "\u2022\u2022\u2022\u2022" : text(node.value);
      if (node.placeholder) facts.placeholder = text(node.placeholder);
      if (node.readOnly) facts.readonly = true;
    }
    if (node instanceof HTMLInputElement && ["checkbox", "radio"].includes(node.type)) facts.checked = node.checked;
    if (node instanceof HTMLSelectElement) {
      facts.value = text(node.value);
      facts.options = [...node.options].slice(0, BROWSER_DEBUG.MAX_OPTIONS).map((option) => text(option.text));
      facts.optionsOmitted = Math.max(0, node.options.length - BROWSER_DEBUG.MAX_OPTIONS);
    }
    if ("disabled" in node && node.disabled || node.getAttribute("aria-disabled") === "true") facts.disabled = true;
    if ("required" in node && node.required) facts.required = true;
    if (node.hasAttribute("aria-checked")) facts.checked = node.getAttribute("aria-checked");
    return facts;
  }

  // browser/src/preload/debug-nodes.ts
  var DebugNodes = class {
    /** A new read invalidates the old nodes; references never retarget matching replacement markup. */
    nodes = /* @__PURE__ */ new Map();
    /** Never reuse a number within a document. */
    next = 0;
    /** Trusted receipt belongs to this exact selected document. */
    pointer = false;
    /** A new arm removes its preceding acknowledgement listener. */
    stopPointer;
    /** Require native trusted movement before any button press. */
    armPointer(id) {
      this.releasePointer();
      const node = this.get(id);
      this.stopPointer = observeDebugPointer(node, () => {
        this.pointer = true;
      });
    }
    /** Read isolated acknowledgement, never a page-owned status variable. */
    pointerReached() {
      return this.pointer;
    }
    /** Every terminal path retires the acknowledgement listener. */
    releasePointer() {
      this.stopPointer?.();
      this.stopPointer = void 0;
      this.pointer = false;
    }
    /** Read local content only; native main enumerates and labels descendant documents separately. */
    read() {
      this.releasePointer();
      this.nodes.clear();
      const rows = this.controls().map((node) => this.row(node));
      return {
        title: document.title.slice(0, BROWSER_DEBUG.MAX_LABEL),
        url: location.href,
        text: (document.body?.innerText ?? "").slice(0, BROWSER_DEBUG.MAX_TEXT),
        elements: rows
      };
    }
    /** Bounded author controls from light and open shadow roots. */
    controls() {
      const roots = [document], found = [];
      for (let index = 0; index < roots.length && index < BROWSER_DEBUG.MAX_FRAMES; index++) {
        scanDebugControls(roots[index], roots, found);
      }
      return found;
    }
    /** Private node identities retain baseline names and redact password field contents. */
    row(node) {
      const id = ++this.next;
      this.nodes.set(id, node);
      return {
        id,
        tag: node.localName,
        role: debugRole(node),
        name: debugLabel(node),
        facts: debugFieldFacts(node),
        href: node instanceof HTMLAnchorElement ? node.href : null,
        bounds: debugRect(node.getBoundingClientRect()),
        covered: covered(node)
      };
    }
    /** A disconnected node cannot be replaced by a selector with the same id. */
    get(id) {
      const node = this.nodes.get(id);
      if (!node?.isConnected) throw Error("Element is no longer valid. Read the page again before retrying.");
      return node;
    }
    /** Scroll only for an admitted action, then return a real reachable client-rect centre. */
    point(id, scroll) {
      const node = this.get(id);
      if (scroll) node.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
      return { ...debugPoint(node), bounds: debugRect(node.getBoundingClientRect()) };
    }
    /** Frame typing retains the existing explicit DOM value contract and reports its actual method. */
    type(id, text2, submit) {
      const node = this.get(id);
      if (node instanceof HTMLSelectElement) return choose(node, text2);
      if (node.isContentEditable) node.textContent = text2;
      else setInput(node, text2);
      node.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
      node.dispatchEvent(new Event("change", { bubbles: true }));
      if (submit) node.closest("form")?.requestSubmit();
    }
  };
  function shown(node) {
    const rect = node.getBoundingClientRect(), style = getComputedStyle(node);
    return rect.width > 0 && rect.height > 0 && style.visibility !== "hidden" && style.display !== "none";
  }
  function covered(node) {
    if (!hasDebugVisibleRect(node)) return false;
    try {
      debugPoint(node);
      return false;
    } catch {
      return true;
    }
  }
  function setInput(node, value) {
    if (!(node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement))
      throw Error("Element is not an editable field");
    if (node.disabled || node.readOnly) throw Error("Field is disabled or read-only");
    const prototype = node instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value").set.call(node, value);
  }
  function choose(node, value) {
    const match = [...node.options].find(
      (o) => o.value === value || o.text.trim().toLowerCase() === value.trim().toLowerCase()
    );
    if (!match || node.disabled || match.disabled) throw Error("No enabled option matches that value");
    node.value = match.value;
    node.dispatchEvent(new Event("input", { bubbles: true }));
    node.dispatchEvent(new Event("change", { bubbles: true }));
  }
  function debugElement(x, y) {
    if (![x, y].every(Number.isFinite) || x < 0 || y < 0 || x >= innerWidth || y >= innerHeight)
      throw Error("Point is outside the viewport");
    const node = debugHit(x, y);
    return node ? { node, data: debugElementData(node) } : null;
  }
  function debugElementData(node) {
    return {
      tag: node.localName,
      id: node.id || null,
      classes: [...node.classList],
      pointerEvents: getComputedStyle(node).pointerEvents,
      bounds: debugRect(node.getBoundingClientRect())
    };
  }
  function hasDebugVisibleRect(node) {
    return [...node.getClientRects()].some(
      (r) => r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight
    );
  }
  function scanDebugControls(root, roots, found) {
    for (const node of root.querySelectorAll("*")) {
      if (node.shadowRoot) roots.push(node.shadowRoot);
      if (found.length < BROWSER_DEBUG.MAX_ELEMENTS && debugInteractive(node) && shown(node)) found.push(node);
    }
  }
  function debugRole(node) {
    const declared = node.getAttribute("role");
    if (declared) return declared;
    if (node instanceof HTMLAnchorElement) return "link";
    if (node.localName === "summary") return "button";
    if (node.hasAttribute("contenteditable") && node.isContentEditable) return "textbox";
    if (node instanceof HTMLSelectElement) return "combobox";
    if (node instanceof HTMLTextAreaElement) return "textbox";
    if (node instanceof HTMLInputElement)
      return ["checkbox", "radio", "button", "submit"].includes(node.type) ? node.type : "textbox";
    return node.localName;
  }
  function observeDebugPointer(node, reached) {
    const listener = (event) => {
      if (event.isTrusted && event.composedPath().some((target) => target instanceof Node && (target === node || node.contains(target))))
        reached();
    };
    const stop = () => {
      document.removeEventListener("mousemove", listener, true);
      clearTimeout(timer);
    };
    const timer = setTimeout(stop, BROWSER_DEBUG.OPERATION_MS);
    document.addEventListener("mousemove", listener, true);
    return stop;
  }

  // browser/src/preload/debug-owners.ts
  var DebugOwners = class {
    /** Exact nodes, never URL/name selectors. */
    nodes = /* @__PURE__ */ new Map();
    /** A child challenge is consumed synchronously before page listeners can replay it. */
    pending = /* @__PURE__ */ new Map();
    /** One-use results await native collection. */
    results = /* @__PURE__ */ new Map();
    /** Monotonic document-local owner identities. */
    next = 0;
    /** Resolved owner observations are consumed without waiting on unrelated candidate parents. */
    ready = /* @__PURE__ */ new Map();
    /** Install before author scripts; late installation is never used as a fallback. */
    constructor() {
      addEventListener("message", (event) => this.receive(event), { capture: true });
    }
    /** Native main arms a single-use challenge before instructing the exact child to send it. */
    arm(nonce) {
      if (!nonce || this.results.size >= BROWSER_DEBUG.MAX_PENDING || this.results.has(nonce))
        throw Error("Frame challenge capacity exceeded");
      const result = new Promise((resolve, reject) => this.start(nonce, resolve, reject));
      this.results.set(nonce, result);
      void result.catch(() => {
      });
    }
    /** Expiration cleans abandoned requests even when native collection never arrives. */
    start(nonce, resolve, reject) {
      const timer = setTimeout(() => this.cancel(nonce), BROWSER_DEBUG.OWNER_MS);
      this.pending.set(nonce, { resolve, reject, timer });
    }
    /** Separate collection lets native main await arming before the child is signalled. */
    async collect(nonce) {
      const result = this.results.get(nonce);
      if (!result) throw Error("Frame challenge is missing or already consumed");
      try {
        return await result;
      } finally {
        this.results.delete(nonce);
        this.ready.delete(nonce);
      }
    }
    /** Only trusted native postMessage delivery from the real child window can consume a challenge. */
    receive(event) {
      if (!event.isTrusted || event.data?.type !== BROWSER_DEBUG.MESSAGE || typeof event.data.nonce !== "string") return;
      const challenge = this.pending.get(event.data.nonce);
      if (!challenge) return;
      this.pending.delete(event.data.nonce);
      const owners = debugOwners().filter((node) => node.contentWindow === event.source);
      if (owners.length !== 1) return challenge.reject(Error("Frame owner is missing or ambiguous"));
      event.stopImmediatePropagation();
      const owner = this.remember(owners[0]);
      this.ready.set(event.data.nonce, owner);
      challenge.resolve(owner);
    }
    /** Nonblocking native discovery never waits on every unrelated candidate parent. */
    peek(nonce) {
      return this.ready.get(nonce) ?? null;
    }
    /** Retire unused candidate challenges after exact owner discovery or navigation. */
    cancel(nonce) {
      const pending = this.pending.get(nonce);
      if (pending) {
        clearTimeout(pending.timer);
        pending.reject(Error("Frame challenge retired"));
      }
      this.pending.delete(nonce);
      this.results.delete(nonce);
      this.ready.delete(nonce);
    }
    /** An owner reference stays attached to this exact connected DOM node. */
    remember(node) {
      for (const [id2, known] of this.nodes) {
        if (!known.isConnected) this.nodes.delete(id2);
        else if (known === node) return id2;
      }
      if (this.nodes.size >= BROWSER_DEBUG.MAX_FRAMES) throw Error("Frame owner limit exceeded");
      const id = `o-${++this.next}`;
      this.nodes.set(id, node);
      return id;
    }
    /** No selector or origin fallback can retarget an old owner reference. */
    get(id) {
      const node = this.nodes.get(id);
      if (!node?.isConnected) throw Error("Frame owner changed or closed");
      return node;
    }
  };

  // browser/src/preload/debug-runtime.ts
  var randomValues = crypto.getRandomValues.bind(crypto);
  function generation() {
    return Array.from(randomValues(new Uint32Array(BROWSER_DEBUG.TOKEN_WORDS))).join("-");
  }
  var DebugDocument = class {
    /** Changes on trusted navigation, even when BFCache restores the same realm. */
    document = generation();
    /** Node capabilities never leave the isolated world. */
    nodes = new DebugNodes();
    /** The earliest capture listener owns parent/child association. */
    owners = new DebugOwners();
    /** Fixed operation map cannot expose arbitrary preload execution. */
    operations;
    /** Install during native document-start initialization, never as a late page-script fallback. */
    /** Browser-owned evaluator. */
    evaluate;
    /** Registration transport. */
    registration;
    constructor(evaluate, registration) {
      this.evaluate = evaluate;
      this.registration = registration;
      this.operations = this.operationMap();
      addEventListener("pagehide", (event) => this.retire(event), { capture: true });
      addEventListener(
        "pageshow",
        (event) => {
          if (event.isTrusted) this.register();
        },
        { capture: true }
      );
    }
    /** A registration contains no page-controlled native frame identity. */
    register() {
      this.registration({ kind: "register", document: this.document });
    }
    /** A stale document reference stays stale after navigation and BFCache restoration. */
    retire(event) {
      if (!event.isTrusted) return;
      this.registration({ kind: "retire", document: this.document });
      this.document = generation();
      this.nodes.releasePointer();
    }
    /** Requests validate their document before evaluating anything, within the same renderer turn. */
    async execute(request) {
      try {
        this.admit(request);
        return boundedDebugResponse({
          id: request.id,
          document: this.document,
          ok: true,
          data: await this.operations[request.kind](request.params ?? {})
        });
      } catch (why) {
        return {
          id: request.id,
          document: this.document,
          ok: false,
          error: String(why?.message ?? why).slice(0, BROWSER_DEBUG.MAX_ERROR)
        };
      }
    }
    /** Only initial metadata can omit a document capability. */
    admit(request) {
      if (request.document !== this.document && (request.document !== void 0 || request.kind !== "metadata"))
        throw Error("Frame changed or closed. Read the page again with frames:true.");
      if (!Object.hasOwn(this.operations, request.kind)) throw Error("Unsupported diagnostic operation");
    }
    /** The native boundary invokes named trusted handlers, never a caller-selected function. */
    operationMap() {
      return { ...this.documentOperations(), ...this.nodeOperations(), ...this.ownerOperations() };
    }
    /** Metadata is measured in the exact isolated document. */
    documentOperations() {
      return {
        metadata: () => ({
          document: this.document,
          url: location.href.length > BROWSER_DEBUG.MAX_URL ? location.href.slice(0, BROWSER_DEBUG.MAX_URL) + "\u2026 [URL shortened]" : location.href,
          width: innerWidth,
          height: innerHeight,
          dpr: devicePixelRatio
        }),
        read: () => this.nodes.read(),
        eval: (p) => this.evaluate(p.expression)
      };
    }
    /** Native main alone supplies node capability numbers issued by a read. */
    nodeOperations() {
      return {
        point: (p) => this.nodes.point(Number(p.element), p.scroll === true),
        type: (p) => this.nodes.type(Number(p.element), String(p.text ?? ""), p.submit === true),
        arm_pointer: (p) => this.nodes.armPointer(Number(p.element)),
        pointer_reached: () => this.nodes.pointerReached(),
        release_pointer: () => this.nodes.releasePointer()
      };
    }
    /** A one-use challenge associates actual WindowProxy sources with parent-private owner nodes. */
    ownerOperations() {
      return {
        owner_peek: (p) => this.owners.peek(String(p.nonce)),
        cancel_owner: (p) => this.owners.cancel(String(p.nonce)),
        arm: (p) => this.owners.arm(String(p.nonce)),
        owner_result: (p) => this.owners.collect(String(p.nonce)),
        challenge: (p) => parent.postMessage({ type: BROWSER_DEBUG.MESSAGE, nonce: p.nonce }, "*"),
        hit: (p) => this.hit(Number(p.x), Number(p.y)),
        project: (p) => this.project(p)
      };
    }
    /** Only actual browser hits can produce a private frame-owner reference. */
    hit(x, y) {
      const hit = debugElement(x, y);
      if (!hit) return { element: null };
      return {
        element: hit.data,
        owner: hit.node.matches("iframe,frame") ? this.owners.remember(hit.node) : null
      };
    }
    /** Projection checks actual interception unless the caller asks only for geometry. */
    project(p) {
      const owner = this.owners.get(String(p.owner));
      if (p.scroll === true) owner.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
      const point = debugProject(owner, { x: Number(p.x), y: Number(p.y) }, p.inverse === true);
      const hit = p.inverse === true ? debugHit(Number(p.x), Number(p.y)) : debugHit(point.x, point.y);
      if (p.geometry !== true && hit !== owner)
        throw Error("Frame is covered at the requested point. No click was sent.");
      return point;
    }
  };
  function boundedDebugResponse(response) {
    if (new TextEncoder().encode(JSON.stringify(response)).length > BROWSER_DEBUG.MAX_REPLY_BYTES)
      return {
        id: response.id,
        document: response.document,
        ok: false,
        error: "Frame diagnostic result exceeded 256 KiB."
      };
    return response;
  }

  // browser/src/preload/debug-webkit.ts
  var runtime = new DebugDocument(
    () => {
      throw Error("Page-world eval must use the native WK adapter");
    },
    (event) => window.webkit.messageHandlers.keel_debug.postMessage(JSON.stringify(event))
  );
  Object.defineProperty(globalThis, "__keelDebug", { value: runtime, configurable: false, writable: false });
  runtime.register();
})();

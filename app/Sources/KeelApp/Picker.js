// Injected into every frame of the preview, including the dev server's own — a WKUserScript with
// forMainFrameOnly:false crosses the origin boundary that an iframe script cannot. That is the
// whole reason design mode is cheaper native than it was in a browser shell.
//
// Three jobs, one script: pick an element and describe it; watch the DOM after the agent writes
// a file and report what moved, which is both "HMR has landed" and "here is where"; and draw the
// pins someone has left on the page. Everything it draws carries data-keel, so it never reports
// its own overlay as a change.
//
// Inert until asked. Nothing observes and nothing is drawn until a message says so.
(function () {
  if (window.__keelPicker) return;
  window.__keelPicker = true;

  var on = false, host = null, hover = null, armedUntil = 0;

  // Where this frame sits inside the top-level view, in top-level CSS pixels.
  //
  // Everything Keel photographs is a rect of the *web view*, and `getBoundingClientRect` inside an
  // iframe is relative to that iframe. Without this, a pick inside the dev server's frame was
  // photographed at the wrong place on the page and the verdict was about somewhere else — the
  // exact case `forMainFrameOnly:false` exists to serve. A frame cannot read its own position
  // cross-origin, so the parent tells it: each frame announces `origin` to its children, adding
  // its own, and the composition walks to any depth.
  var frameOrigin = { x: 0, y: 0 };

  function kids() { return document.querySelectorAll('iframe,frame'); }

  function announceFrames() {
    var f = kids();
    for (var i = 0; i < f.length; i++) {
      var r = f[i].getBoundingClientRect();
      try {
        f[i].contentWindow.postMessage({
          keel: 'origin',
          x: frameOrigin.x + r.left + (f[i].clientLeft || 0),
          y: frameOrigin.y + r.top + (f[i].clientTop || 0)
        }, '*');
      } catch (e) {}
    }
  }

  // Keel posts to the top frame only; every frame passes what it got to its own children, so a
  // frame nested two deep is reached. The old fan-out walked `window.frames` one level and stopped.
  function relay(m) {
    var f = kids();
    for (var i = 0; i < f.length; i++) {
      try { f[i].contentWindow.postMessage(m, '*'); } catch (e) {}
    }
  }

  // ── overlay ────────────────────────────────────────────────────────────────

  function layer() {
    if (host) return host;
    host = document.createElement('div');
    host.setAttribute('data-keel', '');
    host.style.cssText = 'position:fixed;inset:0;z-index:2147483647;pointer-events:none;';
    var style = document.createElement('style');
    style.setAttribute('data-keel', '');
    style.textContent =
      '[data-keel-box]{position:fixed;pointer-events:none;border-radius:2px;box-sizing:border-box}' +
      '[data-keel-hover]{border:2px solid #2563eb;background:rgba(37,99,235,.12);transition:all .05s linear}' +
      '[data-keel-outline]{border:2px solid #2563eb;animation:keel-pulse 1.1s ease-in-out infinite}' +
      '[data-keel-ripple]{border:2px solid #2563eb;animation:keel-ripple 1.2s ease-out forwards}' +
      '[data-keel-dot]{position:fixed;width:18px;height:18px;margin:-9px 0 0 -9px;border-radius:50%;' +
        'background:#2563eb;color:#fff;font:600 10px/18px -apple-system,sans-serif;text-align:center;' +
        'pointer-events:auto;cursor:pointer;box-shadow:0 1px 4px rgba(0,0,0,.35)}' +
      '[data-keel-dot][data-keel-new]{animation:keel-beat 1.4s ease-in-out infinite}' +
      '[data-keel-dot][data-keel-pin]{background:#111;border:2px solid #2563eb;line-height:14px}' +
      '[data-keel-label]{position:fixed;pointer-events:none;background:#111;color:#fff;' +
        'font:500 10px/16px ui-monospace,SFMono-Regular,Menlo,monospace;padding:0 5px;' +
        'border-radius:3px;white-space:nowrap;max-width:236px;overflow:hidden;text-overflow:ellipsis;' +
        'box-shadow:0 1px 3px rgba(0,0,0,.3)}' +
      '[data-keel-gap]{position:fixed;pointer-events:none}' +
      '[data-keel-grip]{position:fixed;width:9px;height:9px;margin:-5px 0 0 -5px;' +
        'background:#fff;border:1.5px solid #2563eb;border-radius:2px;pointer-events:auto;' +
        'box-shadow:0 1px 2px rgba(0,0,0,.25)}' +
      '@keyframes keel-pulse{0%,100%{box-shadow:0 0 0 0 rgba(37,99,235,.45)}50%{box-shadow:0 0 0 6px rgba(37,99,235,0)}}' +
      '@keyframes keel-ripple{0%{box-shadow:0 0 0 0 rgba(37,99,235,.6);background:rgba(37,99,235,.22)}' +
        '100%{box-shadow:0 0 0 14px rgba(37,99,235,0);background:rgba(37,99,235,0)}}' +
      '@keyframes keel-beat{0%,100%{transform:scale(1)}50%{transform:scale(1.18)}}';
    host.appendChild(style);
    document.documentElement.appendChild(host);
    return host;
  }

  function place(box, el) {
    var r = el.getBoundingClientRect();
    box.style.top = r.top + 'px'; box.style.left = r.left + 'px';
    box.style.width = r.width + 'px'; box.style.height = r.height + 'px';
    box.style.display = (r.width || r.height) ? 'block' : 'none';
  }

  function mkBox(kind) {
    var b = document.createElement('div');
    b.setAttribute('data-keel', ''); b.setAttribute('data-keel-box', ''); b.setAttribute(kind, '');
    layer().appendChild(b);
    return b;
  }

  function find(selector) {
    try { return document.querySelector(selector); } catch (e) { return null; }
  }

  // Everything drawn is re-anchored on scroll, resize and after each DOM settle, so a pin left on
  // a button is still on the button after the page hot-reloads under it.
  var rafPending = false;
  function reposition() {
    if (rafPending) return;
    rafPending = true;
    requestAnimationFrame(function () {
      rafPending = false;
      if (!host) { announceFrames(); return; }
      announceFrames();
      var boxes = host.querySelectorAll('[data-keel-for]');
      for (var i = 0; i < boxes.length; i++) {
        var el = find(boxes[i].getAttribute('data-keel-for'));
        if (!el) { boxes[i].style.display = 'none'; continue; }
        if (boxes[i].hasAttribute('data-keel-dot')) {
          var r = el.getBoundingClientRect();
          boxes[i].style.display = (r.width || r.height) ? 'block' : 'none';
          boxes[i].style.top = (r.top + 2) + 'px'; boxes[i].style.left = (r.right - 2) + 'px';
        } else {
          place(boxes[i], el);
        }
      }
    });
  }
  window.addEventListener('scroll', reposition, true);
  window.addEventListener('resize', reposition);

  // ── selectors, style, hints (as before) ──────────────────────────────────

  function unique(sel, el) {
    try {
      var all = document.querySelectorAll(sel);
      return all.length === 1 && all[0] === el;
    } catch (e) { return false; }
  }

  // Tailwind's classes are the ones that discriminate — `md:flex`, `w-1/2`, `p-[3px]` — and they
  // were all being thrown away for containing a `:` or a `[`. `CSS.escape` makes them legal in a
  // selector. What is dropped instead is the noise that changes on every build: framework hashes.
  function classPart(el) {
    var cls = (typeof el.className === 'string' ? el.className : '').trim();
    if (!cls) return '';
    return cls.split(/\s+/).filter(function (c) {
      return c && c.length < 40 && !/^(ng-|svelte-[a-z0-9]{5,}|jsx-\d|css-[a-z0-9]{5,})/.test(c);
    }).slice(0, 3).map(function (c) { return '.' + CSS.escape(c); }).join('');
  }

  // Widen until it means exactly one element.
  //
  // A selector that matches several nodes re-photographs whichever comes first, and that is how a
  // verdict ends up being about the wrong button. So each ancestor is added only while the answer
  // is still ambiguous, and the result says whether it ever became unambiguous.
  function selectorFor(el) {
    var parts = [], node = el;
    while (node && node.nodeType === 1 && node !== document.documentElement && parts.length < 8) {
      var part;
      if (node.id) {
        part = '#' + CSS.escape(node.id);
        parts.unshift(part);
        if (unique(parts.join(' > '), el)) return parts.join(' > ');
        break;
      }
      part = node.tagName.toLowerCase() + classPart(node);
      var parent = node.parentElement;
      if (parent) {
        var same = Array.prototype.filter.call(parent.children, function (c) {
          return c.tagName === node.tagName;
        });
        if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
      }
      parts.unshift(part);
      if (unique(parts.join(' > '), el)) return parts.join(' > ');
      node = parent;
    }
    return parts.join(' > ');
  }

  var WANTED = ['display', 'position', 'width', 'height', 'margin', 'padding', 'color',
    'background-color', 'border', 'border-radius', 'font-family', 'font-size', 'font-weight',
    'line-height', 'letter-spacing', 'text-align', 'flex-direction', 'justify-content',
    'align-items', 'gap', 'grid-template-columns', 'opacity', 'box-shadow', 'z-index'];

  function styleOf(el) {
    var cs = getComputedStyle(el), out = {};
    WANTED.forEach(function (p) {
      var v = cs.getPropertyValue(p);
      if (v && v !== 'none' && v !== 'normal' && v !== 'auto' && v !== '0px') out[p] = v;
    });
    return out;
  }

  function sourceHints(el) {
    var hints = [];
    // A build-time attribute is the only hint that is a file and a line without guessing.
    // `data-testid` used to be in this list and is not a source path — it is a test selector, and
    // offering it as "the file" is exactly the confident wrong guess this feature exists to catch.
    for (var node = el; node && node.nodeType === 1; node = node.parentElement) {
      var rel = node.getAttribute && (node.getAttribute('data-inspector-relative-path')
        || node.getAttribute('data-source-file'));
      if (rel) {
        var line = node.getAttribute('data-inspector-line');
        hints.push({ kind: 'attribute', value: line ? rel + ':' + line : rel });
        break;
      }
    }

    // Svelte publishes file and line on the element itself in dev — free, exact, and never read
    // until now, so a Svelte app got an empty ranking and the agent guessed.
    for (var n = el; n && n.nodeType === 1 && hints.length < 3; n = n.parentElement) {
      var meta = n.__svelte_meta;
      if (meta && meta.loc && meta.loc.file) {
        hints.push({ kind: 'svelte', value: meta.loc.file + ':' + (meta.loc.line || 1) });
        break;
      }
    }

    // Vue names the component that owns the node, and often its file with it.
    for (var v = el; v && v.nodeType === 1 && hints.length < 3; v = v.parentElement) {
      var vc = v.__vueParentComponent;
      if (vc && vc.type) {
        var file = vc.type.__file;
        var vname = vc.type.name || vc.type.__name;
        if (file) hints.push({ kind: 'vue', value: file });
        else if (vname) hints.push({ kind: 'component', value: vname });
        break;
      }
    }

    for (var key in el) {
      if (key.indexOf('__reactFiber$') !== 0 && key.indexOf('__reactInternalInstance$') !== 0) continue;
      var fiber = el[key];
      for (var i = 0; fiber && i < 12; i++) {
        if (fiber._debugSource && fiber._debugSource.fileName) {
          hints.push({ kind: 'fiber',
            value: fiber._debugSource.fileName + ':' + (fiber._debugSource.lineNumber || 1) });
          break;
        }
        var t = fiber.type;
        var name = typeof t === 'function' ? (t.displayName || t.name)
          : (t && (t.displayName || (t.render && (t.render.displayName || t.render.name))));
        if (name && name[0] === name[0].toUpperCase()) { hints.push({ kind: 'component', value: name }); break; }
        fiber = fiber.return;
      }
      break;
    }

    // A test id is worth naming — it is how the agent will find the element — but as what it is,
    // and last.
    var tid = el.closest && el.closest('[data-testid]');
    if (tid) hints.push({ kind: 'test id', value: tid.getAttribute('data-testid') });
    return hints;
  }

  function rectOf(el) {
    var r = el.getBoundingClientRect();
    return { x: r.left + frameOrigin.x, y: r.top + frameOrigin.y,
             width: r.width, height: r.height };
  }

  // What you are about to select, in words. An unlabelled blue rectangle tells you where the
  // element is and nothing about which one it is — and picking the wrong node is the failure that
  // every other part of this feature then faithfully verifies.
  function brief(el) {
    var r = el.getBoundingClientRect(), out = el.tagName.toLowerCase();
    if (el.id) out += '#' + el.id;
    out += '  ' + Math.round(r.width) + '×' + Math.round(r.height);
    var h = sourceHints(el)[0];
    if (h) out += '  ' + h.value.split('/').pop();
    return out;
  }

  function post(msg) {
    try { window.webkit.messageHandlers.keel.postMessage(msg); } catch (e) {}
  }

  /// An open shadow root retargets the event to the host, so a click on a button inside a custom
  /// element picks the custom element — and `document.querySelector` can never re-find the button.
  /// Saying so beats silently describing the wrong node.
  function inShadow(el) {
    return !!(el.shadowRoot || (el.getRootNode && el.getRootNode() !== document));
  }

  function payload(el) {
    var sel = selectorFor(el);
    return {
      shadow: inShadow(el),
      selector: sel,
      unique: unique(sel, el),
      tag: el.tagName.toLowerCase(),
      text: (el.innerText || '').trim().slice(0, 200),
      html: el.outerHTML.slice(0, 2000),
      style: styleOf(el),
      hints: sourceHints(el),
      rect: rectOf(el)
    };
  }

  function sendPick(el) {
    var p = payload(el);
    p.type = 'pick';
    post(p);
  }

  // ── nudges: the change you made by hand, as a sentence ────────────────────
  //
  // Keel does not write files — the editor was deleted on purpose. So dragging a handle is not an
  // edit, it is a way of *saying* what you want in the units the source uses: `width 240px →
  // 320px` reads back to the agent exactly as you meant it, and the pixel check afterwards proves
  // the source now matches. What you did by hand is a preview and is undone when the turn starts,
  // so what you are looking at afterwards is the agent's change and not your ghost of it.

  var touched = [];

  function remember(el) {
    for (var i = 0; i < touched.length; i++) if (touched[i].el === el) return;
    touched.push({ el: el, cssText: el.style.cssText, html: el.innerHTML });
  }

  function revertAll() {
    touched.forEach(function (t) {
      t.el.style.cssText = t.cssText;
      // Only when the text was actually edited: writing `innerHTML` back into a node a framework
      // owns tears out its listeners, and a resize never touched it.
      if (t.el.innerHTML !== t.html) t.el.innerHTML = t.html;
    });
    touched = [];
    reposition();
  }

  function sendNudge(el, label) {
    post({ type: 'nudge', label: label, pick: payload(el) });
  }

  function px(n) { return Math.round(n) + 'px'; }

  // ── pins and regions ──────────────────────────────────────────────────────

  function dot(selector, label, isPin) {
    var d = document.createElement('div');
    d.setAttribute('data-keel', ''); d.setAttribute('data-keel-dot', '');
    d.setAttribute('data-keel-for', selector);
    if (isPin) d.setAttribute('data-keel-pin', ''); else d.setAttribute('data-keel-new', '');
    d.textContent = label;
    d.title = isPin ? 'Pinned' : 'Changed — click to pin a note here';
    d.addEventListener('click', function (e) {
      e.preventDefault(); e.stopPropagation();
      var el = find(selector);
      if (el) sendPick(el);
    }, true);
    layer().appendChild(d);
    return d;
  }

  function drawPins(pins) {
    pinned = (pins || []).map(function (p) { return p.selector; });
    var old = host ? host.querySelectorAll('[data-keel-pin]') : [];
    for (var i = 0; i < old.length; i++) old[i].remove();
    (pins || []).forEach(function (p, i) { dot(p.selector, String(i + 1), true); });
    reposition();
  }

  function outline(selector) {
    var old = host ? host.querySelectorAll('[data-keel-outline]') : [];
    for (var i = 0; i < old.length; i++) old[i].remove();
    var el = selector && find(selector);
    if (!el) return;
    var b = mkBox('data-keel-outline');
    b.setAttribute('data-keel-for', selector);
    place(b, el);
  }

  function ripple(el, n) {
    var b = mkBox('data-keel-ripple');
    place(b, el);
    setTimeout(function () { b.remove(); }, 1300);
    var d = dot(selectorFor(el), String(n), false);
    reposition();
    return d;
  }

  function clearRegions() {
    var old = host ? host.querySelectorAll('[data-keel-new],[data-keel-ripple],[data-keel-outline]') : [];
    for (var i = 0; i < old.length; i++) old[i].remove();
  }

  // ── the observer: what the agent's write did to the page ─────────────────

  var pending = new Set(), settle = null;

  function ours(node) {
    for (var n = node; n; n = n.parentNode) {
      if (n.nodeType === 1 && n.hasAttribute && n.hasAttribute('data-keel')) return true;
    }
    return false;
  }

  function ignorable(node) {
    var el = node.nodeType === 1 ? node : node.parentElement;
    if (!el) return true;
    if (ours(el)) return true;
    var tag = el.tagName;
    if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'LINK' || tag === 'HEAD' || tag === 'HTML') return true;
    if (el.closest && el.closest('head')) return true;
    // HMR runtimes mount their own error overlays and status badges.
    if (el.id && /^(__next|vite|webpack|nextjs|__nuxt)/i.test(el.id)) return true;
    if (el.tagName && /-(overlay|portal|toast)$/i.test(el.tagName)) return true;
    return false;
  }

  var observer = new MutationObserver(function (records) {
    if (Date.now() > armedUntil) return;
    records.forEach(function (rec) {
      var target = rec.type === 'childList' ? rec.target : rec.target;
      if (rec.type === 'childList') {
        rec.addedNodes.forEach(function (n) { var e = n.nodeType === 1 ? n : n.parentElement; if (e && !ignorable(e)) pending.add(e); });
        rec.removedNodes.forEach(function () { if (target.nodeType === 1 && !ignorable(target)) pending.add(target); });
      } else {
        var el = target.nodeType === 1 ? target : target.parentElement;
        if (el && !ignorable(el)) pending.add(el);
      }
    });
    if (settle) clearTimeout(settle);
    settle = setTimeout(report, 250);
  });

  // Coalesce to the outermost changed ancestors: a re-rendered list is one region, not forty.
  function report() {
    settle = null;
    var els = Array.from(pending).filter(function (e) { return e.isConnected && e !== document.body; });
    pending.clear();
    if (!els.length) return;
    var outer = els.filter(function (e) {
      return !els.some(function (o) { return o !== e && o.contains(e); });
    });
    // A change that swallowed the whole page is not a region anyone can point at.
    outer = outer.filter(function (e) {
      var r = e.getBoundingClientRect();
      return r.width > 0 && r.height > 0 && !(r.width >= innerWidth - 2 && r.height >= innerHeight - 2);
    }).slice(0, 12);
    if (!outer.length) return;
    clearRegions();
    var regions = outer.map(function (el, i) {
      ripple(el, i + 1);
      return { selector: selectorFor(el), rect: rectOf(el), tag: el.tagName.toLowerCase(),
               text: (el.innerText || '').trim().slice(0, 80) };
    });
    post({ type: 'changed', regions: regions });
  }

  function arm(ms) {
    armedUntil = Date.now() + (ms || 20000);
    if (!observer._on) {
      observer.observe(document.documentElement,
        { childList: true, attributes: true, characterData: true, subtree: true });
      observer._on = true;
    }
  }

  // Where these elements are *now*.
  //
  // The check re-photographs the pin at the end of the turn, and a rect captured when you clicked
  // is a rect of the viewport, not of the element: scroll between the two and the after-shot is of
  // whatever moved into that space. So the rect is asked for again immediately before the shot.
  // A selector this frame cannot resolve is simply absent — some other frame may have it, and one
  // that nobody has means the element is gone, which is itself the answer.
  function answerRects(m) {
    var found = {}, any = false;
    (m.selectors || []).forEach(function (sel) {
      var el = find(sel);
      if (!el) return;
      found[sel] = rectOf(el);
      any = true;
    });
    if (any) post({ type: 'rects', id: m.id, found: found });
  }

  // ── messages from Keel ────────────────────────────────────────────────────

  window.addEventListener('message', function (e) {
    var m = e.data;
    if (!m || typeof m !== 'object' || !m.keel) return;
    if (m.keel === 'origin') {
      frameOrigin = { x: m.x || 0, y: m.y || 0 };
      announceFrames();
      return;
    }
    relay(m);
    switch (m.keel) {
      case 'pick-on': on = true; announceFrames(); break;
      case 'pick-off': on = false; blur(); break;
      case 'expect': arm(m.ms); break;
      case 'outline': outline(m.selector); break;
      case 'pins': drawPins(m.pins); break;
      case 'clear': clearRegions(); outline(null); drawPins([]); break;
      case 'rects': answerRects(m); break;
      case 'revert': revertAll(); break;
    }
  });

  announceFrames();
  window.addEventListener('load', announceFrames);
  setTimeout(announceFrames, 1000);
  setTimeout(announceFrames, 3000);

  // ── what the hover box is on, and how to move it ─────────────────────────

  var current = null, label = null, measured = [], pinned = [];

  function focusOn(el) {
    if (!el || el.nodeType !== 1) return;
    current = el;
    if (!hover) hover = mkBox('data-keel-hover');
    hover.style.display = 'block';
    place(hover, el);
    if (!label) {
      label = document.createElement('div');
      label.setAttribute('data-keel', ''); label.setAttribute('data-keel-label', '');
      layer().appendChild(label);
    }
    label.textContent = brief(el);
    var r = el.getBoundingClientRect();
    label.style.display = 'block';
    // Above the element, unless there is no room up there.
    label.style.top = (r.top > 20 ? r.top - 19 : Math.min(r.bottom + 3, innerHeight - 18)) + 'px';
    label.style.left = Math.max(2, Math.min(r.left, innerWidth - 240)) + 'px';
    placeHandles(el);
  }

  function blur() {
    current = null;
    if (hover) hover.style.display = 'none';
    if (label) label.style.display = 'none';
    hideHandles();
    clearMeasure();
  }

  // ── handles ───────────────────────────────────────────────────────────────

  var handles = null, drag = null, justDragged = false, editingEl = null;

  var GRIPS = [
    { at: 'e', cursor: 'ew-resize' },
    { at: 's', cursor: 'ns-resize' },
    { at: 'se', cursor: 'nwse-resize' }
  ];

  function makeHandles() {
    if (handles) return handles;
    handles = GRIPS.map(function (g) {
      var h = document.createElement('div');
      h.setAttribute('data-keel', ''); h.setAttribute('data-keel-grip', g.at);
      h.style.cursor = g.cursor;
      h.addEventListener('mousedown', function (e) { startResize(e, g.at); }, true);
      layer().appendChild(h);
      return h;
    });
    return handles;
  }

  function placeHandles(el) {
    var hs = makeHandles(), r = el.getBoundingClientRect();
    var at = [[r.right, r.top + r.height / 2], [r.left + r.width / 2, r.bottom], [r.right, r.bottom]];
    for (var i = 0; i < hs.length; i++) {
      hs[i].style.display = (r.width > 12 && r.height > 12) ? 'block' : 'none';
      hs[i].style.left = at[i][0] + 'px';
      hs[i].style.top = at[i][1] + 'px';
    }
  }

  function hideHandles() {
    if (!handles) return;
    for (var i = 0; i < handles.length; i++) handles[i].style.display = 'none';
  }

  function startResize(e, at) {
    if (!current) return;
    e.preventDefault(); e.stopPropagation();
    var r = current.getBoundingClientRect();
    remember(current);
    drag = { kind: 'size', el: current, at: at, x: e.clientX, y: e.clientY,
             w: r.width, h: r.height };
  }

  function startMove(e) {
    if (!current) return;
    e.preventDefault(); e.stopPropagation();
    remember(current);
    drag = { kind: 'move', el: current, x: e.clientX, y: e.clientY, dx: 0, dy: 0 };
  }

  function onDrag(e) {
    if (!drag) return;
    e.preventDefault();
    var dx = e.clientX - drag.x, dy = e.clientY - drag.y;
    if (drag.kind === 'size') {
      if (drag.at.indexOf('e') >= 0) drag.el.style.width = px(Math.max(4, drag.w + dx));
      if (drag.at.indexOf('s') >= 0) drag.el.style.height = px(Math.max(4, drag.h + dy));
    } else {
      drag.dx = dx; drag.dy = dy;
      drag.el.style.transform = 'translate(' + px(dx) + ',' + px(dy) + ')';
    }
    placeHandles(drag.el);
    if (hover) place(hover, drag.el);
    reposition();
  }

  function endDrag() {
    if (!drag) return;
    var d = drag;
    drag = null;
    justDragged = true;
    var r = d.el.getBoundingClientRect();
    if (d.kind === 'size') {
      var bits = [];
      if (Math.round(r.width) !== Math.round(d.w)) bits.push('width ' + px(d.w) + ' → ' + px(r.width));
      if (Math.round(r.height) !== Math.round(d.h)) bits.push('height ' + px(d.h) + ' → ' + px(r.height));
      if (bits.length) sendNudge(d.el, bits.join(', '));
    } else if (Math.round(d.dx) || Math.round(d.dy)) {
      sendNudge(d.el, 'moved ' + px(d.dx) + ' across and ' + px(d.dy) + ' down from where it sits '
        + 'now — change the layout so it lands there, do not add a transform');
    }
  }

  // ── editing text in place ─────────────────────────────────────────────────

  function editText(el) {
    if (editingEl) return;
    remember(el);
    editingEl = el;
    var was = (el.innerText || '').trim();
    el.setAttribute('contenteditable', 'plaintext-only');
    el.focus();
    try {
      var range = document.createRange();
      range.selectNodeContents(el);
      var sel = window.getSelection();
      sel.removeAllRanges(); sel.addRange(range);
    } catch (err) {}
    function done() {
      el.removeEventListener('blur', done, true);
      el.removeEventListener('keydown', keys, true);
      el.removeAttribute('contenteditable');
      editingEl = null;
      var now = (el.innerText || '').trim();
      if (now !== was) sendNudge(el, 'text "' + was.slice(0, 80) + '" → "' + now.slice(0, 80) + '"');
    }
    function keys(ev) {
      ev.stopPropagation();
      if (ev.key === 'Enter' && !ev.shiftKey) { ev.preventDefault(); el.blur(); }
      if (ev.key === 'Escape') { ev.preventDefault(); el.innerText = was; el.blur(); }
    }
    el.addEventListener('blur', done, true);
    el.addEventListener('keydown', keys, true);
  }

  function skipOurs(el) { return el && !ours(el) ? el : null; }

  function firstChild(el) {
    for (var c = el.firstElementChild; c; c = c.nextElementSibling) {
      if (!ours(c)) return c;
    }
    return null;
  }

  function sibling(el, forward) {
    for (var c = forward ? el.nextElementSibling : el.previousElementSibling; c;
         c = forward ? c.nextElementSibling : c.previousElementSibling) {
      if (!ours(c)) return c;
    }
    return null;
  }

  // ── measurement ───────────────────────────────────────────────────────────

  function clearMeasure() {
    measured.forEach(function (n) { n.remove(); });
    measured = [];
  }

  /// The gap between the hovered element and the nearest pin, on whichever axis they are
  /// separated. The number a designer asks for first, and the one nobody can read off a screenshot.
  function measure(el) {
    clearMeasure();
    var b = el.getBoundingClientRect(), best = null;
    pinned.forEach(function (sel) {
      var other = find(sel);
      if (!other || other === el) return;
      var a = other.getBoundingClientRect();
      var gapX = a.right <= b.left ? b.left - a.right : (b.right <= a.left ? a.left - b.right : null);
      var gapY = a.bottom <= b.top ? b.top - a.bottom : (b.bottom <= a.top ? a.top - b.bottom : null);
      var d = gapX === null ? gapY : (gapY === null ? gapX : Math.min(gapX, gapY));
      if (d === null) return;
      if (!best || d < best.d) {
        best = { d: d, a: a, b: b, vertical: gapY !== null && (gapX === null || gapY <= gapX) };
      }
    });
    if (!best) return;
    var line = document.createElement('div');
    line.setAttribute('data-keel', ''); line.setAttribute('data-keel-gap', '');
    var a = best.a, r = best.b;
    if (best.vertical) {
      var top = Math.min(a.bottom, r.bottom), x = Math.max(Math.min(a.left, r.left), 0) + 8;
      line.style.cssText += 'top:' + top + 'px;left:' + x + 'px;width:0;height:' + best.d + 'px;' +
        'border-left:1px dashed #2563eb';
    } else {
      var left = Math.min(a.right, r.right), y = Math.max(Math.min(a.top, r.top), 0) + 8;
      line.style.cssText += 'top:' + y + 'px;left:' + left + 'px;height:0;width:' + best.d + 'px;' +
        'border-top:1px dashed #2563eb';
    }
    layer().appendChild(line);
    var tag = document.createElement('div');
    tag.setAttribute('data-keel', ''); tag.setAttribute('data-keel-label', '');
    tag.textContent = Math.round(best.d) + 'px';
    tag.style.display = 'block';
    tag.style.top = (best.vertical ? Math.min(a.bottom, r.bottom) + best.d / 2 - 8 : Math.min(a.top, r.top)) + 'px';
    tag.style.left = (best.vertical ? Math.max(Math.min(a.left, r.left), 0) + 12
                                    : Math.min(a.right, r.right) + best.d / 2 - 14) + 'px';
    layer().appendChild(tag);
    measured = [line, tag];
  }

  // ── input ─────────────────────────────────────────────────────────────────

  document.addEventListener('mouseover', function (e) {
    if (!on || drag || editingEl || ours(e.target)) return;
    focusOn(e.target);
    if (e.altKey) measure(e.target); else clearMeasure();
  }, true);

  document.addEventListener('mousedown', function (e) {
    if (!on || ours(e.target) || editingEl) return;
    justDragged = false;
    // ⌘-drag moves it. A plain drag would fight text selection and every draggable widget on
    // the page.
    if (e.metaKey) startMove(e);
  }, true);

  document.addEventListener('mousemove', onDrag, true);
  document.addEventListener('mouseup', function () { endDrag(); }, true);

  document.addEventListener('dblclick', function (e) {
    if (!on || ours(e.target)) return;
    e.preventDefault(); e.stopPropagation();
    editText(e.target);
  }, true);

  document.addEventListener('click', function (e) {
    if (!on || ours(e.target) || editingEl) return;
    e.preventDefault();
    // The click that ends a drag is not a pick.
    if (justDragged) { justDragged = false; e.stopPropagation(); return; }
    e.stopPropagation();
    // Whatever the keyboard walked to, not what the mouse is technically over: pressing ↑ three
    // times and then clicking must pick the card, not the label inside it.
    sendPick(current && current.contains(e.target) ? current : e.target);
  }, true);

  // The arrow keys are the affordance a design tool has and a picker does not: the thing you want
  // is nearly always the parent of the thing under the cursor, and there was no way to say so.
  document.addEventListener('keydown', function (e) {
    if (!on || editingEl) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      on = false; blur();
      post({ type: 'picking', on: false });
      return;
    }
    if (!current) return;
    var next = null;
    if (e.key === 'ArrowUp' || e.key === '[') next = skipOurs(current.parentElement);
    else if (e.key === 'ArrowDown' || e.key === ']') next = firstChild(current);
    else if (e.key === 'ArrowLeft') next = sibling(current, false);
    else if (e.key === 'ArrowRight') next = sibling(current, true);
    else if (e.key === 'Enter') { e.preventDefault(); sendPick(current); return; }
    else if (e.key === 'Alt') { measure(current); return; }
    else return;
    if (!next || next === document.documentElement || next.tagName === 'BODY') return;
    e.preventDefault(); e.stopPropagation();
    focusOn(next);
  }, true);

  document.addEventListener('keyup', function (e) {
    if (on && e.key === 'Alt') clearMeasure();
  }, true);
})();

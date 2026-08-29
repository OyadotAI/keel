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

  var on = false, host = null, hover = null, pinsWanted = [], regionsShown = [], armedUntil = 0;

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
      '[data-keel-hover]{border:2px solid #3b82f6;background:rgba(59,130,246,.12);transition:all .05s linear}' +
      '[data-keel-outline]{border:2px solid #3b82f6;animation:keel-pulse 1.1s ease-in-out infinite}' +
      '[data-keel-ripple]{border:2px solid #3b82f6;animation:keel-ripple 1.2s ease-out forwards}' +
      '[data-keel-dot]{position:fixed;width:18px;height:18px;margin:-9px 0 0 -9px;border-radius:50%;' +
        'background:#3b82f6;color:#fff;font:600 10px/18px -apple-system,sans-serif;text-align:center;' +
        'pointer-events:auto;cursor:pointer;box-shadow:0 1px 4px rgba(0,0,0,.35)}' +
      '[data-keel-dot][data-keel-new]{animation:keel-beat 1.4s ease-in-out infinite}' +
      '[data-keel-dot][data-keel-pin]{background:#111;border:2px solid #3b82f6;line-height:14px}' +
      '@keyframes keel-pulse{0%,100%{box-shadow:0 0 0 0 rgba(59,130,246,.45)}50%{box-shadow:0 0 0 6px rgba(59,130,246,0)}}' +
      '@keyframes keel-ripple{0%{box-shadow:0 0 0 0 rgba(59,130,246,.6);background:rgba(59,130,246,.22)}' +
        '100%{box-shadow:0 0 0 14px rgba(59,130,246,0);background:rgba(59,130,246,0)}}' +
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
      if (!host) return;
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

  function selectorFor(el) {
    var parts = [];
    while (el && el.nodeType === 1 && parts.length < 5) {
      if (el.id) { parts.unshift('#' + CSS.escape(el.id)); break; }
      var part = el.tagName.toLowerCase();
      var cls = (typeof el.className === 'string' ? el.className : '').trim()
        .split(/\s+/).filter(function (c) { return c && !/[:\[\]\/\\.%]/.test(c); }).slice(0, 2);
      if (cls.length) part += '.' + cls.join('.');
      var parent = el.parentElement;
      if (parent) {
        var same = Array.prototype.filter.call(parent.children, function (c) {
          return c.tagName === el.tagName;
        });
        if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
      }
      parts.unshift(part);
      el = parent;
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
    for (var node = el; node && node.nodeType === 1; node = node.parentElement) {
      var rel = node.getAttribute && (node.getAttribute('data-inspector-relative-path')
        || node.getAttribute('data-source-file') || node.getAttribute('data-testid'));
      if (rel) {
        var line = node.getAttribute('data-inspector-line');
        hints.push({ kind: 'attribute', value: line ? rel + ':' + line : rel });
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
    return hints;
  }

  function rectOf(el) {
    var r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  }

  function post(msg) {
    try { window.webkit.messageHandlers.keel.postMessage(msg); } catch (e) {}
  }

  function sendPick(el) {
    post({
      type: 'pick',
      selector: selectorFor(el),
      tag: el.tagName.toLowerCase(),
      text: (el.innerText || '').trim().slice(0, 200),
      html: el.outerHTML.slice(0, 2000),
      style: styleOf(el),
      hints: sourceHints(el),
      rect: rectOf(el),
      dpr: window.devicePixelRatio || 1
    });
  }

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
    pinsWanted = pins || [];
    var old = host ? host.querySelectorAll('[data-keel-pin]') : [];
    for (var i = 0; i < old.length; i++) old[i].remove();
    pinsWanted.forEach(function (p, i) { dot(p.selector, String(i + 1), true); });
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
    regionsShown = [];
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
    regionsShown = regions;
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

  // ── messages from Keel ────────────────────────────────────────────────────

  window.addEventListener('message', function (e) {
    var m = e.data;
    if (!m || typeof m !== 'object' || !m.keel) return;
    switch (m.keel) {
      case 'pick-on': on = true; break;
      case 'pick-off': on = false; if (hover) hover.style.display = 'none'; break;
      case 'expect': arm(m.ms); break;
      case 'outline': outline(m.selector); break;
      case 'pins': drawPins(m.pins); break;
      case 'clear': clearRegions(); outline(null); drawPins([]); break;
    }
  });

  document.addEventListener('mouseover', function (e) {
    if (!on || ours(e.target)) return;
    if (!hover) hover = mkBox('data-keel-hover');
    place(hover, e.target);
  }, true);

  document.addEventListener('click', function (e) {
    if (!on || ours(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
    sendPick(e.target);
  }, true);
})();

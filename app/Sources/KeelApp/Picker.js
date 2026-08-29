// Injected into every frame of the preview, including the dev server's own — a WKUserScript with
// forMainFrameOnly:false crosses the origin boundary that an iframe script cannot. That is the
// whole reason design mode is cheaper native than it was in a browser shell.
//
// Inert until asked. It installs one message listener and nothing else, so a page that is never
// picked from pays for a no-op.
(function () {
  if (window.__keelPicker) return;
  window.__keelPicker = true;

  var on = false, box = null, last = null;

  function overlay() {
    if (box) return box;
    box = document.createElement('div');
    box.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;' +
      'border:2px solid #3b82f6;background:rgba(59,130,246,.12);border-radius:2px;' +
      'transition:all .05s linear';
    document.documentElement.appendChild(box);
    return box;
  }

  function move(el) {
    var r = el.getBoundingClientRect(), b = overlay();
    b.style.top = r.top + 'px'; b.style.left = r.left + 'px';
    b.style.width = r.width + 'px'; b.style.height = r.height + 'px';
    b.style.display = 'block';
  }

  // A selector specific enough to find the element again, short enough to read. An id ends it
  // immediately; otherwise it climbs, taking a class and a position at each level.
  function selectorFor(el) {
    var parts = [];
    while (el && el.nodeType === 1 && parts.length < 5) {
      if (el.id) { parts.unshift('#' + el.id); break; }
      var part = el.tagName.toLowerCase();
      var cls = (el.className || '').toString().trim().split(/\s+/).filter(Boolean).slice(0, 2);
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

  // Not the full computed style: three hundred properties is not context, it is noise that
  // crowds out the part of the prompt that says what to change.
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

  // Source hints the frameworks leave behind, best first. React 19 removed _debugSource, so the
  // data-* attributes some dev plugins add and the owning component name are the common path.
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
          hints.push({
            kind: 'fiber',
            value: fiber._debugSource.fileName + ':' + (fiber._debugSource.lineNumber || 1)
          });
          break;
        }
        var t = fiber.type;
        var name = typeof t === 'function' ? (t.displayName || t.name)
          : (t && (t.displayName || (t.render && (t.render.displayName || t.render.name))));
        if (name && name[0] === name[0].toUpperCase()) {
          hints.push({ kind: 'component', value: name });
          break;
        }
        fiber = fiber.return;
      }
      break;
    }
    return hints;
  }

  function send(el) {
    var r = el.getBoundingClientRect();
    window.webkit.messageHandlers.keel.postMessage({
      selector: selectorFor(el),
      tag: el.tagName.toLowerCase(),
      text: (el.innerText || '').trim().slice(0, 200),
      html: el.outerHTML.slice(0, 2000),
      style: styleOf(el),
      hints: sourceHints(el),
      rect: { x: r.left, y: r.top, width: r.width, height: r.height },
      dpr: window.devicePixelRatio || 1
    });
  }

  window.addEventListener('message', function (e) {
    if (!e.data || typeof e.data !== 'object') return;
    if (e.data.keel === 'pick-on') { on = true; }
    if (e.data.keel === 'pick-off') {
      on = false;
      if (box) box.style.display = 'none';
    }
  });

  document.addEventListener('mouseover', function (e) {
    if (!on) return;
    last = e.target;
    move(e.target);
  }, true);

  document.addEventListener('click', function (e) {
    if (!on) return;
    e.preventDefault();
    e.stopPropagation();
    send(e.target);
  }, true);
})();

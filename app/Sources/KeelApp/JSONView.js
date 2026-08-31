// A JSON response, read as JSON rather than as one long line of text.
//
// The preview's address bar goes wherever you type, and half of what a person building on this
// stack wants to look at is their own API. WebKit renders `application/json` as a text document —
// a single <pre> holding the whole payload, unwrapped and unindented — which is where Safari shows
// its own viewer and a WKWebView shows nothing of the kind.
//
// Nothing here is a dependency: `<details>` collapses, `JSON.parse` parses, and the document the
// browser already built is the source. It only ever touches a document that *is* JSON; an ordinary
// page is left exactly as it was.
//
// Everything drawn carries data-keel, the same rule Picker.js follows, so rewriting the body of a
// JSON page never reports itself to the design canvas as a region the agent changed.
(function () {
  if (window.__keelJSON) return;
  window.__keelJSON = true;

  // A payload past this many values is shown as indented text instead of a tree: a million nodes
  // is a million DOM elements, and the pane would stop responding while it built them.
  var NODE_BUDGET = 20000;

  function jsonish() {
    var type = (document.contentType || '').toLowerCase();
    if (type.indexOf('json') >= 0) return true;
    // An API that answers `text/plain` is still an API. WebKit gives a text document the same
    // shape either way, so the body itself is the test.
    if (type.indexOf('text/plain') < 0) return false;
    var head = (document.body ? document.body.textContent : '').trim().charAt(0);
    return head === '{' || head === '[';
  }

  function count(value) {
    if (value === null || typeof value !== 'object') return 1;
    var n = 1;
    for (var k in value) if (Object.prototype.hasOwnProperty.call(value, k)) {
      n += count(value[k]);
      if (n > NODE_BUDGET) return n;
    }
    return n;
  }

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    e.setAttribute('data-keel', '');
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  // ── the tree ───────────────────────────────────────────────────────────────

  function leaf(value) {
    if (value === null) return el('span', 'k-null', 'null');
    switch (typeof value) {
      case 'string': return el('span', 'k-str', JSON.stringify(value));
      case 'number': return el('span', 'k-num', String(value));
      case 'boolean': return el('span', 'k-bool', String(value));
      default: return el('span', 'k-null', String(value));
    }
  }

  /// One value, with its key when it has one. Objects and arrays become a `<details>` so the
  /// disclosure triangle is the browser's own — collapsing is what makes a large payload readable,
  /// and it is not worth a widget.
  function node(key, value, depth) {
    var row = el('div', 'k-row');
    var object = value !== null && typeof value === 'object';

    if (!object) {
      if (key !== null) {
        row.appendChild(el('span', 'k-key', key));
        row.appendChild(el('span', 'k-sep', ': '));
      }
      row.appendChild(leaf(value));
      return row;
    }

    var array = Array.isArray(value);
    var keys = array ? null : Object.keys(value);
    var size = array ? value.length : keys.length;

    // Empty is a value, not a container: a disclosure triangle that opens onto nothing reads as
    // a viewer that failed to load the rest.
    if (size === 0) {
      if (key !== null) {
        row.appendChild(el('span', 'k-key', key));
        row.appendChild(el('span', 'k-sep', ': '));
      }
      row.appendChild(el('span', 'k-brace', array ? '[]' : '{}'));
      return row;
    }

    var details = el('details', 'k-node');
    // Open far enough to see the records themselves — an envelope like `{data:[{…}]}` spends its
    // first two levels on the envelope — but never a collection long enough to bury the rest of
    // the response under it.
    if (size > 0 && size <= 50 && depth < 3) details.open = true;

    var summary = el('summary', 'k-summary');
    if (key !== null) {
      summary.appendChild(el('span', 'k-key', key));
      summary.appendChild(el('span', 'k-sep', ': '));
    }
    summary.appendChild(el('span', 'k-brace', array ? '[' : '{'));
    summary.appendChild(el('span', 'k-count', size + (array
      ? (size === 1 ? ' item' : ' items')
      : (size === 1 ? ' key' : ' keys'))));
    summary.appendChild(el('span', 'k-brace', array ? ']' : '}'));
    details.appendChild(summary);

    var body = el('div', 'k-body');
    if (array) {
      for (var i = 0; i < size; i++) body.appendChild(node(String(i), value[i], depth + 1));
    } else {
      for (var j = 0; j < size; j++) body.appendChild(node(keys[j], value[keys[j]], depth + 1));
    }
    details.appendChild(body);
    row.appendChild(details);
    return row;
  }

  // ── the page ───────────────────────────────────────────────────────────────

  function style() {
    var s = el('style');
    s.textContent = [
      ':root{color-scheme:light dark}',
      'body[data-keel-json]{margin:0;font:12px/1.55 ui-monospace,SFMono-Regular,Menlo,monospace;',
      '  background:#fff;color:#1f2328}',
      '@media (prefers-color-scheme:dark){body[data-keel-json]{background:#0d1117;color:#e6edf3}}',
      '.k-bar{position:sticky;top:0;display:flex;gap:8px;align-items:center;padding:6px 10px;',
      '  background:inherit;border-bottom:1px solid rgba(127,127,127,.25);font-size:11px}',
      '.k-bar b{font-weight:600}',
      '.k-bar .k-dim{opacity:.55;font-weight:400}',
      '.k-spacer{flex:1}',
      '.k-bar button{font:inherit;padding:2px 8px;border-radius:5px;cursor:pointer;',
      '  border:1px solid rgba(127,127,127,.35);background:transparent;color:inherit}',
      '.k-bar button:hover{background:rgba(127,127,127,.15)}',
      // Room on the left for the outermost disclosure triangle, which hangs into the margin.
      '.k-tree{padding:8px 12px 40px 26px}',
      '.k-row{padding-left:14px}',
      '.k-node{margin:0}',
      '.k-summary{cursor:default;margin-left:-14px;list-style-position:outside}',
      '.k-summary::marker{color:#8b949e}',
      '.k-key{color:#0550ae}.k-str{color:#0a7c42}.k-num{color:#8250df}',
      '.k-bool{color:#bc4c00}.k-null{opacity:.55}.k-brace,.k-sep{opacity:.55}',
      '.k-count{opacity:.55;margin:0 4px;font-size:11px}',
      '@media (prefers-color-scheme:dark){',
      '  .k-key{color:#79c0ff}.k-str{color:#7ee787}.k-num{color:#d2a8ff}.k-bool{color:#ffa657}',
      '  .k-summary::marker{color:#6e7681}}',
      '.k-raw{white-space:pre;padding:8px 12px 40px;margin:0}',
    ].join('');
    return s;
  }

  function render(value, raw, tooBig) {
    var bar = el('div', 'k-bar');
    var kind = Array.isArray(value) ? 'Array' : (value === null ? 'null' : typeof value);
    bar.appendChild(el('b', null, 'JSON'));
    bar.appendChild(el('span', 'k-dim', kind + ' · ' + fmtBytes(raw.length)));
    bar.appendChild(el('span', 'k-spacer'));

    var tree = el('div', 'k-tree');
    var pre = el('pre', 'k-raw');
    // Indented, because "raw" here means "the text, readable" — the unwrapped single line is what
    // this view exists to get away from. Falls back to the response as it arrived if it will not
    // re-serialise (a number too big for a double, say).
    try { pre.textContent = JSON.stringify(value, null, 2); } catch (e) { pre.textContent = raw; }

    if (tooBig) {
      tree.appendChild(el('div', 'k-dim',
        'Too large to expand as a tree — shown as indented text.'));
    } else {
      tree.appendChild(node(null, value, 0));
      pre.style.display = 'none';

      var expanded = false;
      var all = button(bar, 'Expand all', function () {
        expanded = !expanded;
        var d = tree.querySelectorAll('details');
        for (var i = 0; i < d.length; i++) d[i].open = expanded;
        all.textContent = expanded ? 'Collapse all' : 'Expand all';
      });

      var showingRaw = false;
      var toggle = button(bar, 'Raw', function () {
        showingRaw = !showingRaw;
        tree.style.display = showingRaw ? 'none' : '';
        pre.style.display = showingRaw ? '' : 'none';
        toggle.textContent = showingRaw ? 'Tree' : 'Raw';
      });
    }

    var copy = button(bar, 'Copy', function () {
      // The payload as it arrived, not as this view drew it: what you paste into a test or a
      // prompt should be the server's own answer.
      navigator.clipboard.writeText(raw).then(function () {
        copy.textContent = 'Copied';
        setTimeout(function () { copy.textContent = 'Copy'; }, 1200);
      }, function () {});
    });

    document.head.appendChild(style());
    document.body.textContent = '';
    document.body.setAttribute('data-keel-json', '');
    document.body.appendChild(bar);
    document.body.appendChild(tree);
    document.body.appendChild(pre);
  }

  function button(bar, label, onClick) {
    var b = el('button', null, label);
    b.type = 'button';
    b.addEventListener('click', onClick);
    // Pushed to the right in order, so the bar reads name-first and actions last.
    bar.appendChild(b);
    return b;
  }

  function fmtBytes(n) {
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
    return (n / 1024 / 1024).toFixed(1) + ' MB';
  }

  if (!jsonish() || !document.body) return;
  var raw = document.body.textContent || '';
  var value;
  // Not JSON after all — a 404 page served with the wrong content type is the everyday case.
  // Leave the document alone rather than replacing it with an error of our own.
  try { value = JSON.parse(raw); } catch (e) { return; }
  render(value, raw, count(value) > NODE_BUDGET);
})();

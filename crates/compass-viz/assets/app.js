// Compass visual map front-end. Runs in two modes:
//   • live     — fetches /graph and subscribes to /events (SSE) for in-place updates.
//   • snapshot — reads window.__COMPASS__ (both views inlined); no server, no live.
(function () {
  "use strict";

  // Distinct, reasonably colorblind-aware categorical palette for communities/folders/langs.
  var PALETTE = [
    "#6ea8fe", "#63e6be", "#ffd43b", "#ff8787", "#b197fc", "#74c0fc",
    "#ffa94d", "#8ce99a", "#f783ac", "#a9e34b", "#66d9e8", "#ffc078",
    "#e599f7", "#69db7c", "#ff6b6b", "#4dabf7", "#ffe066", "#da77f2",
    "#3bc9db", "#9775fa", "#94d82d", "#fab005",
  ];
  var HUB_COLOR = "#7d8597";

  var snapshot = window.__COMPASS__ || null;
  var reduceMotion = window.matchMedia &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  var $ = function (id) { return document.getElementById(id); };
  function esc(s) {
    return String(s).replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }

  var cy = cytoscape({
    container: $("cy"),
    wheelSensitivity: 0.85,
    minZoom: 0.04,
    maxZoom: 4,
    style: [
      {
        selector: "node",
        style: {
          "background-color": "data(color)",
          "width": "data(size)",
          "height": "data(size)",
          "label": "data(display)",
          "text-wrap": "wrap",
          "text-max-width": 140,
          "line-height": 1.25,
          "color": "#cdd2de",
          "font-size": 7,
          "font-weight": 500,
          "text-opacity": 0,
          "text-valign": "bottom",
          "text-halign": "center",
          "text-margin-y": 3,
          "text-outline-color": "#0d0f15",
          "text-outline-width": 1.5,
          "min-zoomed-font-size": 6,
          "border-width": 0,
          "transition-property": "opacity",
          "transition-duration": "120ms",
        },
      },
      { selector: 'node[kind = "symbol"]', style: { shape: "diamond", "font-size": 6 } },
      { selector: "node[?isHub]", style: { shape: "round-rectangle" } },
      // Mapped, but its contents were skipped (too large / minified): hollow, so it doesn't
      // read as an ordinary file with nothing in it.
      {
        selector: "node[notAnalysed]",
        style: { "background-opacity": 0.15, "border-width": 1.5, "border-style": "dashed", "border-color": "data(color)" },
      },
      {
        selector: "edge",
        style: {
          "width": 1,
          "line-color": "#323848",
          "curve-style": "straight",
          "opacity": 0.5,
        },
      },
      { selector: 'edge[kind = "calls"]', style: { "line-style": "dashed" } },
      // A symbol sits in the cloud around its file, so the file→symbol "defines" edge says
      // nothing position doesn't already say — and there is one per symbol. Not drawn.
      { selector: 'edge[kind = "defines"]', style: { "display": "none" } },
      // Heuristic edges (guessed: namespace→dir, unique-global call) read as dashed + faint,
      // so a human can tell a guess from a path-exact certainty. `dotted` distinguishes a
      // heuristic import from a (dashed) call edge.
      {
        selector: 'edge[confidence = "Heuristic"]',
        style: { "line-style": "dotted", "opacity": 0.28 },
      },
      // A file category the viewer switched off (and its symbols). `display: none` also drops
      // the edges that touch it.
      { selector: ".cat-hidden", style: { "display": "none" } },
      { selector: ".show-label", style: { "text-opacity": 1 } },
      { selector: ".dim", style: { opacity: 0.07 } },
      {
        selector: ".match",
        style: {
          "text-opacity": 1,
          "border-width": 2,
          "border-color": "#ffffff",
          "z-index": 99,
        },
      },
    ],
    elements: [],
  });

  var colorMode = "group";
  var includeSymbols = false;
  var version = 0;
  var layout = null;
  var legendData = [];

  function colorKey(node) {
    var d = node.data();
    if (colorMode === "group") return "g" + d.group;
    if (colorMode === "folder") return "d" + (d.folder || "");
    return "l" + (d.language || "");
  }

  // ---- file categories (code, markup, data, …) ---------------------------------------------
  // Which categories exist comes from the data, and whether one is "supporting" (non-code)
  // comes from core — nothing here knows a category by name. Supporting categories start
  // hidden; whatever the viewer switches is remembered for next time.
  var CATEGORY_PREFS_KEY = "compass.categoryVisible";
  var categoryPrefs = (function () {
    try { return JSON.parse(localStorage.getItem(CATEGORY_PREFS_KEY)) || {}; } catch (e) { return {}; }
  })();
  var categories = [];                              // [{ name, count, supporting, visible }]

  function categoryVisible(name, supporting) {
    return Object.prototype.hasOwnProperty.call(categoryPrefs, name) ? !!categoryPrefs[name] : !supporting;
  }
  function setCategoryVisible(name, visible) {
    categoryPrefs[name] = visible;
    try { localStorage.setItem(CATEGORY_PREFS_KEY, JSON.stringify(categoryPrefs)); } catch (e) { /* private mode */ }
  }
  function visibleFiles() { return cy.nodes('[kind = "file"]').not(".cat-hidden"); }

  function applyCategories() {
    var byName = {};
    cy.nodes('[kind = "file"]').forEach(function (n) {
      var name = n.data("category") || "code";
      var c = byName[name] || (byName[name] = { name: name, count: 0, supporting: !!n.data("supporting") });
      c.count++;
    });
    categories = Object.keys(byName).sort().map(function (k) {
      byName[k].visible = categoryVisible(k, byName[k].supporting);
      return byName[k];
    });
    var hidden = {};
    categories.forEach(function (c) { if (!c.visible) hidden[c.name] = true; });
    cy.batch(function () {
      cy.nodes().forEach(function (n) {           // symbols carry their file's category
        n.toggleClass("cat-hidden", !!hidden[n.data("category") || "code"]);
      });
    });
  }

  function applyColors() {
    var keys = Array.from(new Set(cy.nodes().map(colorKey))).sort();
    var index = new Map(keys.map(function (k, i) { return [k, i]; }));

    // Per-key count + a representative label (the most-connected file's folder/name).
    var meta = {};
    visibleFiles().forEach(function (n) {
      var k = colorKey(n);
      var m = meta[k] || (meta[k] = { count: 0, topDeg: -1, label: "" });
      m.count++;
      var deg = n.data("degree") || 0;
      if (deg > m.topDeg) {
        m.topDeg = deg;
        m.label = folderLabel(n.data("path")) || n.data("label");
      }
    });

    cy.batch(function () {
      cy.nodes().forEach(function (n) {
        var color = (colorMode === "group" && n.data("isHub"))
          ? HUB_COLOR
          : PALETTE[index.get(colorKey(n)) % PALETTE.length];
        n.data("color", color);
      });
    });

    legendData = Object.keys(meta).map(function (k) {
      var label = colorMode === "folder" ? (k.slice(1) || "(root)")
        : colorMode === "language" ? (k.slice(1) || "(unknown)")
        : (meta[k].label || "cluster");
      return { color: PALETTE[index.get(k) % PALETTE.length], label: label, count: meta[k].count };
    }).sort(function (a, b) { return b.count - a.count; });

    if (colorMode === "group") {
      var hubs = cy.nodes('[kind = "file"][?isHub]').length;
      if (hubs) legendData.push({ color: HUB_COLOR, label: "shared (hubs)", count: hubs, hub: true });
    }
    renderLegend();
    scheduleHulls();
  }

  // The most *meaningful* folder for a file: its parent directory, skipping a trailing generic
  // source dir (src/lib/app/…) so `crates/compass-viz/src/lib.rs` → "compass-viz", not "src".
  var GENERIC_DIRS = { src: 1, lib: 1, source: 1, sources: 1, app: 1, dist: 1, build: 1 };
  function folderLabel(path) {
    var parts = (path || "").split(/[\/\\]/).filter(Boolean);
    parts.pop();
    if (parts.length > 1 && GENERIC_DIRS[parts[parts.length - 1].toLowerCase()]) parts.pop();
    return parts.length ? parts[parts.length - 1] : "";
  }

  function decorate() {
    cy.batch(function () {
      cy.nodes().forEach(function (n) {
        var deg = n.data("degree") || 0;
        n.data("size", 12 + Math.sqrt(deg) * 6);
        if (n.data("kind") === "file") {
          var folder = folderLabel(n.data("path"));
          n.data("display", folder ? n.data("label") + "\n" + folder : n.data("label"));
        } else {
          n.data("display", n.data("label"));
        }
      });
    });
    applyColors();
  }

  // ---- layout ---------------------------------------------------------------------------
  // Only FILES go through the force simulation. It is O(nodes²) per iteration and runs on the
  // main thread: 73 files settle in ~0.1 s, but the same repo's 1,700 file+symbol nodes took
  // ~40 s, freezing the tab. Symbols don't need simulating — each belongs to exactly one file —
  // so they are placed in a sunflower spiral around it, which is instant and keeps a file's
  // symbols visibly together.
  var SYMBOL_SPACING = 8;                           // spiral constant; ~1.8x this between symbols
  var GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

  function symbolsByFile() {
    var byFile = {};
    cy.nodes('[kind = "symbol"]').forEach(function (s) {
      var path = s.data("path");
      (byFile[path] || (byFile[path] = [])).push(s);
    });
    return byFile;
  }

  // Radius a file needs so its symbol cloud doesn't run into its neighbours'.
  function cloudRadius(file, symbolCount) {
    var r = file.data("size") / 2;
    return symbolCount ? r + SYMBOL_SPACING * (Math.sqrt(symbolCount) + 1) : r;
  }

  function placeSymbols(byFile) {
    cy.batch(function () {
      visibleFiles().forEach(function (file) {
        var center = file.position(), inner = file.data("size") / 2;
        (byFile[file.id()] || []).forEach(function (symbol, i) {
          var r = inner + SYMBOL_SPACING * Math.sqrt(i + 1), a = i * GOLDEN_ANGLE;
          symbol.position({ x: center.x + r * Math.cos(a), y: center.y + r * Math.sin(a) });
        });
      });
    });
  }

  function runLayout(fresh) {
    if (layout) layout.stop();
    var files = visibleFiles();
    var byFile = symbolsByFile();
    var withSymbols = cy.nodes('[kind = "symbol"]').not(".cat-hidden").nonempty();

    // Let the simulation see each file at the size of its whole cloud, so it leaves room. The
    // layout reads a node's rendered size, so the real size is swapped back in `layoutstop` —
    // within the same synchronous run (no animation with symbols), so it is never painted.
    var area = 0;
    files.forEach(function (f) {
      var d = cloudRadius(f, (byFile[f.id()] || []).length) * 2;
      area += d * d;
      if (withSymbols) { f.scratch("_size", f.data("size")); f.data("size", d); }
    });
    var width = Math.max(cy.width(), Math.sqrt(area * 2.5 * 16 / 9));

    layout = files.union(files.edgesWith(files).filter('[kind = "import"]')).layout({
      name: "cose",
      // Symbols are positioned once the files have settled; animating the files would leave
      // the clouds trailing behind them, so with symbols shown the layout is applied at once.
      animate: fresh || withSymbols ? false : !reduceMotion,
      randomize: fresh,
      fit: false,
      boundingBox: { x1: 0, y1: 0, w: width, h: width * 9 / 16 },
      nodeRepulsion: 9000,
      nodeOverlap: 20,
      idealEdgeLength: 70,
      edgeElasticity: 80,
      gravity: 0.3,
      numIter: 1200,
      componentSpacing: 90,
    });
    layout.one("layoutstop", function () {
      if (withSymbols) files.forEach(function (f) { f.data("size", f.scratch("_size")); });
      placeSymbols(byFile);
      clouds = byFile;
      if (fresh || withSymbols) fitAll();
    });
    layout.run();
  }

  // Frame every node. `cy.fit` measures labels too — including the hidden ones under each of
  // a thousand symbols — which leaves the graph small in a corner, so measure the nodes alone
  // and keep clear of the toolbar (top) and the legend (right).
  function fitAll() {
    var box = cy.nodes().not(".cat-hidden")
      .boundingBox({ includeLabels: false, includeOverlays: false });
    if (!box.w || !box.h) return;
    var pad = { top: 90, right: 250, bottom: 60, left: 40 };
    var w = Math.max(cy.width() - pad.left - pad.right, 100);
    var h = Math.max(cy.height() - pad.top - pad.bottom, 100);
    var zoom = Math.min(w / box.w, h / box.h, cy.maxZoom());
    cy.viewport({
      zoom: zoom,
      pan: {
        x: pad.left + (w - box.w * zoom) / 2 - box.x1 * zoom,
        y: pad.top + (h - box.h * zoom) / 2 - box.y1 * zoom,
      },
    });
  }

  // Dragging a file carries its symbol cloud along.
  var clouds = {};                                  // file id -> its symbol nodes, as last laid out
  var dragFrom = null;
  cy.on("grab", 'node[kind = "file"]', function (evt) {
    var p = evt.target.position();
    dragFrom = { x: p.x, y: p.y };
  });
  cy.on("drag", 'node[kind = "file"]', function (evt) {
    var cloud = clouds[evt.target.id()];
    if (!dragFrom || !cloud) return;
    var p = evt.target.position(), dx = p.x - dragFrom.x, dy = p.y - dragFrom.y;
    dragFrom = { x: p.x, y: p.y };
    cy.batch(function () {
      cloud.forEach(function (symbol) {
        var q = symbol.position();
        symbol.position({ x: q.x + dx, y: q.y + dy });
      });
    });
  });
  cy.on("free", 'node[kind = "file"]', function () { dragFrom = null; });

  // ---- cluster hulls (translucent "puddles" behind same-colored file nodes) -------------
  var hullCanvas = $("hulls");
  var hctx = hullCanvas.getContext("2d");
  var hullRAF = null;

  function sizeHullCanvas() {
    var dpr = window.devicePixelRatio || 1;
    hullCanvas.width = Math.round(hullCanvas.clientWidth * dpr);
    hullCanvas.height = Math.round(hullCanvas.clientHeight * dpr);
    hctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }
  function hullsEnabled() { return !includeSymbols && cy.nodes('[kind = "file"]').length > 1; }
  function scheduleHulls() { if (!hullRAF) hullRAF = requestAnimationFrame(drawHulls); }

  function hexA(hex, a) {
    var h = hex.replace("#", "");
    return "rgba(" + parseInt(h.substr(0, 2), 16) + "," + parseInt(h.substr(2, 2), 16) +
      "," + parseInt(h.substr(4, 2), 16) + "," + a + ")";
  }
  function convexHull(pts) {
    if (pts.length < 3) return pts.slice();
    var p = pts.slice().sort(function (a, b) { return a.x - b.x || a.y - b.y; });
    var cr = function (o, a, b) { return (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x); };
    var lo = [], hi = [], i;
    for (i = 0; i < p.length; i++) {
      while (lo.length >= 2 && cr(lo[lo.length - 2], lo[lo.length - 1], p[i]) <= 0) lo.pop();
      lo.push(p[i]);
    }
    for (i = p.length - 1; i >= 0; i--) {
      while (hi.length >= 2 && cr(hi[hi.length - 2], hi[hi.length - 1], p[i]) <= 0) hi.pop();
      hi.push(p[i]);
    }
    lo.pop(); hi.pop();
    return lo.concat(hi);
  }
  function drawBlob(hull, pad, fill) {
    var cx = 0, cy0 = 0, i;
    for (i = 0; i < hull.length; i++) { cx += hull[i].x; cy0 += hull[i].y; }
    cx /= hull.length; cy0 /= hull.length;
    var pts = hull.map(function (p) {
      var dx = p.x - cx, dy = p.y - cy0, d = Math.hypot(dx, dy) || 1;
      return { x: p.x + (dx / d) * pad, y: p.y + (dy / d) * pad };
    });
    hctx.fillStyle = fill;
    hctx.beginPath();
    if (pts.length < 3) { hctx.arc(cx, cy0, pad + 16, 0, Math.PI * 2); hctx.fill(); return; }
    var mid = function (a, b) { return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }; };
    var start = mid(pts[pts.length - 1], pts[0]);
    hctx.moveTo(start.x, start.y);
    for (i = 0; i < pts.length; i++) {
      var cur = pts[i], next = pts[(i + 1) % pts.length], m = mid(cur, next);
      hctx.quadraticCurveTo(cur.x, cur.y, m.x, m.y);
    }
    hctx.closePath();
    hctx.fill();
  }
  function drawHulls() {
    hullRAF = null;
    hctx.clearRect(0, 0, hullCanvas.clientWidth, hullCanvas.clientHeight);
    if (!hullsEnabled()) return;
    // Group by community/folder/language KEY (not by color — the palette cycles, so distinct
    // groups can share a color and must not merge into one hull). Only real clusters get a
    // puddle; singletons/pairs (e.g. disconnected leaf files) are left bare.
    var groups = {};
    visibleFiles().forEach(function (n) {
      if (colorMode === "group" && n.data("isHub")) return;
      var k = colorKey(n);
      var g = groups[k] || (groups[k] = { color: n.data("color"), pts: [] });
      g.pts.push(n.renderedPosition());
    });
    Object.keys(groups).forEach(function (k) {
      var g = groups[k];
      if (g.pts.length < 3) return;
      drawBlob(convexHull(g.pts), 24, hexA(g.color, 0.1));
    });
  }
  cy.on("render", scheduleHulls);
  window.addEventListener("resize", function () { sizeHullCanvas(); scheduleHulls(); });

  // ---- tooltip --------------------------------------------------------------------------
  var tip = $("tooltip");
  function showTip(node, rp) {
    var d = node.data();
    var meta = d.kind === "symbol"
      ? ((d.symbolKind || "symbol") + (d.language ? " · " + d.language : ""))
      : (d.language || "file");
    // A skipped file would otherwise read as an empty one.
    if (d.notAnalysed) meta += " · not analysed: " + d.notAnalysed;
    tip.innerHTML = "<strong>" + esc(d.path || d.label) + "</strong>" +
      '<span class="tip-meta">' + esc(meta) + "</span>";
    tip.hidden = false;
    var pad = 14, tw = tip.offsetWidth, th = tip.offsetHeight;
    var x = rp.x + pad, y = rp.y + pad;
    if (x + tw > window.innerWidth - 8) x = rp.x - tw - pad;
    if (y + th > window.innerHeight - 8) y = rp.y - th - pad;
    tip.style.left = Math.max(8, x) + "px";
    tip.style.top = Math.max(8, y) + "px";
  }
  cy.on("mouseover", "node", function (e) { showTip(e.target, e.renderedPosition || e.target.renderedPosition()); });
  cy.on("mouseout", "node", function () { tip.hidden = true; });
  cy.on("pan zoom drag", function () { tip.hidden = true; });

  // ---- legend ---------------------------------------------------------------------------
  var legendEl = $("legend");
  function renderLegend() {
    if (!legendData.length && categories.length < 2) { legendEl.hidden = true; return; }
    var MAX = 8;
    var title = colorMode === "folder" ? "Folders" : colorMode === "language" ? "Languages" : "Sub-parts";
    var html = '<div class="legend-title">' + title + " · " + legendData.length + "</div>";
    legendData.slice(0, MAX).forEach(function (e) {
      html += '<div class="legend-row"><span class="swatch' + (e.hub ? " hub" : "") +
        '" style="background:' + e.color + '"></span><span class="name">' + esc(e.label) +
        '</span><span class="count">' + e.count + "</span></div>";
    });
    if (legendData.length > MAX) html += '<div class="legend-more">+' + (legendData.length - MAX) + " more</div>";
    // One-line note so a human can read dotted/faint edges as guessed, not certain.
    if (cy.edges('[confidence = "Heuristic"]').nonempty()) {
      html += '<div class="legend-note"><span class="edge-sample" aria-hidden="true"></span>' +
        "dotted &amp; faint = heuristic (guessed) edge</div>";
    }
    // Only worth a control when there is something to choose between.
    if (categories.length > 1) {
      // A labelled group, not a <fieldset>: a <legend> is drawn *on* the border line.
      html += '<div class="legend-filters" role="group" aria-labelledby="legend-show">' +
        '<div class="legend-title" id="legend-show">Show</div>';
      categories.forEach(function (c) {
        html += '<label class="legend-row filter"><input type="checkbox" data-category="' + esc(c.name) + '"' +
          (c.visible ? " checked" : "") + '><span class="name">' + esc(c.name) +
          '</span><span class="count">' + c.count + "</span></label>";
      });
      html += "</div>";
    }
    legendEl.innerHTML = html;
    legendEl.hidden = false;
  }
  legendEl.addEventListener("change", function (e) {
    var name = e.target.getAttribute && e.target.getAttribute("data-category");
    if (name === null || name === undefined) return;
    setCategoryVisible(name, e.target.checked);
    applyCategories();
    applyColors();                                  // re-renders the legend, keeping focus below
    runLayout(false);
    updateCounts();
    applySearch();
    var again = legendEl.querySelector('input[data-category="' + name.replace(/"/g, '\\"') + '"]');
    if (again) again.focus();
  });

  // ---- elements / data ------------------------------------------------------------------
  function reconcile(elements) {
    var ids = new Set();
    elements.nodes.forEach(function (n) { ids.add(n.data.id); });
    elements.edges.forEach(function (e) { ids.add(e.data.id); });
    cy.batch(function () {
      cy.elements().forEach(function (el) { if (!ids.has(el.id())) el.remove(); });
      elements.nodes.forEach(function (n) {
        var ex = cy.getElementById(n.data.id);
        if (ex.nonempty()) ex.data(n.data);
        else cy.add({ group: "nodes", data: n.data });
      });
      elements.edges.forEach(function (e) {
        if (cy.getElementById(e.data.id).empty()) cy.add({ group: "edges", data: e.data });
      });
    });
  }

  function setElements(elements, fresh) {
    var before = cy.elements().length;
    if (fresh) { cy.elements().remove(); cy.add(elements); }
    else { reconcile(elements); }
    applyCategories();
    decorate();
    runLayout(fresh || before === 0);
    updateCounts();
    applySearch();
  }

  function updateCounts() {
    var allFiles = cy.nodes('[kind = "file"]').length;
    var files = visibleFiles().length;
    var syms = cy.nodes('[kind = "symbol"]').not(".cat-hidden").length;
    var parts = [files + " files"];
    if (syms) parts.push(syms + " symbols");
    parts.push(cy.edges(":visible").length + " edges");
    if (allFiles > files) parts.push((allFiles - files) + " hidden");
    $("counts").textContent = parts.join("  ·  ");
    $("empty").hidden = allFiles + syms > 0;
  }

  function loadGraph(fresh) {
    if (snapshot) {
      setElements(includeSymbols ? snapshot.symbols : snapshot.files, fresh);
      return Promise.resolve();
    }
    return fetch("/graph?symbols=" + (includeSymbols ? "1" : "0"))
      .then(function (r) { return r.json(); })
      .then(function (payload) { version = payload.version; setElements(payload.elements, fresh); });
  }

  function setLive(on) {
    var el = $("live");
    el.textContent = on ? "live" : (snapshot ? "snapshot" : "offline");
    el.classList.toggle("off", !on);
  }
  function connectLive() {
    if (snapshot || typeof EventSource === "undefined") { setLive(false); return; }
    var es = new EventSource("/events");
    es.onopen = function () { setLive(true); };
    es.onerror = function () { setLive(false); };
    es.onmessage = function (e) {
      var v = parseInt(e.data, 10);
      if (!isNaN(v) && v !== version) { version = v; loadGraph(false); }
    };
  }

  // ---- search ---------------------------------------------------------------------------
  var searchEl = $("search");
  function applySearch() {
    var q = searchEl.value.trim().toLowerCase();
    cy.batch(function () {
      cy.elements().removeClass("dim match");
      if (!q) return;
      var matched = cy.nodes().filter(function (n) {
        return (n.data("path") || "").toLowerCase().indexOf(q) !== -1 ||
          (n.data("label") || "").toLowerCase().indexOf(q) !== -1;
      });
      if (matched.length === 0) return;
      cy.elements().not(matched.closedNeighborhood()).addClass("dim");
      matched.addClass("match");
    });
  }
  searchEl.addEventListener("input", applySearch);

  // ---- controls -------------------------------------------------------------------------
  $("color-mode").addEventListener("change", function (e) { colorMode = e.target.value; applyColors(); });
  // Not a fresh layout: the files keep their places and only make room for (or close up after)
  // the symbol clouds, so the map stays recognisable.
  $("symbols").addEventListener("change", function (e) {
    includeSymbols = e.target.checked;
    $("loading").hidden = false;
    loadGraph(false).then(function () { $("loading").hidden = true; });
  });
  $("fit").addEventListener("click", function () {
    cy.animate({ fit: { eles: cy.elements(), padding: 50 }, duration: reduceMotion ? 0 : 300 });
  });

  // Reveal labels as you zoom in (Obsidian-style); searched/selected always show.
  cy.on("zoom", function () {
    var show = cy.zoom() > 0.55;
    cy.batch(function () { cy.nodes('[kind = "file"]').toggleClass("show-label", show); });
  });
  cy.on("tap", "node", function (evt) { evt.target.toggleClass("show-label"); });

  // ---- boot -----------------------------------------------------------------------------
  // The token-savings dashboard needs the live server; hide its link in offline snapshots.
  if (snapshot) { var tl = $("tokens-link"); if (tl) tl.hidden = true; }
  sizeHullCanvas();
  loadGraph(true).then(function () { $("loading").hidden = true; connectLive(); });
})();

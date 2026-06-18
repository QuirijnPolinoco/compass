// Compass visual map front-end. Runs in two modes:
//   • live     — fetches /graph and subscribes to /events (SSE) for in-place updates.
//   • snapshot — reads window.__COMPASS__ (both views inlined); no server, no live.
(function () {
  "use strict";

  // Distinct, reasonably colorblind-friendly categorical palette for communities/folders.
  var PALETTE = [
    "#6ea8fe", "#63e6be", "#ffd43b", "#ff8787", "#b197fc", "#74c0fc",
    "#ffa94d", "#8ce99a", "#f783ac", "#a9e34b", "#66d9e8", "#ffc078",
    "#e599f7", "#69db7c", "#ff6b6b", "#4dabf7", "#ffe066", "#da77f2",
    "#3bc9db", "#9775fa", "#94d82d", "#fab005",
  ];
  var HUB_COLOR = "#7d8597";

  var snapshot = window.__COMPASS__ || null;

  var cy = cytoscape({
    container: document.getElementById("cy"),
    wheelSensitivity: 0.2,
    minZoom: 0.04,
    maxZoom: 4,
    style: [
      {
        selector: "node",
        style: {
          "background-color": "data(color)",
          "width": "data(size)",
          "height": "data(size)",
          "label": "data(label)",
          "color": "#c7ccd8",
          "font-size": 7,
          "text-opacity": 0,
          "text-valign": "bottom",
          "text-halign": "center",
          "text-margin-y": 3,
          "min-zoomed-font-size": 6,
          "border-width": 0,
        },
      },
      { selector: 'node[kind = "symbol"]', style: { shape: "diamond", "font-size": 6 } },
      { selector: "node[?isHub]", style: { shape: "round-rectangle" } },
      {
        selector: "edge",
        style: {
          "width": 1,
          "line-color": "#2a2f3d",
          "curve-style": "straight",
          "opacity": 0.55,
        },
      },
      { selector: 'edge[kind = "calls"]', style: { "line-style": "dashed" } },
      { selector: 'edge[kind = "defines"]', style: { "line-color": "#222633", "opacity": 0.35 } },
      { selector: ".show-label", style: { "text-opacity": 1 } },
      { selector: ".dim", style: { opacity: 0.07 } },
      {
        selector: ".match",
        style: { "text-opacity": 1, "border-width": 2, "border-color": "#ffffff", "z-index": 99 },
      },
    ],
    elements: [],
  });

  var colorMode = "group";
  var includeSymbols = false;
  var version = 0;
  var layout = null;

  function colorKey(node) {
    var d = node.data();
    if (colorMode === "group") return "g" + d.group;
    if (colorMode === "folder") return "d" + (d.folder || "");
    return "l" + (d.language || "");
  }

  function applyColors() {
    var keys = Array.from(new Set(cy.nodes().map(colorKey))).sort();
    var index = new Map(keys.map(function (k, i) { return [k, i]; }));
    cy.batch(function () {
      cy.nodes().forEach(function (n) {
        var color;
        if (colorMode === "group" && n.data("isHub")) color = HUB_COLOR;
        else color = PALETTE[index.get(colorKey(n)) % PALETTE.length];
        n.data("color", color);
      });
    });
  }

  function decorate() {
    cy.batch(function () {
      cy.nodes().forEach(function (n) {
        var deg = n.data("degree") || 0;
        n.data("size", 12 + Math.sqrt(deg) * 6);
      });
    });
    applyColors();
  }

  // `fresh` (first load / topology change) lays out instantly and fits the viewport;
  // live updates relax from current positions and keep the user's zoom/pan (in-place glide).
  function runLayout(fresh) {
    if (layout) layout.stop();
    layout = cy.layout({
      name: "cose",
      animate: !fresh,
      randomize: fresh,
      fit: fresh,
      padding: 60,
      nodeRepulsion: 9000,
      idealEdgeLength: 70,
      edgeElasticity: 80,
      gravity: 0.3,
      numIter: 1200,
      componentSpacing: 90,
    });
    layout.run();
  }

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
    if (fresh) {
      cy.elements().remove();
      cy.add(elements);
    } else {
      reconcile(elements);
    }
    decorate();
    // Re-randomize only on a fresh load; otherwise relax from current positions (glide).
    runLayout(fresh || before === 0);
    updateCounts();
    applySearch();
  }

  function updateCounts() {
    var files = cy.nodes('[kind = "file"]').length;
    var syms = cy.nodes('[kind = "symbol"]').length;
    var edges = cy.edges().length;
    var parts = [files + " files"];
    if (syms) parts.push(syms + " symbols");
    parts.push(edges + " edges");
    document.getElementById("counts").textContent = parts.join("  ·  ");
  }

  function loadGraph(fresh) {
    if (snapshot) {
      setElements(includeSymbols ? snapshot.symbols : snapshot.files, fresh);
      return Promise.resolve();
    }
    return fetch("/graph?symbols=" + (includeSymbols ? "1" : "0"))
      .then(function (r) { return r.json(); })
      .then(function (payload) {
        version = payload.version;
        setElements(payload.elements, fresh);
      });
  }

  function setLive(on) {
    var el = document.getElementById("live");
    el.textContent = on ? "live" : (snapshot ? "snapshot" : "offline");
    el.classList.toggle("off", !on);
  }

  function connectLive() {
    if (snapshot || typeof EventSource === "undefined") {
      setLive(false);
      return;
    }
    var es = new EventSource("/events");
    es.onopen = function () { setLive(true); };
    es.onerror = function () { setLive(false); };
    es.onmessage = function (e) {
      var v = parseInt(e.data, 10);
      if (!isNaN(v) && v !== version) {
        version = v;
        loadGraph(false);
      }
    };
  }

  // --- search ---------------------------------------------------------------
  var searchEl = document.getElementById("search");
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
      var neighborhood = matched.closedNeighborhood();
      cy.elements().not(neighborhood).addClass("dim");
      matched.addClass("match");
    });
  }
  searchEl.addEventListener("input", applySearch);

  // --- controls -------------------------------------------------------------
  document.getElementById("color-mode").addEventListener("change", function (e) {
    colorMode = e.target.value;
    applyColors();
  });
  document.getElementById("symbols").addEventListener("change", function (e) {
    includeSymbols = e.target.checked;
    loadGraph(true); // topology changes → fresh layout
  });
  document.getElementById("fit").addEventListener("click", function () {
    cy.animate({ fit: { eles: cy.elements(), padding: 50 }, duration: 300 });
  });

  // Reveal labels as you zoom in (Obsidian-style), always show searched/selected.
  cy.on("zoom", function () {
    var show = cy.zoom() > 0.55;
    cy.batch(function () { cy.nodes('[kind = "file"]').toggleClass("show-label", show); });
  });
  cy.on("tap", "node", function (evt) {
    evt.target.toggleClass("show-label");
  });

  // --- boot -----------------------------------------------------------------
  loadGraph(true).then(connectLive);
})();

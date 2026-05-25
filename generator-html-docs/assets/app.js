// generator-html-docs runtime: persistent UI state for the docs site.
//
//   - Theme toggle (light / dark / auto).
//   - Server picker in the header (populated from the spec's `servers[]`).
//   - Server-variable editor on the landing page; substituted values
//     are what `effectiveServerUrl()` returns.
//   - Sidebar "on-path" highlighting for the current page's ancestors.
//
// State lives in a single localStorage key so all pages stay in sync
// when the user changes a value on one page and navigates to another.
// The module exposes the read API at `window.openapiForge` so later
// features (request builder, auth flows) can consult it.
(function () {
  "use strict";

  var STORAGE_KEY = "openapi-forge-html-docs:state:v1";

  // ---- state ----

  function loadState() {
    var s = { theme: null, serverUrl: null, variables: {} };
    try {
      var raw = localStorage.getItem(STORAGE_KEY);
      if (raw) {
        var parsed = JSON.parse(raw);
        if (parsed && typeof parsed === "object") {
          s.theme = parsed.theme || null;
          s.serverUrl = parsed.serverUrl || null;
          s.variables = parsed.variables || {};
        }
      }
    } catch (e) {}
    return s;
  }

  function saveState(s) {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(s));
    } catch (e) {}
  }

  var state = loadState();
  var listeners = { serverchange: [] };

  function emit(name, detail) {
    (listeners[name] || []).forEach(function (cb) {
      try { cb(detail); } catch (e) {}
    });
  }

  function substitute(template, vars) {
    if (!template) return template;
    return template.replace(/\{([^{}]+)\}/g, function (m, name) {
      return vars && Object.prototype.hasOwnProperty.call(vars, name)
        ? vars[name]
        : m;
    });
  }

  function effectiveServerUrl() {
    if (!state.serverUrl) return null;
    var vars = state.variables[state.serverUrl] || {};
    return substitute(state.serverUrl, vars);
  }

  function setServer(url) {
    if (state.serverUrl === url) return;
    state.serverUrl = url;
    saveState(state);
    refreshHeaderEffective();
    emit("serverchange", { url: url, effective: effectiveServerUrl() });
  }

  function setVariable(serverUrl, name, value) {
    if (!state.variables[serverUrl]) state.variables[serverUrl] = {};
    state.variables[serverUrl][name] = value;
    saveState(state);
    if (state.serverUrl === serverUrl) {
      refreshHeaderEffective();
      emit("serverchange", {
        url: serverUrl,
        effective: effectiveServerUrl(),
      });
    }
  }

  // ---- theme ----

  function applyTheme(theme) {
    document.documentElement.setAttribute("data-theme", theme);
    var btn = document.querySelector("[data-theme-toggle]");
    if (btn) btn.setAttribute("aria-pressed", theme === "dark" ? "true" : "false");
  }

  function nextTheme(current) {
    if (current === "dark") return "light";
    if (current === "light") return "auto";
    return "dark";
  }

  if (state.theme) applyTheme(state.theme);

  // ---- header server picker ----

  function refreshHeaderEffective() {
    var out = document.querySelector("[data-server-effective]");
    if (!out) return;
    var eff = effectiveServerUrl();
    out.textContent = eff || "";
    out.hidden = !eff;
  }

  function initServerPicker() {
    var sel = document.querySelector("[data-server-picker]");
    if (!sel) return;
    // First page load — adopt the stored server, else the first option.
    var options = Array.prototype.map.call(sel.options, function (o) { return o.value; });
    if (state.serverUrl && options.indexOf(state.serverUrl) !== -1) {
      sel.value = state.serverUrl;
    } else {
      state.serverUrl = sel.value;
      saveState(state);
    }
    sel.addEventListener("change", function () {
      setServer(sel.value);
    });
    refreshHeaderEffective();
  }

  // ---- landing-page variable form ----

  function initServerVariableForms() {
    var forms = document.querySelectorAll("[data-server-variables-form]");
    Array.prototype.forEach.call(forms, function (form) {
      var serverUrl = form.dataset.serverUrl;
      var stored = state.variables[serverUrl] || {};
      var inputs = form.querySelectorAll("[data-variable]");
      Array.prototype.forEach.call(inputs, function (input) {
        var name = input.dataset.variable;
        if (Object.prototype.hasOwnProperty.call(stored, name)) {
          input.value = stored[name];
        }
        var ev = input.tagName === "SELECT" ? "change" : "input";
        input.addEventListener(ev, function () {
          setVariable(serverUrl, name, input.value);
        });
      });
    });
  }

  // ---- sidebar on-path highlighting ----

  function initSidebarOnPath() {
    var current = document.querySelector(".nav-tree a[aria-current=page]");
    var li = current && current.closest("li");
    while (li) {
      li.dataset.onPath = "true";
      li = li.parentElement && li.parentElement.closest("li");
    }
  }

  // ---- public API ----

  window.openapiForge = {
    get state() { return JSON.parse(JSON.stringify(state)); },
    get selectedServerTemplate() { return state.serverUrl; },
    get selectedServerVariables() {
      return Object.assign({}, state.variables[state.serverUrl] || {});
    },
    effectiveServerUrl: effectiveServerUrl,
    setServer: setServer,
    setVariable: setVariable,
    on: function (name, cb) {
      if (!listeners[name]) listeners[name] = [];
      listeners[name].push(cb);
    },
  };

  // ---- bootstrap ----

  document.addEventListener("DOMContentLoaded", function () {
    var btn = document.querySelector("[data-theme-toggle]");
    if (btn) {
      btn.addEventListener("click", function () {
        var current = document.documentElement.getAttribute("data-theme") || "auto";
        var next = nextTheme(current);
        applyTheme(next);
        state.theme = next;
        saveState(state);
      });
    }
    initServerPicker();
    initServerVariableForms();
    initSidebarOnPath();
  });
})();

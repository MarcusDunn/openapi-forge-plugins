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
  var AUTH_KEY = "openapi-forge-html-docs:auth:v1";

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

  // ---- copy-to-clipboard ----

  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).catch(function () { fallbackCopy(text); });
    } else {
      fallbackCopy(text);
    }
  }

  function fallbackCopy(text) {
    var ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    try { document.execCommand("copy"); } catch (e) {}
    document.body.removeChild(ta);
  }

  function flashCopied(btn) {
    var original = btn.textContent;
    btn.textContent = "copied";
    btn.classList.add("copied");
    setTimeout(function () {
      btn.textContent = original;
      btn.classList.remove("copied");
    }, 900);
  }

  function initCopyButtons() {
    document.addEventListener("click", function (ev) {
      var btn = ev.target.closest("[data-copy-btn]");
      if (!btn) return;
      var text = btn.dataset.copyText;
      if (!text && btn.dataset.copyEndpoint) {
        var base = effectiveServerUrl() || "";
        // Strip trailing `/` from base to avoid double-slash on join.
        text = base.replace(/\/+$/, "") + btn.dataset.copyEndpoint;
      }
      if (!text) return;
      copyText(text);
      flashCopied(btn);
    });
  }

  // ---- auth state (sessionStorage; tab-scoped) ----

  function loadAuth() {
    try {
      var raw = sessionStorage.getItem(AUTH_KEY);
      if (raw) {
        var parsed = JSON.parse(raw);
        if (parsed && typeof parsed === "object" && parsed.schemes) return parsed;
      }
    } catch (e) {}
    return { schemes: {} };
  }

  function saveAuth(a) {
    try { sessionStorage.setItem(AUTH_KEY, JSON.stringify(a)); } catch (e) {}
  }

  var authState = loadAuth();

  /// Is the credential for `schemeId` valid right now?
  function schemeSatisfied(schemeId) {
    var s = authState.schemes[schemeId];
    if (!s) return false;
    if (s.kind === "bearer") return !!s.token;
    if (s.kind === "oauth2-client-credentials") {
      if (!s.access_token) return false;
      if (s.expires_at && Date.now() > s.expires_at) return false;
      return true;
    }
    return false;
  }

  function authHeaderFor(schemeId) {
    var s = authState.schemes[schemeId];
    if (!s) return null;
    if (s.kind === "bearer") return s.token ? "Bearer " + s.token : null;
    if (s.kind === "oauth2-client-credentials") {
      return s.access_token ? "Bearer " + s.access_token : null;
    }
    return null;
  }

  function setBearerToken(schemeId, token) {
    if (token) {
      authState.schemes[schemeId] = { kind: "bearer", token: token };
    } else {
      delete authState.schemes[schemeId];
    }
    saveAuth(authState);
    refreshAllTryItAuthIndicators();
  }

  function setOAuthToken(schemeId, accessToken, expiresIn) {
    var expiresAt = expiresIn ? Date.now() + Math.max(0, expiresIn - 30) * 1000 : null;
    authState.schemes[schemeId] = {
      kind: "oauth2-client-credentials",
      access_token: accessToken,
      expires_at: expiresAt,
    };
    saveAuth(authState);
    refreshAllTryItAuthIndicators();
  }

  function clearScheme(schemeId) {
    delete authState.schemes[schemeId];
    saveAuth(authState);
    refreshAllTryItAuthIndicators();
  }

  function initAuthForms() {
    var forms = document.querySelectorAll("[data-auth-form]");
    Array.prototype.forEach.call(forms, function (form) {
      var schemeId = form.dataset.schemeId;
      var status = form.querySelector("[data-auth-status]");
      var kind = form.dataset.authKind;

      // Rehydrate existing token state into the form display.
      if (kind === "bearer") {
        var existing = authState.schemes[schemeId];
        if (existing && existing.token) {
          status.textContent = "Token saved.";
        }
        var input = form.querySelector("[data-auth-bearer-token]");
        var save = form.querySelector("[data-auth-bearer-save]");
        save.addEventListener("click", function () {
          setBearerToken(schemeId, input.value.trim());
          status.textContent = input.value.trim() ? "Token saved." : "Cleared.";
        });
      } else if (kind === "oauth2-client-credentials") {
        var existingOauth = authState.schemes[schemeId];
        if (existingOauth && existingOauth.access_token) {
          status.textContent = existingOauth.expires_at
            ? "Token cached. Expires " + new Date(existingOauth.expires_at).toLocaleTimeString() + "."
            : "Token cached.";
        }
        var clientId = form.querySelector("[data-auth-client-id]");
        var clientSecret = form.querySelector("[data-auth-client-secret]");
        var scopeBoxes = form.querySelectorAll("[data-auth-scope]");
        var requestBtn = form.querySelector("[data-auth-oauth-request]");
        var tokenUrl = form.dataset.tokenUrl;
        requestBtn.addEventListener("click", function () {
          requestClientCredentialsToken(
            schemeId,
            tokenUrl,
            clientId.value.trim(),
            clientSecret.value,
            Array.prototype.filter.call(scopeBoxes, function (b) { return b.checked; })
              .map(function (b) { return b.value; }),
            status
          );
        });
      }

      var clearBtn = form.querySelector("[data-auth-clear]");
      if (clearBtn) {
        clearBtn.addEventListener("click", function () {
          clearScheme(schemeId);
          status.textContent = "Cleared.";
          // Also clear the inputs visually.
          var inputs = form.querySelectorAll("input");
          Array.prototype.forEach.call(inputs, function (i) {
            if (i.type === "checkbox") i.checked = false;
            else i.value = "";
          });
        });
      }
    });
  }

  function requestClientCredentialsToken(schemeId, tokenUrl, clientId, clientSecret, scopes, statusEl) {
    if (!tokenUrl || !clientId || !clientSecret) {
      statusEl.textContent = "Missing client id / secret / token URL.";
      return;
    }
    statusEl.textContent = "requesting…";
    var body = "grant_type=client_credentials";
    if (scopes.length) body += "&scope=" + encodeURIComponent(scopes.join(" "));
    var headers = {
      "Content-Type": "application/x-www-form-urlencoded",
      "Authorization": "Basic " + btoa(clientId + ":" + clientSecret),
    };
    fetch(tokenUrl, { method: "POST", headers: headers, body: body, credentials: "omit" })
      .then(function (resp) {
        return resp.text().then(function (text) {
          var data;
          try { data = JSON.parse(text); } catch (e) { data = null; }
          if (!resp.ok) {
            statusEl.textContent =
              "HTTP " + resp.status +
              ((data && data.error) ? " — " + data.error : "") +
              ((data && data.error_description) ? ": " + data.error_description : "");
            return;
          }
          if (!data || !data.access_token) {
            statusEl.textContent = "OK but no access_token in response.";
            return;
          }
          setOAuthToken(schemeId, data.access_token, data.expires_in || null);
          statusEl.textContent = "Token cached" +
            (data.expires_in ? " — expires in " + data.expires_in + "s." : ".");
        });
      })
      .catch(function (err) {
        statusEl.textContent = (err && err.message) ? err.message : "network / CORS error";
      });
  }

  // ---- try-it request builder ----

  function highlightJsonClient(text) {
    // Best-effort: re-parse and pretty-print. If it isn't JSON we
    // return the text unwrapped (the browser will still render).
    try {
      var parsed = JSON.parse(text);
      return JSON.stringify(parsed, null, 2);
    } catch (e) {
      return text;
    }
  }

  function buildUrl(form) {
    var template = form.dataset.pathTemplate;
    var path = template.replace(/\{([^{}]+)\}/g, function (m, name) {
      var input = form.querySelector('[data-tryit-param][data-name="' + cssEscape(name) + '"][data-location="path"]');
      var v = input ? input.value : "";
      return encodeURIComponent(v);
    });
    var query = [];
    var queryInputs = form.querySelectorAll('[data-tryit-param][data-location="query"]');
    Array.prototype.forEach.call(queryInputs, function (i) {
      if (i.value !== "") query.push(encodeURIComponent(i.dataset.name) + "=" + encodeURIComponent(i.value));
    });
    var base = effectiveServerUrl() || "";
    var url = base.replace(/\/+$/, "") + path;
    if (query.length) url += (url.indexOf("?") === -1 ? "?" : "&") + query.join("&");
    return url;
  }

  function collectHeaders(form) {
    var h = {};
    var inputs = form.querySelectorAll('[data-tryit-param][data-location="header"]');
    Array.prototype.forEach.call(inputs, function (i) {
      if (i.value !== "") h[i.dataset.name] = i.value;
    });
    // Auth: if the op declares security, attach Authorization from
    // the first satisfied scheme. Param-supplied Authorization always
    // wins, so users can manually override.
    if (!h["Authorization"]) {
      var pills = form.querySelectorAll("[data-required-scheme]");
      for (var i = 0; i < pills.length; i++) {
        var schemeId = pills[i].dataset.requiredScheme;
        var header = authHeaderFor(schemeId);
        if (header) { h["Authorization"] = header; break; }
      }
    }
    return h;
  }

  /// Update the green/red pill next to each declared scheme on a
  /// try-it form. Called after any auth state change.
  function refreshAllTryItAuthIndicators() {
    var pills = document.querySelectorAll("[data-required-scheme]");
    Array.prototype.forEach.call(pills, function (pill) {
      var ok = schemeSatisfied(pill.dataset.requiredScheme);
      pill.classList.toggle("auth-pill--ok", ok);
      pill.classList.toggle("auth-pill--missing", !ok);
      var state = pill.querySelector("[data-auth-state]");
      if (state) state.textContent = ok ? "✓ ready" : "✗ missing";
    });
  }

  function statusClass(status) {
    if (status >= 200 && status < 300) return "status-2xx";
    if (status >= 300 && status < 400) return "status-3xx";
    if (status >= 400 && status < 500) return "status-4xx";
    if (status >= 500 && status < 600) return "status-5xx";
    if (status === 0) return "status-default";
    return "status-1xx";
  }

  function cssEscape(s) {
    if (window.CSS && window.CSS.escape) return window.CSS.escape(s);
    // Minimal fallback: only escape what we'd actually see in a
    // parameter name. OAS allows `[A-Za-z0-9_\-\.]`, none of which
    // collide with CSS selector syntax.
    return s.replace(/[^A-Za-z0-9_\-]/g, "_");
  }

  function refreshTryItEffective(form) {
    var out = form.querySelector("[data-tryit-effective-url]");
    if (!out) return;
    try {
      out.textContent = buildUrl(form);
    } catch (e) {
      out.textContent = "";
    }
  }

  function sendTryIt(form) {
    var btn = form.querySelector("[data-tryit-send]");
    var statusOut = form.querySelector("[data-tryit-status]");
    var responseBlock = form.querySelector("[data-tryit-response]");
    var statusBadge = form.querySelector("[data-tryit-response-status]");
    var statusText = form.querySelector("[data-tryit-response-status-text]");
    var durationOut = form.querySelector("[data-tryit-response-duration]");
    var headersDl = form.querySelector("[data-tryit-response-headers]");
    var bodyEl = form.querySelector("[data-tryit-response-body]");

    var url = buildUrl(form);
    var method = form.dataset.method;
    var headers = collectHeaders(form);
    var bodyTextarea = form.querySelector("[data-tryit-body]");
    var contentTypeSel = form.querySelector("[data-tryit-content-type]");
    var body = null;
    if (bodyTextarea && bodyTextarea.value !== "" && method !== "GET" && method !== "HEAD") {
      body = bodyTextarea.value;
      if (contentTypeSel && !headers["Content-Type"]) {
        headers["Content-Type"] = contentTypeSel.value;
      }
    }

    btn.disabled = true;
    statusOut.textContent = "sending…";
    var started = performance.now();
    fetch(url, { method: method, headers: headers, body: body, credentials: "omit" })
      .then(function (resp) {
        var duration = Math.round(performance.now() - started);
        statusOut.textContent = "";
        durationOut.textContent = duration + " ms";
        statusBadge.textContent = resp.status;
        statusBadge.className = "status-badge " + statusClass(resp.status);
        statusText.textContent = resp.statusText || "";
        headersDl.innerHTML = "";
        resp.headers.forEach(function (value, key) {
          var dt = document.createElement("dt");
          var dd = document.createElement("dd");
          var code = document.createElement("code");
          code.textContent = key;
          dt.appendChild(code);
          dd.textContent = value;
          headersDl.appendChild(dt);
          headersDl.appendChild(dd);
        });
        return resp.text().then(function (text) {
          bodyEl.textContent = highlightJsonClient(text);
        });
      })
      .catch(function (err) {
        statusOut.textContent = "";
        durationOut.textContent = "";
        statusBadge.textContent = "—";
        statusBadge.className = "status-badge status-default";
        statusText.textContent = (err && err.message) ? err.message : "request failed (network / CORS)";
        headersDl.innerHTML = "";
        bodyEl.textContent = "";
      })
      .then(function () {
        btn.disabled = false;
        responseBlock.hidden = false;
      });
  }

  function initTryIt() {
    var forms = document.querySelectorAll("[data-tryit-form]");
    Array.prototype.forEach.call(forms, function (form) {
      // Keep the effective URL preview in sync as inputs / server / vars change.
      refreshTryItEffective(form);
      form.addEventListener("input", function () { refreshTryItEffective(form); });
      window.openapiForge.on("serverchange", function () { refreshTryItEffective(form); });
      var btn = form.querySelector("[data-tryit-send]");
      if (btn) btn.addEventListener("click", function () { sendTryIt(form); });
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
    initCopyButtons();
    initAuthForms();
    initTryIt();
    refreshAllTryItAuthIndicators();
  });
})();

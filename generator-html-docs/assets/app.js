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
  var PKCE_PENDING_KEY = "openapi-forge-html-docs:pkce-pending:v1";

  // ---- baked-in oauth client config (per scheme id) ----

  function readOauthClientConfig() {
    var meta = document.querySelector('meta[name="openapi-forge-oauth-clients"]');
    if (!meta) return {};
    try { return JSON.parse(meta.content || "{}"); } catch (e) { return {}; }
  }
  function readCallbackPath() {
    var meta = document.querySelector('meta[name="openapi-forge-callback-path"]');
    return meta ? meta.content : "auth/callback.html";
  }
  var OAUTH_CLIENTS = readOauthClientConfig();

  function resolvedRedirectUri(schemeId) {
    var cfg = OAUTH_CLIENTS[schemeId] || {};
    if (cfg.redirectUri) return cfg.redirectUri;
    // The meta tag gives us a relative path from the current page.
    // Resolve it against the page origin so it becomes the absolute
    // URL the IdP redirects to.
    return new URL(readCallbackPath(), window.location.href).toString();
  }

  // ---- state ----

  // `serverUrl` of this sentinel means "use the user-supplied
  // custom URL instead of any declared server".
  var CUSTOM_SERVER = "__custom";

  function loadState() {
    var s = { theme: null, serverUrl: null, customServerUrl: "", variables: {} };
    try {
      var raw = localStorage.getItem(STORAGE_KEY);
      if (raw) {
        var parsed = JSON.parse(raw);
        if (parsed && typeof parsed === "object") {
          s.theme = parsed.theme || null;
          s.serverUrl = parsed.serverUrl || null;
          s.customServerUrl = parsed.customServerUrl || "";
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
    if (state.serverUrl === CUSTOM_SERVER) {
      return (state.customServerUrl || "").trim() || null;
    }
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

  function setCustomServerUrl(url) {
    state.customServerUrl = url || "";
    saveState(state);
    if (state.serverUrl === CUSTOM_SERVER) {
      refreshHeaderEffective();
      emit("serverchange", {
        url: CUSTOM_SERVER,
        effective: effectiveServerUrl(),
      });
    }
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
    var customInput = document.querySelector("[data-server-custom-url]");

    // First page load — adopt the stored server, else the first option.
    var options = Array.prototype.map.call(sel.options, function (o) { return o.value; });
    if (state.serverUrl && options.indexOf(state.serverUrl) !== -1) {
      sel.value = state.serverUrl;
    } else {
      state.serverUrl = sel.value;
      saveState(state);
    }

    function applyCustomVisibility() {
      if (!customInput) return;
      var isCustom = sel.value === CUSTOM_SERVER;
      customInput.hidden = !isCustom;
      if (isCustom) customInput.value = state.customServerUrl || "";
    }
    applyCustomVisibility();

    sel.addEventListener("change", function () {
      setServer(sel.value);
      applyCustomVisibility();
      if (sel.value === CUSTOM_SERVER && customInput) customInput.focus();
    });
    if (customInput) {
      customInput.addEventListener("input", function () {
        setCustomServerUrl(customInput.value);
      });
    }
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
    if (s.kind === "oauth2-client-credentials" || s.kind === "oauth2-authorization-code") {
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
    if (s.kind === "oauth2-client-credentials" || s.kind === "oauth2-authorization-code") {
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

  // ---- PKCE primitives ----

  function randomString(bytes) {
    var arr = new Uint8Array(bytes);
    crypto.getRandomValues(arr);
    return base64url(arr);
  }
  function base64url(bytes) {
    var s = "";
    for (var i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  function sha256base64url(text) {
    return crypto.subtle
      .digest("SHA-256", new TextEncoder().encode(text))
      .then(function (buf) { return base64url(new Uint8Array(buf)); });
  }

  function loadPkcePending() {
    try {
      var raw = sessionStorage.getItem(PKCE_PENDING_KEY);
      return raw ? JSON.parse(raw) : {};
    } catch (e) { return {}; }
  }
  function savePkcePending(p) {
    try { sessionStorage.setItem(PKCE_PENDING_KEY, JSON.stringify(p)); } catch (e) {}
  }

  function beginPkceLogin(form, schemeId, statusEl) {
    var cfg = OAUTH_CLIENTS[schemeId] || {};
    if (!cfg.clientId) {
      statusEl.textContent =
        "No client_id configured for `" + schemeId + "`. Set `oauth." + schemeId +
        ".clientId` in the generator config.";
      return;
    }
    var authUrl = form.dataset.authorizationUrl;
    var tokenUrl = form.dataset.tokenUrl;
    var redirectUri = resolvedRedirectUri(schemeId);

    // Scopes: union of the user's checkbox picks and the config
    // defaults. Falling back to whatever the flow declared is the
    // template's job (it pre-emits checkboxes).
    var scopeBoxes = form.querySelectorAll("[data-auth-scope]");
    var checked = Array.prototype.filter.call(scopeBoxes, function (b) { return b.checked; })
      .map(function (b) { return b.value; });
    var scopes = checked.length ? checked : (cfg.scopes || []);

    var state = randomString(16);
    var verifier = randomString(48);
    sha256base64url(verifier).then(function (challenge) {
      var pending = loadPkcePending();
      pending[state] = {
        schemeId: schemeId,
        verifier: verifier,
        tokenUrl: tokenUrl,
        redirectUri: redirectUri,
        scopes: scopes,
        startedAt: Date.now(),
      };
      savePkcePending(pending);

      var qs = new URLSearchParams({
        response_type: "code",
        client_id: cfg.clientId,
        redirect_uri: redirectUri,
        state: state,
        code_challenge: challenge,
        code_challenge_method: "S256",
      });
      if (scopes.length) qs.set("scope", scopes.join(" "));
      var separator = authUrl.indexOf("?") === -1 ? "?" : "&";
      var url = authUrl + separator + qs.toString();
      statusEl.textContent = "Opening IdP popup…";
      var popup = window.open(url, "openapi-forge-auth", "popup,width=540,height=720");
      if (!popup) {
        statusEl.textContent =
          "Popup blocked. Allow popups for this site and try again.";
        return;
      }
    });
  }

  function exchangeAuthCode(schemeId, pending, code, statusEl) {
    var cfg = OAUTH_CLIENTS[schemeId] || {};
    var body = new URLSearchParams({
      grant_type: "authorization_code",
      code: code,
      redirect_uri: pending.redirectUri,
      client_id: cfg.clientId,
      code_verifier: pending.verifier,
    });
    var headers = { "Content-Type": "application/x-www-form-urlencoded" };
    // Confidential clients: include the secret via Basic auth — that's
    // what Keycloak prefers for confidential clients.
    if (cfg.clientSecret) {
      headers["Authorization"] = "Basic " + btoa(cfg.clientId + ":" + cfg.clientSecret);
    }
    return fetch(pending.tokenUrl, {
      method: "POST",
      headers: headers,
      body: body.toString(),
      credentials: "omit",
    }).then(function (resp) {
      return resp.text().then(function (text) {
        var data;
        try { data = JSON.parse(text); } catch (e) { data = null; }
        if (!resp.ok) {
          var msg = "HTTP " + resp.status;
          if (data && data.error) msg += " — " + data.error;
          if (data && data.error_description) msg += ": " + data.error_description;
          throw new Error(msg);
        }
        if (!data || !data.access_token) throw new Error("token response missing access_token");
        setOAuthAuthCodeToken(schemeId, data.access_token, data.expires_in || null, data.refresh_token || null);
        if (statusEl) {
          statusEl.textContent = "Signed in" +
            (data.expires_in ? " — token expires in " + data.expires_in + "s." : ".");
        }
      });
    });
  }

  function setOAuthAuthCodeToken(schemeId, accessToken, expiresIn, refreshToken) {
    var expiresAt = expiresIn ? Date.now() + Math.max(0, expiresIn - 30) * 1000 : null;
    authState.schemes[schemeId] = {
      kind: "oauth2-authorization-code",
      access_token: accessToken,
      expires_at: expiresAt,
      refresh_token: refreshToken,
      // Per-audience exchanged tokens cache:
      exchanged: {},
    };
    saveAuth(authState);
    refreshAllTryItAuthIndicators();
  }

  function initPkceCallbackListener() {
    window.addEventListener("message", function (ev) {
      if (ev.origin !== window.location.origin) return;
      var data = ev.data;
      if (!data || data.source !== "openapi-forge-auth-callback") return;
      var pending = loadPkcePending();
      var entry = pending[data.state];
      if (!entry) return;
      delete pending[data.state];
      savePkcePending(pending);
      // Find the matching auth form by scheme id so we can report
      // status into its UI; gracefully fall back to console if the
      // form isn't on this page.
      var form = document.querySelector('[data-auth-form][data-auth-kind="oauth2-authorization-code"][data-scheme-id="' + cssEscape(entry.schemeId) + '"]');
      var statusEl = form && form.querySelector("[data-auth-status]");
      if (!data.ok) {
        var msg = "Login failed: " + (data.error || "unknown") +
          (data.error_description ? " — " + data.error_description : "");
        if (statusEl) statusEl.textContent = msg;
        else console.error("[openapi-forge] " + msg);
        return;
      }
      if (statusEl) statusEl.textContent = "Got code — exchanging for token…";
      exchangeAuthCode(entry.schemeId, entry, data.code, statusEl).catch(function (err) {
        if (statusEl) statusEl.textContent = err.message;
        else console.error("[openapi-forge] " + err.message);
      });
    });
  }

  function initPkceForms() {
    var forms = document.querySelectorAll('[data-auth-form][data-auth-kind="oauth2-authorization-code"]');
    Array.prototype.forEach.call(forms, function (form) {
      var schemeId = form.dataset.schemeId;
      var status = form.querySelector("[data-auth-status]");
      var loginBtn = form.querySelector("[data-auth-pkce-login]");
      var redirectSlot = form.querySelector("[data-auth-pkce-redirect-uri]");
      if (redirectSlot) redirectSlot.textContent = resolvedRedirectUri(schemeId);
      var existing = authState.schemes[schemeId];
      if (existing && existing.access_token) {
        status.textContent = existing.expires_at
          ? "Signed in. Token expires " + new Date(existing.expires_at).toLocaleTimeString() + "."
          : "Signed in.";
      }
      if (loginBtn) {
        loginBtn.addEventListener("click", function () { beginPkceLogin(form, schemeId, status); });
      }
    });
  }

  // ---- RFC 8693 token exchange ----

  function tokenExchangeKey(audience, scope) {
    return audience + "::" + (scope || "");
  }

  /// Get (or fetch + cache) an audience-scoped token for `schemeId`
  /// using RFC 8693 subject-token exchange. The cache lives on the
  /// scheme's auth-state record keyed by audience + scope.
  function getExchangedToken(schemeId, audience, scopes) {
    var s = authState.schemes[schemeId];
    if (!s || !s.access_token) return Promise.reject(new Error("not signed in"));
    s.exchanged = s.exchanged || {};
    var scopeStr = (scopes || []).join(" ");
    var key = tokenExchangeKey(audience, scopeStr);
    var cached = s.exchanged[key];
    if (cached && (!cached.expires_at || Date.now() < cached.expires_at)) {
      return Promise.resolve(cached.access_token);
    }

    var form = document.querySelector('[data-auth-form][data-auth-kind="oauth2-authorization-code"][data-scheme-id="' + cssEscape(schemeId) + '"]');
    var tokenUrl = form && form.dataset.tokenUrl;
    if (!tokenUrl) return Promise.reject(new Error("scheme " + schemeId + " has no token URL on this page"));
    var cfg = OAUTH_CLIENTS[schemeId] || {};
    var body = new URLSearchParams({
      grant_type: "urn:ietf:params:oauth:grant-type:token-exchange",
      subject_token: s.access_token,
      subject_token_type: "urn:ietf:params:oauth:token-type:access_token",
      requested_token_type: "urn:ietf:params:oauth:token-type:access_token",
      audience: audience,
    });
    if (scopeStr) body.set("scope", scopeStr);
    var headers = { "Content-Type": "application/x-www-form-urlencoded" };
    if (cfg.clientSecret) {
      headers["Authorization"] = "Basic " + btoa(cfg.clientId + ":" + cfg.clientSecret);
    } else {
      body.set("client_id", cfg.clientId);
    }
    return fetch(tokenUrl, {
      method: "POST",
      headers: headers,
      body: body.toString(),
      credentials: "omit",
    }).then(function (resp) {
      return resp.text().then(function (text) {
        var data;
        try { data = JSON.parse(text); } catch (e) { data = null; }
        if (!resp.ok) {
          var msg = "token-exchange HTTP " + resp.status;
          if (data && data.error) msg += " — " + data.error;
          if (data && data.error_description) msg += ": " + data.error_description;
          throw new Error(msg);
        }
        if (!data || !data.access_token) throw new Error("exchange response missing access_token");
        var expiresAt = data.expires_in ? Date.now() + Math.max(0, data.expires_in - 30) * 1000 : null;
        s.exchanged[key] = { access_token: data.access_token, expires_at: expiresAt };
        saveAuth(authState);
        return data.access_token;
      });
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
    return h;
  }

  /// Returns a Promise that resolves with the Authorization value to
  /// attach (or null when there's nothing to attach). Handles the
  /// RFC 8693 token-exchange case asynchronously by swapping the
  /// signed-in subject token for an audience-scoped token whose
  /// audience is derived from the operation's path-param input.
  function resolveAuthHeader(form) {
    // Manual override always wins. Caller checks before us.
    var txSchemeId = form.dataset.txSchemeId;
    if (txSchemeId && schemeSatisfied(txSchemeId)) {
      var placeholder = form.dataset.txPlaceholder;
      var template = form.dataset.txAudienceTemplate;
      var extra = (form.dataset.txExtraScope || "").trim().split(/\s+/).filter(Boolean);
      var pathInput = form.querySelector('[data-tryit-param][data-location="path"][data-name="' + cssEscape(placeholder) + '"]');
      if (!pathInput || !pathInput.value) {
        // Without a path-param value we can't substitute; fall back
        // to the bare subject token (caller's headers gets the raw
        // one below).
      } else {
        var audience = template.replace("{" + placeholder + "}", pathInput.value);
        return getExchangedToken(txSchemeId, audience, extra).then(function (tok) {
          return "Bearer " + tok;
        });
      }
    }
    // Default: first satisfied declared scheme on the op.
    var pills = form.querySelectorAll("[data-required-scheme]");
    for (var i = 0; i < pills.length; i++) {
      var schemeId = pills[i].dataset.requiredScheme;
      var header = authHeaderFor(schemeId);
      if (header) return Promise.resolve(header);
    }
    return Promise.resolve(null);
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
    // Resolve Authorization async (token-exchange when applicable),
    // user-supplied `Authorization` header always wins.
    var authPromise = headers["Authorization"]
      ? Promise.resolve(null)
      : resolveAuthHeader(form);
    authPromise
      .then(function (auth) {
        if (auth && !headers["Authorization"]) headers["Authorization"] = auth;
        return fetch(url, { method: method, headers: headers, body: body, credentials: "omit" });
      })
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
    setCustomServerUrl: setCustomServerUrl,
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
    initPkceForms();
    initPkceCallbackListener();
    initTryIt();
    refreshAllTryItAuthIndicators();
  });
})();

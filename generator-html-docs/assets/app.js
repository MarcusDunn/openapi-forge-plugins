// generator-html-docs runtime: theme toggle + sidebar collapse memory.
// Site is fully usable with JS disabled.
(function () {
  "use strict";

  var STORAGE_THEME = "openapi-forge-html-docs:theme";

  function applyTheme(theme) {
    document.documentElement.setAttribute("data-theme", theme);
    var btn = document.querySelector("[data-theme-toggle]");
    if (btn) {
      btn.setAttribute("aria-pressed", theme === "dark" ? "true" : "false");
    }
  }

  function nextTheme(current) {
    if (current === "dark") return "light";
    if (current === "light") return "auto";
    return "dark";
  }

  try {
    var saved = localStorage.getItem(STORAGE_THEME);
    if (saved) applyTheme(saved);
  } catch (e) {}

  document.addEventListener("DOMContentLoaded", function () {
    var btn = document.querySelector("[data-theme-toggle]");
    if (btn) {
      btn.addEventListener("click", function () {
        var current = document.documentElement.getAttribute("data-theme") || "auto";
        var next = nextTheme(current);
        applyTheme(next);
        try { localStorage.setItem(STORAGE_THEME, next); } catch (e) {}
      });
    }

    // Highlight the path from each "current page" entry up through its
    // ancestor sidebar nodes. Keeps the current location obvious even
    // in deep trees.
    var current = document.querySelector(".nav-tree a[aria-current=page]");
    var li = current && current.closest("li");
    while (li) {
      li.dataset.onPath = "true";
      li = li.parentElement && li.parentElement.closest("li");
    }
  });
})();

// The menu scrolls to where the reader is.
//
// The section being read is open when the page arrives — the layout writes that
// into the HTML — but with sixty decision records in one section, the entry for
// the page you are on can be a long way down a column that scrolls on its own.
// The browser will not scroll it, because nothing on the page is focused there.
//
// This moves the menu's own scroll and nothing else. `scrollIntoView` would
// also move the window, which would start every page part-way down.

// The header's links follow the same rule as the sidebar: shipped open, and
// closed where they are a menu button rather than a row (the width the
// stylesheet's header query names). A link followed inside the panel closes
// it, so an anchor on the same page is not hidden behind it.
(function () {
  "use strict";

  var menu = document.querySelector(".site-header-menu");
  if (!menu) return;

  var narrow = window.matchMedia("(max-width: 1099px)");

  function fit(query) {
    menu.open = !query.matches;
  }

  fit(narrow);
  if (narrow.addEventListener) narrow.addEventListener("change", fit);

  menu.addEventListener("click", function (event) {
    if (narrow.matches && event.target.closest("a")) menu.open = false;
  });
})();

(function () {
  "use strict";

  var nav = document.querySelector(".site-nav");
  if (!nav) return;

  // The menu ships open, because a reader with no script must get a menu and
  // not a line that will not expand. Where it *is* a control — the widths at
  // which it is one line above the text rather than a column beside it — it
  // starts closed, and follows the window if that window is resized across the
  // boundary the stylesheet uses.
  var outer = nav.querySelector(".site-nav-outer");
  var narrow = window.matchMedia("(max-width: 899px)");

  function fit(query) {
    if (outer) outer.open = !query.matches;
  }

  if (outer) {
    fit(narrow);
    if (narrow.addEventListener) narrow.addEventListener("change", fit);
  }

  var here = nav.querySelector('a[aria-current="page"]');
  if (!here) return;

  var overflow = nav.scrollHeight - nav.clientHeight;
  if (overflow <= 0) return;

  var wanted = here.offsetTop - nav.clientHeight / 2;
  nav.scrollTop = Math.max(0, Math.min(wanted, overflow));
})();

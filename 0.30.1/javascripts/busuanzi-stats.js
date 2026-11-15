/*
 * Busuanzi (不蒜子) visit statistics for the docs site.
 *
 * MkDocs Material's instant navigation swaps page content via XHR instead of
 * a full reload, so a plain <script src="...busuanzi..."> would count only
 * the first page load of a session. Re-injecting the counter script on each
 * `document$` emission (Material's navigation observable) keeps per-page
 * views counted; the initial emission covers the first load as well.
 */
(function () {
  var SCRIPT_ID = "busuanzi-script";
  var SRC = "https://busuanzi.ibruce.info/busuanzi/2.3/busuanzi.pure.mini.js";

  function loadBusuanzi() {
    var previous = document.getElementById(SCRIPT_ID);
    if (previous) {
      previous.remove();
    }
    var script = document.createElement("script");
    script.id = SCRIPT_ID;
    script.async = true;
    script.src = SRC;
    document.body.appendChild(script);
  }

  if (typeof document$ !== "undefined" && document$.subscribe) {
    document$.subscribe(loadBusuanzi);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", loadBusuanzi);
  } else {
    loadBusuanzi();
  }
})();

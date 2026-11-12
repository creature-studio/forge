/* Forge landing page — progressive enhancement only. */
(function () {
  "use strict";

  var doc = document.documentElement;
  var header = document.getElementById("site-header");
  var navToggle = document.querySelector(".nav-toggle");
  var nav = document.getElementById("site-nav");
  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* Header state ---------------------------------------------------------- */
  function updateHeader() {
    if (!header) return;
    header.classList.toggle("is-scrolled", window.scrollY > 8);
  }

  updateHeader();
  window.addEventListener("scroll", updateHeader, { passive: true });

  /* Mobile navigation ----------------------------------------------------- */
  function closeNav() {
    if (!header || !navToggle) return;
    header.classList.remove("nav-open");
    navToggle.setAttribute("aria-expanded", "false");
    navToggle.setAttribute("aria-label", "打开导航菜单");
    document.body.style.removeProperty("overflow");
  }

  if (navToggle && nav) {
    navToggle.addEventListener("click", function () {
      var open = header.classList.toggle("nav-open");
      navToggle.setAttribute("aria-expanded", String(open));
      navToggle.setAttribute("aria-label", open ? "关闭导航菜单" : "打开导航菜单");
      document.body.style.overflow = open ? "hidden" : "";
    });

    nav.addEventListener("click", function (event) {
      if (event.target.closest("a")) closeNav();
    });

    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape") closeNav();
    });

    window.addEventListener("resize", function () {
      if (window.innerWidth > 980) closeNav();
    });
  }

  /* Scroll reveal --------------------------------------------------------- */
  var revealItems = Array.prototype.slice.call(document.querySelectorAll("[data-reveal]"));

  function showAll() {
    revealItems.forEach(function (item) {
      item.classList.add("is-visible");
    });
  }

  try {
    if (
      reduceMotion ||
      !("IntersectionObserver" in window) ||
      new URLSearchParams(window.location.search).has("no-reveal")
    ) {
      showAll();
    } else {
      doc.classList.add("reveal-ready");

      var observer = new IntersectionObserver(
        function (entries) {
          entries.forEach(function (entry) {
            if (entry.isIntersecting) {
              entry.target.classList.add("is-visible");
              observer.unobserve(entry.target);
            }
          });
        },
        { rootMargin: "0px 0px -8% 0px", threshold: 0.08 }
      );

      revealItems.forEach(function (item) {
        observer.observe(item);
      });
    }
  } catch (error) {
    doc.classList.remove("reveal-ready");
    showAll();
  }

  /* Copy install command -------------------------------------------------- */
  var copyButton = document.querySelector("[data-copy-target]");
  var copyStatus = document.querySelector(".copy-status");

  function legacyCopy(text) {
    var area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-1000px";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    var copied = false;
    try {
      copied = document.execCommand("copy");
    } catch (error) {
      copied = false;
    }
    document.body.removeChild(area);
    return copied;
  }

  if (copyButton) {
    copyButton.addEventListener("click", function () {
      var target = document.getElementById(copyButton.getAttribute("data-copy-target"));
      if (!target) return;

      var text = target.textContent.replace(/\u00a0/g, " ").trim();
      var done = function (ok) {
        if (!ok) {
          try {
            var range = document.createRange();
            range.selectNodeContents(target);
            var selection = window.getSelection();
            selection.removeAllRanges();
            selection.addRange(range);
          } catch (error) {}
          if (copyStatus) copyStatus.textContent = "已选中命令，按 Ctrl+C 复制。";
          return;
        }
        copyButton.textContent = "已复制";
        copyButton.classList.add("copied");
        if (copyStatus) copyStatus.textContent = "命令已复制到剪贴板。";
        window.setTimeout(function () {
          copyButton.textContent = "复制";
          copyButton.classList.remove("copied");
          if (copyStatus) copyStatus.textContent = "";
        }, 2400);
      };

      if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).then(
          function () {
            done(true);
          },
          function () {
            done(legacyCopy(text));
          }
        );
      } else {
        done(legacyCopy(text));
      }
    });
  }
})();

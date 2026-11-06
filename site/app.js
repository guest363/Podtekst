(function () {
  "use strict";

  var header = document.querySelector(".site-header");
  var menu = document.getElementById("mobile-menu");
  var menuToggle = document.querySelector(".menu-toggle");
  var lastFocused = null;
  var closeTimer = 0;

  function onScroll() {
    if (header) {
      header.classList.toggle("is-scrolled", window.scrollY > 8);
    }
  }

  window.addEventListener("scroll", onScroll, { passive: true });
  onScroll();

  function detectOS() {
    var ua = navigator.userAgent || "";
    if (/Android|iPhone|iPad|iPod|Mobile/i.test(ua)) {
      return "компьютера";
    }
    if (/Win/i.test(ua)) {
      return "Windows";
    }
    if (/Mac/i.test(ua)) {
      return "macOS";
    }
    if (/Linux/i.test(ua)) {
      return "Linux";
    }
    return "";
  }

  var os = detectOS();
  if (os) {
    var labels = document.querySelectorAll("[data-download]");
    for (var i = 0; i < labels.length; i += 1) {
      labels[i].textContent = "Скачать для " + os;
    }
  }

  function openMenu() {
    if (!menu || !menuToggle) return;
    window.clearTimeout(closeTimer);
    lastFocused = document.activeElement;
    menu.hidden = false;
    menu.offsetHeight;
    menu.classList.add("is-open");
    menuToggle.setAttribute("aria-expanded", "true");
    menuToggle.setAttribute("aria-label", "Закрыть меню");
    document.body.classList.add("menu-open");
    var firstLink = menu.querySelector("a");
    if (firstLink) firstLink.focus();
  }

  function closeMenu() {
    if (!menu || !menuToggle) return;
    menu.classList.remove("is-open");
    menuToggle.setAttribute("aria-expanded", "false");
    menuToggle.setAttribute("aria-label", "Открыть меню");
    document.body.classList.remove("menu-open");
    closeTimer = window.setTimeout(function () {
      menu.hidden = true;
    }, 220);
    if (lastFocused && typeof lastFocused.focus === "function") {
      lastFocused.focus();
    }
  }

  if (menuToggle) {
    menuToggle.addEventListener("click", function () {
      if (menuToggle.getAttribute("aria-expanded") === "true") {
        closeMenu();
      } else {
        openMenu();
      }
    });
  }

  if (menu) {
    menu.addEventListener("click", function (event) {
      var link = event.target.closest("a");
      if (link) closeMenu();
    });

    document.addEventListener("keydown", function (event) {
      if (menu.hidden) return;

      if (event.key === "Escape") {
        event.preventDefault();
        closeMenu();
        return;
      }

      if (event.key !== "Tab") return;

      var focusable = menu.querySelectorAll("a, button");
      if (focusable.length === 0) return;
      var first = focusable[0];
      var last = focusable[focusable.length - 1];

      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    });
  }

})();

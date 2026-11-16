(function () {
  "use strict";

  var menu = document.getElementById("mobile-menu");
  var menuToggle = document.querySelector(".menu-toggle");
  var menuBackground = document.querySelectorAll(".site-header .brand, .site-header .nav, .site-header .header-cta, main, .site-footer");
  var lastFocused = null;
  var closeTimer = 0;

  function detectOS() {
    var ua = navigator.userAgent || "";
    // На телефоне CTA ведёт на APK: «Скачать для компьютера» там бессмысленно.
    if (/Android|iPhone|iPad|iPod|Mobile/i.test(ua)) {
      return "";
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

  function setBackgroundInert(value) {
    for (var i = 0; i < menuBackground.length; i += 1) {
      menuBackground[i].inert = value;
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
    setBackgroundInert(true);
    var firstLink = menu.querySelector("a");
    if (firstLink) firstLink.focus();
  }

  function closeMenu() {
    if (!menu || !menuToggle) return;
    setBackgroundInert(false);
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

  var demo = document.querySelector("[data-demo-video]");
  var demoToggle = document.querySelector("[data-demo-toggle]");
  var demoUserPaused = false;
  if (demo) {
    var demoMotionOk = !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    var demoPlay = function () { demo.play().catch(function () {}); };
    var demoPause = function () { demo.pause(); };
    var demoSync = function () {
      var paused = demo.paused;
      demoToggle.setAttribute("aria-pressed", paused ? "true" : "false");
      demoToggle.textContent = paused ? "Продолжить демо" : "Поставить на паузу";
    };
    if (demoMotionOk && typeof window.IntersectionObserver === "function") {
      var demoWatch = new IntersectionObserver(function (entries) {
        for (var i = 0; i < entries.length; i += 1) {
          if (entries[i].isIntersecting) {
            if (!demoUserPaused) demoPlay();
          } else {
            demoPause();
          }
        }
      });
      demoWatch.observe(demo);
      document.addEventListener("visibilitychange", function () {
        if (document.hidden) {
          demoPause();
        } else if (!demoUserPaused) {
          demoPlay();
        }
      });
    } else if (demoMotionOk) {
      demoPlay();
    }
    demo.addEventListener("play", demoSync);
    demo.addEventListener("pause", demoSync);
    demoSync();
    if (demoToggle) {
      demoToggle.addEventListener("click", function () {
        if (demo.paused) {
          demoUserPaused = false;
          demoPlay();
        } else {
          demoUserPaused = true;
          demoPause();
        }
      });
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

    var desktopMenu = window.matchMedia("(min-width: 941px)");
    var onDesktopChange = function (e) {
      if (e.matches && menuToggle.getAttribute("aria-expanded") === "true") {
        closeMenu();
      }
    };
    if (desktopMenu.addEventListener) {
      desktopMenu.addEventListener("change", onDesktopChange);
    } else if (desktopMenu.addListener) {
      desktopMenu.addListener(onDesktopChange);
    }

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

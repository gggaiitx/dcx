/* DCX 官网交互：主题、导航、目录、复制、代码页签、任务栏演示 */
(function () {
  "use strict";

  var doc = document;
  var root = doc.documentElement;
  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* ---------- 主题 ---------- */
  function systemTheme() {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }

  function readStoredTheme() {
    try {
      var v = localStorage.getItem("dcx-theme");
      return v === "light" || v === "dark" ? v : null;
    } catch (e) {
      return null;
    }
  }

  function applyTheme(theme, persist) {
    root.setAttribute("data-theme", theme);
    root.style.colorScheme = theme;
    if (persist) {
      try {
        localStorage.setItem("dcx-theme", theme);
      } catch (e) { /* 隐私模式下忽略 */ }
    }
    var btn = doc.querySelector("[data-theme-toggle]");
    if (btn) {
      var next = theme === "dark" ? "切换到浅色模式" : "切换到深色模式";
      btn.setAttribute("aria-label", next);
      btn.setAttribute("title", next);
      btn.setAttribute("aria-pressed", theme === "dark" ? "true" : "false");
    }
  }

  var queryTheme = new URLSearchParams(window.location.search).get("theme");
  var initialTheme =
    queryTheme === "light" || queryTheme === "dark"
      ? queryTheme
      : readStoredTheme() || root.getAttribute("data-theme") || systemTheme();
  applyTheme(initialTheme, false);

  doc.querySelectorAll("[data-theme-toggle]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var next = root.getAttribute("data-theme") === "dark" ? "light" : "dark";
      applyTheme(next, true);
    });
  });

  var mq = window.matchMedia("(prefers-color-scheme: dark)");
  var onSystemTheme = function () {
    if (!readStoredTheme()) applyTheme(systemTheme(), false);
  };
  if (mq.addEventListener) mq.addEventListener("change", onSystemTheme);
  else if (mq.addListener) mq.addListener(onSystemTheme);

  /* ---------- 头部滚动状态 ---------- */
  var header = doc.querySelector(".site-header");
  if (header) {
    var onScroll = function () {
      header.classList.toggle("is-scrolled", window.scrollY > 6);
    };
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
  }

  /* ---------- 阅读进度 ---------- */
  var progress = doc.querySelector("[data-progress]");
  if (progress) {
    var updateProgress = function () {
      var max = doc.documentElement.scrollHeight - window.innerHeight;
      var ratio = max > 0 ? Math.min(1, Math.max(0, window.scrollY / max)) : 0;
      progress.style.transform = "scaleX(" + ratio + ")";
    };
    updateProgress();
    window.addEventListener("scroll", updateProgress, { passive: true });
    window.addEventListener("resize", updateProgress);
  }

  /* ---------- 移动端导航 ---------- */
  var menuBtn = doc.querySelector("[data-menu-toggle]");
  var mobileNav = doc.querySelector("[data-mobile-nav]");
  if (menuBtn && mobileNav) {
    var setMenu = function (open) {
      mobileNav.classList.toggle("is-open", open);
      menuBtn.setAttribute("aria-expanded", open ? "true" : "false");
      menuBtn.setAttribute("aria-label", open ? "关闭导航" : "打开导航");
    };
    menuBtn.addEventListener("click", function () {
      setMenu(mobileNav.classList.contains("is-open") === false);
    });
    mobileNav.addEventListener("click", function (e) {
      if (e.target.closest("a")) setMenu(false);
    });
    doc.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && mobileNav.classList.contains("is-open")) setMenu(false);
    });
    window.addEventListener("resize", function () {
      if (window.innerWidth > 900) setMenu(false);
    });
  }

  /* ---------- 代码块：标题栏与复制 ---------- */
  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      var ta = doc.createElement("textarea");
      ta.value = text;
      ta.setAttribute("readonly", "");
      ta.style.position = "fixed";
      ta.style.top = "-1000px";
      ta.style.opacity = "0";
      doc.body.appendChild(ta);
      ta.select();
      try {
        var ok = doc.execCommand("copy");
        doc.body.removeChild(ta);
        ok ? resolve() : reject(new Error("copy failed"));
      } catch (err) {
        doc.body.removeChild(ta);
        reject(err);
      }
    });
  }

  doc.querySelectorAll(".code").forEach(function (box) {
    if (box.dataset.ready === "1") return;
    box.dataset.ready = "1";
    var pre = box.querySelector("pre");
    if (!pre) return;

    var top = doc.createElement("div");
    top.className = "code-top";

    var lang = doc.createElement("span");
    lang.className = "code-lang";
    lang.textContent = box.getAttribute("data-lang") || "text";
    top.appendChild(lang);

    var btn = doc.createElement("button");
    btn.type = "button";
    btn.className = "copy-btn";
    btn.setAttribute("aria-label", "复制代码");
    btn.innerHTML =
      '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M6 15H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v1"/></svg><span>复制</span>';
    top.appendChild(btn);
    box.insertBefore(top, pre);

    btn.addEventListener("click", function () {
      copyText(pre.innerText).then(
        function () {
          btn.classList.add("is-done");
          btn.querySelector("span").textContent = "已复制";
          setTimeout(function () {
            btn.classList.remove("is-done");
            btn.querySelector("span").textContent = "复制";
          }, 1600);
        },
        function () {
          // 剪贴板不可用时至少把代码选中，用户按 Ctrl+C 即可
          try {
            var range = doc.createRange();
            range.selectNodeContents(pre);
            var sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            btn.querySelector("span").textContent = "已选中";
          } catch (err) {
            btn.querySelector("span").textContent = "复制失败";
          }
          setTimeout(function () {
            btn.querySelector("span").textContent = "复制";
          }, 2400);
        }
      );
    });
  });

  /* ---------- 代码页签 ---------- */
  doc.querySelectorAll("[data-tabs]").forEach(function (group) {
    var buttons = Array.prototype.slice.call(group.querySelectorAll("[data-tab]"));
    var panels = Array.prototype.slice.call(group.querySelectorAll("[data-panel]"));
    if (!buttons.length) return;

    function select(key) {
      buttons.forEach(function (b) {
        var on = b.getAttribute("data-tab") === key;
        b.setAttribute("aria-selected", on ? "true" : "false");
        b.tabIndex = on ? 0 : -1;
      });
      panels.forEach(function (p) {
        p.hidden = p.getAttribute("data-panel") !== key;
      });
    }

    buttons.forEach(function (b, i) {
      b.setAttribute("role", "tab");
      b.addEventListener("click", function () {
        select(b.getAttribute("data-tab"));
      });
      b.addEventListener("keydown", function (e) {
        if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
        e.preventDefault();
        var dir = e.key === "ArrowRight" ? 1 : -1;
        var next = buttons[(i + dir + buttons.length) % buttons.length];
        next.focus();
        select(next.getAttribute("data-tab"));
      });
    });

    var initial = group.getAttribute("data-tabs-default");
    var first = initial || buttons[0].getAttribute("data-tab");
    select(first);
  });

  /* ---------- 首屏轮播 ---------- */
  var carousel = doc.querySelector("[data-hero-carousel]");
  if (carousel) {
    var slides = Array.prototype.slice.call(carousel.querySelectorAll("[data-slide]"));
    var dots = Array.prototype.slice.call(carousel.querySelectorAll("[data-dot]"));
    var heroCaption = carousel.querySelector("[data-hero-caption]");
    var playBtn = carousel.querySelector("[data-hero-play]");
    var motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
    var heroIndex = 0;
    var heroTimer = null;
    var heroPaused = false;
    var heroHover = false;
    var HERO_DELAY = 6000;

    var renderHero = function (i) {
      heroIndex = (i + slides.length) % slides.length;
      slides.forEach(function (slide, k) {
        var on = k === heroIndex;
        slide.classList.toggle("is-active", on);
        slide.setAttribute("aria-hidden", on ? "false" : "true");
      });
      dots.forEach(function (dot, k) {
        var on = k === heroIndex;
        dot.classList.toggle("is-active", on);
        if (on) dot.setAttribute("aria-current", "true");
        else dot.removeAttribute("aria-current");
      });
      var text = slides[heroIndex].getAttribute("data-caption");
      if (text && heroCaption) heroCaption.textContent = text;
    };

    var stopHero = function () {
      if (heroTimer) {
        window.clearInterval(heroTimer);
        heroTimer = null;
      }
    };

    var runHero = function () {
      stopHero();
      if (heroPaused || heroHover || motionQuery.matches || doc.hidden) return;
      heroTimer = window.setInterval(function () {
        renderHero(heroIndex + 1);
      }, HERO_DELAY);
    };

    var setHeroPaused = function (paused) {
      heroPaused = paused;
      carousel.setAttribute("data-paused", paused ? "true" : "false");
      if (playBtn) {
        playBtn.setAttribute("aria-label", paused ? "开始自动切换" : "暂停自动切换");
        playBtn.setAttribute("aria-pressed", paused ? "true" : "false");
      }
      if (paused) stopHero();
      else runHero();
    };

    dots.forEach(function (dot, k) {
      dot.addEventListener("click", function () {
        renderHero(k);
        runHero();
      });
    });

    if (playBtn) {
      playBtn.addEventListener("click", function () {
        setHeroPaused(!heroPaused);
      });
    }

    carousel.addEventListener("mouseenter", function () {
      heroHover = true;
      stopHero();
    });
    carousel.addEventListener("mouseleave", function () {
      heroHover = false;
      runHero();
    });
    carousel.addEventListener("focusin", stopHero);
    carousel.addEventListener("focusout", function (e) {
      if (!carousel.contains(e.relatedTarget)) runHero();
    });
    carousel.addEventListener("keydown", function (e) {
      if (e.key === "ArrowRight") {
        e.preventDefault();
        renderHero(heroIndex + 1);
        runHero();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        renderHero(heroIndex - 1);
        runHero();
      }
    });
    doc.addEventListener("visibilitychange", function () {
      if (doc.hidden) stopHero();
      else runHero();
    });

    renderHero(0);
    if (motionQuery.matches) setHeroPaused(true);
    else {
      carousel.setAttribute("data-paused", "false");
      if (playBtn) {
        playBtn.setAttribute("aria-label", "暂停自动切换");
        playBtn.setAttribute("aria-pressed", "false");
      }
      runHero();
    }
  }

  /* ---------- 任务栏参数演示 ---------- */
  var demo = doc.querySelector("[data-dock-demo]");
  if (demo) {
    var dock = demo.querySelector(".dock");
    var scaleInput = demo.querySelector("#demoScale");
    var gapInput = demo.querySelector("#demoGap");
    var scaleOut = demo.querySelector("[data-out-scale]");
    var gapOut = demo.querySelector("[data-out-gap]");

    var render = function () {
      if (!dock) return;
      if (scaleInput) {
        var sc = Number(scaleInput.value) / 100;
        dock.style.setProperty("--demo-scale", sc.toFixed(2));
        if (scaleOut) scaleOut.textContent = scaleInput.value + "%";
      }
      if (gapInput) {
        var gap = Number(gapInput.value);
        dock.style.setProperty("--demo-gap", gap + "px");
        if (gapOut) gapOut.textContent = (gap > 0 ? "+" : "") + gap + " px";
      }
    };

    demo.querySelectorAll("[data-shape]").forEach(function (b) {
      if (b.tagName !== "BUTTON") return;
      b.addEventListener("click", function () {
        demo.querySelectorAll("[data-shape]").forEach(function (x) {
          if (x.tagName === "BUTTON") x.setAttribute("aria-pressed", "false");
        });
        b.setAttribute("aria-pressed", "true");
        dock.setAttribute("data-shape", b.getAttribute("data-shape"));
      });
    });

    demo.querySelectorAll("[data-hover]").forEach(function (b) {
      b.addEventListener("click", function () {
        demo.querySelectorAll("[data-hover]").forEach(function (x) {
          x.setAttribute("aria-pressed", "false");
        });
        b.setAttribute("aria-pressed", "true");
        dock.setAttribute("data-hover", b.getAttribute("data-hover"));
      });
    });

    [scaleInput, gapInput].forEach(function (el) {
      if (el) el.addEventListener("input", render);
    });
    render();
  }

  /* ---------- 滚动进入 ---------- */
  var params = new URLSearchParams(window.location.search);
  var still = params.get("still") === "1";
  if (still) root.classList.add("no-reveal");

  var reveals = doc.querySelectorAll(".reveal");
  if (reveals.length) {
    if (still || reduceMotion || !("IntersectionObserver" in window)) {
      reveals.forEach(function (el) { el.classList.add("in"); });
    } else {
      var io = new IntersectionObserver(
        function (entries) {
          entries.forEach(function (entry) {
            if (entry.isIntersecting) {
              entry.target.classList.add("in");
              io.unobserve(entry.target);
            }
          });
        },
        { rootMargin: "0px 0px -8% 0px", threshold: 0.06 }
      );
      reveals.forEach(function (el) { io.observe(el); });
    }
  }

  /* ---------- 目录高亮 ---------- */
  var sideLinks = Array.prototype.slice.call(doc.querySelectorAll(".side-nav a[href^='#']"));
  if (sideLinks.length) {
    var targets = sideLinks
      .map(function (link) {
        var id = link.getAttribute("href").slice(1);
        var el = id ? doc.getElementById(id) : null;
        return el ? { link: link, el: el } : null;
      })
      .filter(Boolean);

    var ticking = false;
    var sync = function () {
      ticking = false;
      var offset = (header ? header.offsetHeight : 0) + 90;
      var current = null;
      for (var i = 0; i < targets.length; i++) {
        if (targets[i].el.getBoundingClientRect().top - offset <= 0) current = targets[i];
      }
      if (!current && targets.length) current = targets[0];
      targets.forEach(function (t) {
        var on = current && t.link === current.link;
        t.link.classList.toggle("is-active", !!on);
        if (on) t.link.setAttribute("aria-current", "location");
        else t.link.removeAttribute("aria-current");
      });
    };

    var requestSync = function () {
      if (!ticking) {
        ticking = true;
        window.requestAnimationFrame(sync);
      }
    };
    window.addEventListener("scroll", requestSync, { passive: true });
    window.addEventListener("resize", requestSync);
    sync();
  }

  /* ---------- 移动端目录折叠 ---------- */
  var side = doc.querySelector(".doc-side");
  var sideToggle = doc.querySelector("[data-side-toggle]");
  if (side && sideToggle) {
    var setSide = function (open) {
      side.classList.toggle("is-collapsed", !open);
      sideToggle.setAttribute("aria-expanded", open ? "true" : "false");
    };
    if (window.innerWidth <= 900) setSide(false);
    sideToggle.addEventListener("click", function () {
      setSide(side.classList.contains("is-collapsed"));
    });
    side.addEventListener("click", function (e) {
      if (e.target.closest("a") && window.innerWidth <= 900) setSide(false);
    });
    window.addEventListener("resize", function () {
      if (window.innerWidth > 900) setSide(true);
    });
  }

  /* ---------- 年份 ---------- */
  doc.querySelectorAll("[data-year]").forEach(function (el) {
    el.textContent = String(new Date().getFullYear());
  });
})();

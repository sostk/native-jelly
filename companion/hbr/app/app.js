/* Native Jelly HBR: the companion app's controller.
 *
 * The app holds no root. Every action runs hbr/nativejelly-hbr.sh (shipped in this app) as root
 * through the Homebrew Channel's exec service, which a rooted TV exposes to apps on the "public"
 * group. The script prints `hbr: active`, `hbr: removed` or `hbr: REFUSED: <why>` for an action,
 * and key=value lines for `status`.
 *
 * Off the TV (no PalmServiceBridge) the page previews canned states for design work:
 * index.html#fresh, #on, #notopened, #noroot, #conflict. */
(function () {
  "use strict";

  var EXEC = "luna://org.webosbrew.hbchannel.service/exec";
  var APPDIR = decodeURIComponent(window.location.pathname).replace(/\/[^\/]*$/, "");
  var SCRIPT = "sh '" + APPDIR + "/hbr/nativejelly-hbr.sh'";
  var APPS = ["com.sostk.nativejelly", "com.sostk.nativejelly.debug"];
  var TESTED_KERNEL = /^4\.4\.84-/;
  var SEEN = "nj-hbr-seen"; // steps 4 and 5 are settings the owner makes; remember they read them
  var bridges = []; // a PalmServiceBridge that is garbage-collected never calls back

  function $(id) { return document.getElementById(id); }

  /* ---- Icons (24x24 line icons) ---- */

  function svg(paths) {
    return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' + paths + "</svg>";
  }
  var ICON = {
    shield: svg('<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>'),
    box: svg('<path d="m7.5 4.27 9 5.15"/><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>'),
    wave: svg('<path d="M2 13a2 2 0 0 0 2-2V7a2 2 0 0 1 4 0v13a2 2 0 0 0 4 0V4a2 2 0 0 1 4 0v13a2 2 0 0 0 4 0v-4a2 2 0 0 1 2-2"/>'),
    speaker: svg('<rect width="16" height="20" x="4" y="2" rx="2"/><path d="M12 6h.01"/><circle cx="12" cy="14" r="4"/><path d="M12 14h.01"/>'),
    toggle: svg('<rect width="20" height="12" x="2" y="6" rx="6"/><circle cx="16" cy="12" r="2"/>'),
    check: svg('<path d="M20 6 9 17l-5-5"/>')
  };

  /* ---- The five steps; render() fills state, text and actions ---- */

  var STEPS = [
    { label: "Root", title: "Rooted TV with the Homebrew Channel", icon: "shield" },
    { label: "Native Jelly", title: "Native Jelly, opened once", icon: "box" },
    { label: "Turn on HBR", title: "Turn on HBR", icon: "wave" },
    { label: "Sound output", title: "Send the sound to your receiver", icon: "speaker" },
    { label: "Native Jelly setting", title: "Switch it on in Native Jelly", icon: "toggle" }
  ];
  var selected = -1;   // the step the card shows
  var last = null;     // the last status, for re-rendering after a local change

  function buildStepper() {
    var nav = $("stepper");
    STEPS.forEach(function (s, i) {
      var tab = document.createElement("div");
      tab.className = "tab";
      tab.id = "tab" + i;
      tab.tabIndex = -1;
      tab.innerHTML = '<div class="node">' + (i + 1) + '</div><div class="label">' + s.label + "</div>";
      tab.addEventListener("click", function () { select(i, true); });
      nav.appendChild(tab);
    });
  }

  /* ---- Talking to the TV ---- */

  function exec(args) {
    if (!window.PalmServiceBridge) return preview(args);
    return new Promise(function (resolve, reject) {
      var bridge = new window.PalmServiceBridge();
      bridges.push(bridge);
      bridge.onservicecallback = function (raw) {
        bridges.splice(bridges.indexOf(bridge), 1);
        var res;
        try { res = JSON.parse(raw); } catch (e) { reject("Unreadable answer from the Homebrew Channel."); return; }
        var out = (res.stdoutString || "").trim();
        if (res.returnValue === true || out) { resolve(out); return; }
        reject(res.errorText || "The Homebrew Channel refused the call.");
      };
      bridge.call(EXEC, JSON.stringify({ command: SCRIPT + " " + args }));
    });
  }

  function status() {
    return exec("status").then(function (out) {
      var st = {};
      out.split("\n").forEach(function (line) {
        var i = line.indexOf("=");
        if (i > 0) st[line.slice(0, i)] = line.slice(i + 1).trim();
      });
      last = st;
      render(st, true);
      return st;
    });
  }

  /* ---- Working out each step from the status ---- */

  function seen(n) { try { return (localStorage.getItem(SEEN) || "").indexOf(String(n)) >= 0; } catch (e) { return false; } }
  function markSeen(n) { try { if (!seen(n)) localStorage.setItem(SEEN, (localStorage.getItem(SEEN) || "") + n); } catch (e) {} }

  function work(st) {
    var root = st.root === "1";
    var kernel = st.kernel || "";
    var ours = st.ours === "1";
    var conflict = st.loaded === "1" && !ours;
    var active = st.enabled === "1" && ours;
    var installed = st.installed === "1";
    var apps = APPS.map(function (id) { return st["app." + id]; }).filter(function (s) { return s && s !== "not-installed"; });
    var opened = apps.filter(function (s) { return s === "node" || s === "no-node"; });
    var ready = active && opened.length > 0 && opened.every(function (s) { return s === "node"; });
    var out = [];

    // 1. Root
    if (!root) {
      out.push({ state: "blocked", text: "This TV is not rooted, or the Homebrew Channel is missing. Root it with the Homebrew Channel first, then open this app again." });
    } else if (!TESTED_KERNEL.test(kernel)) {
      out.push({ state: "done", text: "Rooted. Kernel <i>" + kernel + "</i> was not tested: if the module was not built for it, step 3 refuses and nothing on the TV changes.", next: true });
    } else {
      out.push({ state: "done", text: "Rooted, and on the webOS 6 kernel this module was built and tested for.", next: true });
    }

    // 2. Native Jelly
    if (!root) {
      out.push({ state: "waiting", text: "Install Native Jelly and open it once. That creates the sandbox HBR is added to." });
    } else if (opened.length) {
      out.push({ state: "done", text: "Native Jelly is installed and has its sandbox, so HBR can reach it.", next: true });
    } else {
      out.push({ state: "current", text: (apps.length ? "Native Jelly is installed but has never been opened." : "Native Jelly is not installed yet.") +
        " Install it if needed, open it once, then come back here.",
        actions: [{ id: "refresh", label: "Check again", primary: true }] });
    }

    // 3. HBR
    var before = out[1].state === "done";
    if (!before) {
      out.push({ state: "waiting", text: "Loads the HBR module now, and again at every start of the TV." });
    } else if (conflict) {
      out.push({ state: "blocked", text: "Another earc_route module (from dts_restore_plus) is already loaded. Remove it there, restart the TV, and come back." });
    } else if (ready) {
      out.push({ state: "done", text: "HBR is on. It comes back by itself after every restart of the TV.", next: true,
        actions: [{ id: "remove", label: "Turn off and remove", danger: true }] });
    } else if (active) {
      out.push({ state: "current", text: "HBR is running, but Native Jelly cannot see it yet. That happens after Native Jelly is installed again.",
        actions: [{ id: "install", label: "Fix it", primary: true }, { id: "remove", label: "Turn off and remove", danger: true }] });
    } else if (installed) {
      out.push({ state: "current", text: "HBR is installed but not running.",
        actions: [{ id: "install", label: "Turn on again", primary: true }, { id: "remove", label: "Remove", danger: true }] });
    } else {
      out.push({ state: "current", text: "Loads the HBR module now, and again at every start of the TV. It only changes the sound path while Native Jelly plays a lossless track.",
        actions: [{ id: "install", label: "Turn on HBR", primary: true }] });
    }

    // 4, 5. Settings the owner makes
    var on = out[2].state === "done";
    out.push({ state: !on ? "waiting" : seen(4) ? "done" : "current",
      text: "Connect the receiver by <b>HDMI eARC</b> and pick it in <i>Settings › Sound › Sound Out</i>. Then set <i>Advanced Settings › Digital Sound Output</i> to <b>Pass Through</b>.",
      actions: on ? [{ id: "seen4", label: "Done, next", primary: !seen(4) }] : null });
    out.push({ state: !on ? "waiting" : seen(5) ? "done" : seen(4) ? "current" : "ready",
      text: "In Native Jelly open <i>Settings › Video &amp; playback › Lossless audio to receiver</i> and choose <b>On</b>. Native Jelly then uses HBR by itself.",
      actions: on ? [{ id: "seen5", label: seen(5) ? "All set" : "Finish", primary: !seen(5) }] : null });

    return { steps: out, root: root, conflict: conflict, ready: ready, active: active, installed: installed, kernel: kernel };
  }

  /* ---- Drawing ---- */

  var model = null;

  function render(st, follow) {
    model = work(st);
    var current = -1;
    model.steps.forEach(function (s, i) {
      var tab = $("tab" + i);
      tab.className = "tab " + s.state + (i === selected ? " selected" : "");
      tab.querySelector(".node").innerHTML = s.state === "done" ? ICON.check : s.state === "blocked" ? "!" : String(i + 1);
      if (current < 0 && s.state !== "done") current = i;
    });
    var through = current < 0 ? STEPS.length - 1 : current;
    $("fill").style.width = (through / (STEPS.length - 1) * 100) + "%";

    if (!model.root) pill("bad", "Needs a rooted TV");
    else if (model.conflict) pill("bad", "Another module is loaded");
    else if (model.ready) pill("on", "HBR is on");
    else if (model.active) pill("warn", "On, Native Jelly not ready");
    else if (model.installed) pill("warn", "Installed, not running");
    else pill("off", "HBR is off");
    $("facts").textContent = "Kernel " + (model.kernel || "–") + "   ·   " + (model.installed ? "Starts with the TV" : "Not installed");

    select(follow || selected < 0 ? (current < 0 ? STEPS.length - 1 : current) : selected, false);
  }

  function pill(kind, text) {
    $("pill").className = "pill pill-" + kind;
    $("pillText").textContent = text;
  }

  function select(i, keepTabFocus) {
    if (!model) return;
    var changed = i !== selected;
    selected = i;
    var s = model.steps[i];
    STEPS.forEach(function (_, j) { $("tab" + j).className = $("tab" + j).className.replace(/ selected/g, "") + (j === i ? " selected" : ""); });

    var card = $("card");
    card.className = "card is-" + s.state + (changed ? " swap" : "");
    if (changed) { void card.offsetWidth; } // restart the entrance animation
    var word = { done: "Done", current: "Next up", blocked: "Blocked", waiting: "Later", ready: "Coming up" }[s.state] || "";
    $("kicker").textContent = "Step " + (i + 1) + " of " + STEPS.length + "  ·  " + word;
    $("title").textContent = STEPS[i].title;
    $("text").innerHTML = s.text;

    var badge = s.state === "done" ? '<div class="badge">' + ICON.check + "</div>" : s.state === "blocked" ? '<div class="badge bad">!</div>' : "";
    $("art").innerHTML = '<div class="orb">' + ICON[STEPS[i].icon] + "</div>" + badge;

    var actions = (s.actions || []).slice();
    if (s.next && i < STEPS.length - 1 && !actions.some(function (a) { return a.primary; })) {
      actions.unshift({ id: "next", label: "Next step", primary: true });
    }
    var box = $("actions");
    box.innerHTML = "";
    actions.forEach(function (a) {
      var b = document.createElement("button");
      b.className = "btn" + (a.primary ? " btn-primary" : "") + (a.danger ? " btn-danger" : "");
      b.textContent = a.label;
      b.disabled = busy;
      b.addEventListener("click", function () { run(a.id); });
      box.appendChild(b);
    });

    if (!keepTabFocus) {
      var first = box.querySelector(".btn-primary") || box.querySelector(".btn");
      (first || $("tab" + i)).focus();
    }
  }

  function toast(message, kind) {
    var t = $("toast");
    t.textContent = message;
    t.className = "toast show" + (kind ? " " + kind : "");
    clearTimeout(toast.timer);
    toast.timer = setTimeout(function () { t.className = "toast" + (kind ? " " + kind : ""); }, 5000);
  }

  /* ---- Actions ---- */

  var busy = false;
  function setBusy(on) {
    busy = on;
    Array.prototype.forEach.call(document.querySelectorAll(".btn"), function (b) { b.disabled = on; });
  }

  var SAY = {
    "active": "HBR is on.",
    "removed": "Removed. The module switches off now and is gone after the next restart."
  };

  function shell(label, args) {
    setBusy(true);
    toast(label + "…");
    return exec(args)
      .then(function (out) {
        var lines = out.split("\n");
        var line = lines[lines.length - 1].replace(/^hbr: /, "");
        if (/^REFUSED: /.test(line)) toast("Refused: " + line.slice(9), "bad");
        else toast(SAY[line] || line, "ok");
      }, function (why) { toast(String(why), "bad"); })
      .then(status)
      .catch(function () {})
      .then(function () { setBusy(false); select(selected, false); });
  }

  function run(id) {
    if (busy) return;
    if (id === "next") { select(Math.min(selected + 1, STEPS.length - 1), false); return; }
    if (id === "seen4" || id === "seen5") { markSeen(id.slice(4)); render(last, true); if (id === "seen5") toast("All set. Enjoy lossless sound in Native Jelly.", "ok"); return; }
    if (id === "install") { shell("Turning on HBR", "install '" + APPDIR + "'"); return; }
    if (id === "remove") { shell("Removing HBR", "remove"); return; }
    if (id === "refresh") {
      setBusy(true);
      status()
        .then(function (st) { if (st.enabled === "1" && st.ours === "1") return exec("refresh").then(status); })
        .catch(function (why) { toast(String(why), "bad"); })
        .then(function () { setBusy(false); select(selected, false); });
    }
  }

  /* ---- Remote control: Left/Right walk the steps (or the buttons), Up/Down switch rows ---- */

  document.addEventListener("keydown", function (e) {
    var k = e.keyCode;
    var el = document.activeElement;
    var onTab = el && el.className && String(el.className).indexOf("tab") === 0;
    var btns = Array.prototype.slice.call($("actions").querySelectorAll(".btn"));
    var bi = btns.indexOf(el);
    if (k === 461 || k === 27) {                               // Back
      window.close();
    } else if (onTab && (k === 37 || k === 39)) {              // walk steps
      var n = Math.max(0, Math.min(STEPS.length - 1, selected + (k === 39 ? 1 : -1)));
      select(n, true);
      $("tab" + n).focus();
    } else if (onTab && k === 40) {                            // down to the buttons
      if (btns.length) btns[0].focus();
    } else if (onTab && k === 13) {
      if (btns.length) btns[0].focus();
    } else if (bi >= 0 && (k === 37 || k === 39)) {            // between buttons
      var m = bi + (k === 39 ? 1 : -1);
      if (m >= 0 && m < btns.length) btns[m].focus();
      else if (m < 0 && selected > 0) { select(selected - 1, true); $("tab" + selected).focus(); }
    } else if (bi >= 0 && k === 38) {                          // up to the steps
      $("tab" + selected).focus();
    } else if (!onTab && bi < 0 && k >= 37 && k <= 40) {
      (btns[0] || $("tab" + selected)).focus();
    } else {
      return;
    }
    e.preventDefault();
  });

  /* ---- Preview off the TV ---- */

  function preview(args) {
    var states = {
      fresh: "root=1\nkernel=4.4.84-229.kcl4tv.6\ninstalled=0\nloaded=0\nours=0\nenabled=0\napp.com.sostk.nativejelly=no-node",
      on: "root=1\nkernel=4.4.84-229.kcl4tv.6\ninstalled=1\nloaded=1\nours=1\nenabled=1\napp.com.sostk.nativejelly=node",
      notopened: "root=1\nkernel=4.4.84-229.kcl4tv.6\ninstalled=0\nloaded=0\nours=0\nenabled=0\napp.com.sostk.nativejelly=not-opened",
      noroot: "root=0\nkernel=4.4.84-229.kcl4tv.6\ninstalled=0\nloaded=0\nours=0\nenabled=0",
      conflict: "root=1\nkernel=4.4.84-229.kcl4tv.6\ninstalled=0\nloaded=1\nours=0\nenabled=1\napp.com.sostk.nativejelly=no-node"
    };
    var name = window.location.hash.slice(1) || "fresh";
    if (/^install/.test(args)) { window.location.hash = "on"; return Promise.resolve("hbr: active"); }
    if (args === "remove") { window.location.hash = "fresh"; return Promise.resolve("hbr: removed"); }
    return Promise.resolve(states[name] || states.fresh);
  }

  buildStepper();
  status().catch(function (why) {
    pill("bad", "Cannot reach the Homebrew Channel");
    last = { root: "0" };
    render(last, true);
    toast(String(why), "bad");
  });
})();

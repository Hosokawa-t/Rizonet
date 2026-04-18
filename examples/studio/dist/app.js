// Rizonet Studio — a GUI packager built on Rizonet itself.

"use strict";
const invoke = (cmd, payload) => window.__RIZONET__.invoke(cmd, payload);

const els = {
  source: document.getElementById("source"),
  output: document.getElementById("output"),
  pickSource: document.getElementById("pick-source"),
  pickOutput: document.getElementById("pick-output"),
  name: document.getElementById("name"),
  identifier: document.getElementById("identifier"),
  title: document.getElementById("title"),
  mode: document.getElementById("mode"),
  windowMode: document.getElementById("window-mode"),
  width: document.getElementById("width"),
  height: document.getElementById("height"),
  resizable: document.getElementById("resizable"),
  allow: document.getElementById("allow"),
  makeInstaller: document.getElementById("make-installer"),
  build: document.getElementById("build"),
  openOut: document.getElementById("open-out"),
  devtools: document.getElementById("devtools"),
  log: document.getElementById("log"),
  progress: document.getElementById("progress"),
};

function setProgress(state, text) {
  els.progress.className = "progress " + (state || "");
  els.progress.textContent = text;
}

function appendLog(text, cls) {
  const span = document.createElement("span");
  if (cls) span.className = cls;
  span.textContent = text + "\n";
  els.log.appendChild(span);
  els.log.scrollTop = els.log.scrollHeight;
}

els.pickSource.addEventListener("click", async () => {
  const p = await invoke("dialog:open_file", {
    title: "Pick web app folder (containing index.html)",
    directory: true,
  });
  if (p) els.source.value = p;
});
els.pickOutput.addEventListener("click", async () => {
  const p = await invoke("dialog:open_file", {
    title: "Pick output folder",
    directory: true,
  });
  if (p) els.output.value = p;
});

els.devtools.addEventListener("click", () => invoke("devtools:open"));

els.build.addEventListener("click", async () => {
  const args = {
    source_dir: els.source.value.trim(),
    output_dir: els.output.value.trim(),
    name: els.name.value.trim() || "My App",
    identifier: els.identifier.value.trim() || "com.example.myapp",
    title: els.title.value.trim() || els.name.value.trim(),
    mode: els.mode.value,
    window_mode: els.windowMode.value,
    width: Number(els.width.value) || 1024,
    height: Number(els.height.value) || 768,
    resizable: els.resizable.checked,
    allow: els.allow.value.split(/\r?\n/).map((s) => s.trim()).filter(Boolean),
    make_installer: els.makeInstaller.checked,
  };

  if (!args.source_dir) {
    setProgress("error", "source required");
    return;
  }
  if (!args.output_dir) {
    setProgress("error", "output required");
    return;
  }

  els.log.textContent = "";
  els.build.disabled = true;
  els.openOut.disabled = true;
  setProgress("running", "building…");
  appendLog("▶ starting build", "info");

  try {
    await invoke("studio:build", args);
  } catch (e) {
    setProgress("error", "failed");
    appendLog("✖ " + e.message, "stderr");
    els.build.disabled = false;
  }
});

els.openOut.addEventListener("click", async () => {
  if (els.output.value) await invoke("shell:open", { target: els.output.value });
});

// Event stream from Rust.
window.__RIZONET__.on("studio:log", (p) => {
  appendLog(p.line, p.stream === "stderr" ? "stderr" : null);
});
window.__RIZONET__.on("studio:done", (p) => {
  setProgress("ok", "done");
  appendLog("✔ binary:    " + p.binary, "ok");
  appendLog("✔ assets:    " + p.dist, "ok");
  if (p.installer) appendLog("✔ installer: " + p.installer, "ok");
  els.build.disabled = false;
  els.openOut.disabled = false;
});
window.__RIZONET__.on("studio:error", (p) => {
  setProgress("error", "failed");
  appendLog("✖ " + p.message, "stderr");
  els.build.disabled = false;
});

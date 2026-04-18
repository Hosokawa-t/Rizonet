// Rizonet Notes — a small Markdown notepad built on the Rizonet IPC bridge.
// All persistence (list / read / write / delete / reveal) is delegated to
// Rust via `window.__RIZONET__.invoke(cmd, payload)`.

"use strict";

const invoke = (cmd, payload) => window.__RIZONET__.invoke(cmd, payload);

// ---------- Minimal Markdown renderer ----------
// A tiny subset: headings, bold/italic/code, links, lists, code fences,
// blockquotes, paragraphs. Good enough for notes, zero dependencies.
function escapeHtml(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderMarkdown(src) {
  const lines = src.split(/\r?\n/);
  let html = "";
  let inCode = false;
  let codeBuf = [];
  let paraBuf = [];
  let listBuf = [];

  const flushPara = () => {
    if (paraBuf.length) {
      html += `<p>${inline(paraBuf.join(" "))}</p>`;
      paraBuf = [];
    }
  };
  const flushList = () => {
    if (listBuf.length) {
      html += `<ul>${listBuf.map((i) => `<li>${inline(i)}</li>`).join("")}</ul>`;
      listBuf = [];
    }
  };

  for (const raw of lines) {
    const line = raw;
    if (inCode) {
      if (/^```/.test(line)) {
        html += `<pre><code>${escapeHtml(codeBuf.join("\n"))}</code></pre>`;
        codeBuf = [];
        inCode = false;
      } else {
        codeBuf.push(line);
      }
      continue;
    }
    if (/^```/.test(line)) {
      flushPara();
      flushList();
      inCode = true;
      continue;
    }
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      flushPara();
      flushList();
      html += `<h${h[1].length}>${inline(h[2])}</h${h[1].length}>`;
      continue;
    }
    const li = line.match(/^\s*[-*]\s+(.*)$/);
    if (li) {
      flushPara();
      listBuf.push(li[1]);
      continue;
    }
    const bq = line.match(/^>\s?(.*)$/);
    if (bq) {
      flushPara();
      flushList();
      html += `<blockquote>${inline(bq[1])}</blockquote>`;
      continue;
    }
    if (line.trim() === "") {
      flushPara();
      flushList();
      continue;
    }
    paraBuf.push(line);
  }
  if (inCode) {
    html += `<pre><code>${escapeHtml(codeBuf.join("\n"))}</code></pre>`;
  }
  flushPara();
  flushList();
  return html;
}

function inline(s) {
  let r = escapeHtml(s);
  // Inline code first so its content isn't mangled.
  r = r.replace(/`([^`]+)`/g, (_, c) => `<code>${c}</code>`);
  r = r.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  r = r.replace(/\*([^*]+)\*/g, "<em>$1</em>");
  r = r.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_, t, u) => `<a href="${u}">${t}</a>`);
  return r;
}

// ---------- App state ----------
const state = {
  notes: [],
  activeName: null,
  dirty: false,
  filter: "",
};

// ---------- Rendering ----------
const els = {
  list: document.getElementById("note-list"),
  search: document.getElementById("search"),
  title: document.getElementById("title"),
  editor: document.getElementById("editor"),
  preview: document.getElementById("preview"),
  save: document.getElementById("save"),
  del: document.getElementById("delete"),
  copy: document.getElementById("copy"),
  import: document.getElementById("import"),
  newBtn: document.getElementById("new-note"),
  reveal: document.getElementById("reveal-dir"),
  devtools: document.getElementById("open-devtools"),
  status: document.getElementById("status"),
};

function renderList() {
  const q = state.filter.toLowerCase();
  els.list.innerHTML = "";
  for (const n of state.notes) {
    if (q && !n.name.toLowerCase().includes(q)) continue;
    const li = document.createElement("li");
    if (n.name === state.activeName) li.classList.add("active");
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = n.name;
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = new Date(n.modified_unix * 1000).toLocaleString();
    li.appendChild(name);
    li.appendChild(meta);
    li.addEventListener("click", () => openNote(n.name));
    els.list.appendChild(li);
  }
}

function renderPreview() {
  els.preview.innerHTML = renderMarkdown(els.editor.value || "");
}

function setStatus(text) {
  els.status.textContent = text;
}

function setDirty(v) {
  state.dirty = v;
  els.save.disabled = !v || !state.activeName;
  els.copy.disabled = !state.activeName;
}

// ---------- Actions ----------
async function refreshList() {
  try {
    state.notes = await invoke("notes:list");
    renderList();
  } catch (e) {
    setStatus("list failed: " + e.message);
  }
}

async function openNote(name) {
  try {
    const content = await invoke("notes:read", { name });
    state.activeName = name;
    els.title.value = name.replace(/\.md$/i, "");
    els.title.disabled = false;
    els.editor.value = content;
    els.editor.disabled = false;
    els.del.disabled = false;
    setDirty(false);
    renderList();
    renderPreview();
    setStatus(`opened ${name}`);
  } catch (e) {
    setStatus("open failed: " + e.message);
  }
}

async function saveNote() {
  if (!state.activeName) return;
  const newName = (els.title.value || "Untitled").trim() + ".md";
  try {
    if (newName !== state.activeName) {
      await invoke("notes:rename", { from: state.activeName, to: newName });
      state.activeName = newName;
    }
    await invoke("notes:write", {
      name: state.activeName,
      content: els.editor.value,
    });
    setDirty(false);
    await refreshList();
    setStatus(`saved ${state.activeName}`);
  } catch (e) {
    setStatus("save failed: " + e.message);
  }
}

async function deleteNote() {
  if (!state.activeName) return;
  const name = state.activeName;
  if (!confirm(`Delete ${name}?`)) return;
  try {
    await invoke("notes:delete", { name });
    state.activeName = null;
    els.title.value = "";
    els.title.disabled = true;
    els.editor.value = "";
    els.editor.disabled = true;
    els.del.disabled = true;
    setDirty(false);
    await refreshList();
    renderPreview();
    setStatus(`deleted ${name}`);
  } catch (e) {
    setStatus("delete failed: " + e.message);
  }
}

async function createNote() {
  const base = "Untitled";
  const existing = new Set(state.notes.map((n) => n.name));
  let name = `${base}.md`;
  let i = 2;
  while (existing.has(name)) name = `${base}-${i++}.md`;
  try {
    await invoke("notes:write", {
      name,
      content: `# ${name.replace(/\.md$/, "")}\n\nStart writing...\n`,
    });
    await refreshList();
    await openNote(name);
  } catch (e) {
    setStatus("create failed: " + e.message);
  }
}

async function revealDir() {
  try {
    const path = await invoke("notes:reveal_dir");
    await invoke("shell:open", { target: path });
    setStatus(`opened ${path}`);
  } catch (e) {
    setStatus("reveal failed: " + e.message);
  }
}

async function copyToClipboard() {
  if (!state.activeName) return;
  try {
    await invoke("clipboard:write_text", { text: els.editor.value });
    setStatus(`copied ${state.activeName} to clipboard`);
  } catch (e) {
    setStatus("clipboard failed: " + e.message);
  }
}

async function importFile() {
  try {
    const path = await invoke("dialog:open_file", {
      title: "Import a markdown file",
      filters: [{ name: "Markdown", extensions: ["md", "markdown", "txt"] }],
    });
    if (!path) return;
    // Read via the built-in http:fetch against file://? Simpler: a dedicated
    // notes:import command on the Rust side would work, but for a demo we
    // round-trip through shell open. Here we simply show the path.
    setStatus(`selected ${path}`);
  } catch (e) {
    setStatus("import failed: " + e.message);
  }
}

async function openDevtools() {
  try {
    await invoke("devtools:open");
  } catch (e) {
    setStatus("devtools failed: " + e.message);
  }
}

// ---------- Wire events ----------
els.editor.addEventListener("input", () => {
  setDirty(true);
  renderPreview();
});
els.title.addEventListener("input", () => setDirty(true));
els.save.addEventListener("click", saveNote);
els.del.addEventListener("click", deleteNote);
els.newBtn.addEventListener("click", createNote);
els.reveal.addEventListener("click", revealDir);
els.copy.addEventListener("click", copyToClipboard);
els.import.addEventListener("click", importFile);
els.devtools.addEventListener("click", openDevtools);
els.search.addEventListener("input", (e) => {
  state.filter = e.target.value;
  renderList();
});

// Keyboard: Ctrl/Cmd+S to save.
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
    e.preventDefault();
    saveNote();
  }
});

// ---------- Bootstrap ----------
(async function init() {
  if (!window.__RIZONET__) {
    setStatus("Rizonet bridge missing. Open this app via `cargo run`.");
    return;
  }
  setStatus(`Rizonet ${window.__RIZONET__.version}`);

  // Subscribe to Rust-originated events.
  window.__RIZONET__.on("notes:saved", (p) => {
    // Fire-and-forget native toast so we can demo the notification API.
    invoke("notification:send", {
      title: "Rizonet Notes",
      body: `Saved ${p?.name ?? "note"}`,
    }).catch(() => {});
  });

  await refreshList();
  if (state.notes.length > 0) {
    await openNote(state.notes[0].name);
  }
})();

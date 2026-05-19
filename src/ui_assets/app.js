const token = new URLSearchParams(window.location.search).get("token") || "";
const state = {
  bootstrap: null,
  issues: [],
  current: null,
  selected: new Set(),
  lastBulk: [],
};

const $ = (selector) => document.querySelector(selector);

function toast(message) {
  const node = $("#toast");
  node.textContent = message;
  node.classList.remove("hidden");
  window.clearTimeout(toast.timer);
  toast.timer = window.setTimeout(() => node.classList.add("hidden"), 3200);
}

async function api(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      "X-ITR-Token": token,
      ...(options.headers || {}),
    },
  });
  const text = await response.text();
  const data = text ? JSON.parse(text) : {};
  if (!response.ok) {
    throw new Error(data.error || `HTTP ${response.status}`);
  }
  return data;
}

function paramsFromFilters() {
  const params = new URLSearchParams();
  const fields = ["search:q", "status:status", "priority:priority", "kind:kind", "tag:tag", "skill:skill", "assignee:assigned_to"];
  for (const pair of fields) {
    const [id, key] = pair.split(":");
    const value = $(`#${id}`).value.trim();
    if (value) params.set(key, value);
  }
  if ($("#ready").checked) params.set("ready", "true");
  if ($("#blocked").checked) params.set("blocked", "true");
  if ($("#all").checked) params.set("all", "true");
  params.set("limit", "500");
  return params.toString();
}

function listText(values) {
  return (values || []).join(", ");
}

function parseList(value) {
  return value.split(",").map((item) => item.trim()).filter(Boolean);
}

function parseIds(value) {
  return value.split(",").map((item) => Number(item.trim())).filter(Number.isFinite);
}

function setOptions(select, values, selected) {
  select.innerHTML = "";
  for (const value of values) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    if (value === selected) option.selected = true;
    select.append(option);
  }
}

async function loadBootstrap() {
  const data = await api("/api/bootstrap");
  state.bootstrap = data;
  $("#dbPath").textContent = data.db_path;
  renderStats(data.stats);
  $("#sqlPanel").classList.toggle("hidden", !data.dangerous_sql);
  setOptions(document.querySelector("#newForm [name=priority]"), data.priorities, "medium");
  setOptions(document.querySelector("#newForm [name=kind]"), data.kinds, "task");
}

function renderStats(stats) {
  $("#stats").innerHTML = Object.entries(stats)
    .map(([key, value]) => `<span class="stat">${key}: ${value}</span>`)
    .join("");
}

async function loadIssues(keepCurrent = true) {
  const query = paramsFromFilters();
  const data = await api(`/api/issues?${query}`);
  state.issues = data.issues;
  $("#resultCount").textContent = `${data.total} issue${data.total === 1 ? "" : "s"}`;
  renderRows();
  renderSelection();
  if (keepCurrent && state.current) {
    const found = state.issues.some((issue) => issue.id === state.current.id);
    if (found) await selectIssue(state.current.id);
  }
}

function renderRows() {
  const rows = $("#issueRows");
  rows.innerHTML = "";
  for (const issue of state.issues) {
    const row = document.createElement("tr");
    row.dataset.id = issue.id;
    if (state.current && state.current.id === issue.id) row.classList.add("active");
    row.innerHTML = `
      <td><input type="checkbox" ${state.selected.has(issue.id) ? "checked" : ""}></td>
      <td>#${issue.id}</td>
      <td>${issue.urgency.toFixed(1)}</td>
      <td>${issue.status}</td>
      <td>${issue.priority}</td>
      <td>${issue.kind}</td>
      <td class="title-cell">${escapeHtml(issue.title)}</td>
      <td>${(issue.tags || []).slice(0, 4).map((tag) => `<span class="pill">${escapeHtml(tag)}</span>`).join("")}</td>
      <td class="muted">${escapeHtml(issue.updated_at)}</td>
    `;
    row.addEventListener("click", (event) => {
      if (event.target.type === "checkbox") {
        if (event.target.checked) state.selected.add(issue.id);
        else state.selected.delete(issue.id);
        state.lastBulk = [];
        $("#bulkPreviewPanel").classList.add("hidden");
        renderSelection();
        return;
      }
      selectIssue(issue.id).catch((error) => toast(error.message));
    });
    rows.append(row);
  }
}

function renderSelection() {
  const count = state.selected.size;
  $("#selectionCount").textContent = `${count} selected`;
  $("#bulkApply").disabled = state.lastBulk.length === 0;
  $("#bulkWontfix").disabled = state.lastBulk.length === 0;
}

async function selectIssue(id) {
  const data = await api(`/api/issues/${id}`);
  state.current = data.issue;
  renderDetail();
  renderRows();
}

function renderDetail() {
  const issue = state.current;
  $("#detailEmpty").classList.add("hidden");
  $("#detailForm").classList.remove("hidden");
  $("#notesPanel").classList.remove("hidden");
  $("#linksPanel").classList.remove("hidden");
  $("#detailId").textContent = `#${issue.id}`;

  const form = $("#detailForm");
  const fields = form.elements;
  fields.title.value = issue.title || "";
  setOptions(fields.status, state.bootstrap.statuses, issue.status);
  setOptions(fields.priority, state.bootstrap.priorities, issue.priority);
  setOptions(fields.kind, state.bootstrap.kinds, issue.kind);
  fields.assigned_to.value = issue.assigned_to || "";
  fields.parent_id.value = issue.parent_id || "";
  fields.close_reason.value = issue.close_reason || "";
  fields.context.value = issue.context || "";
  fields.acceptance.value = issue.acceptance || "";
  fields.files.value = listText(issue.files);
  fields.tags.value = listText(issue.tags);
  fields.skills.value = listText(issue.skills);
  renderNotes(issue.notes || []);
  renderDependencies(issue);
  renderRelations(issue.relations || []);
}

async function patchCurrent(patch) {
  if (!state.current) return;
  $("#saveState").textContent = "saving";
  try {
    const data = await api(`/api/issues/${state.current.id}`, {
      method: "PATCH",
      body: JSON.stringify(patch),
    });
    state.current = data.issue;
    $("#saveState").textContent = "saved";
    await loadIssues(false);
  } catch (error) {
    $("#saveState").textContent = "error";
    toast(error.message);
  }
}

function wireDetailAutosave() {
  const form = $("#detailForm");
  const fields = form.elements;
  for (const field of ["status", "priority", "kind"]) {
    fields[field].addEventListener("change", () => patchCurrent({ [field]: fields[field].value }));
  }
  for (const field of ["title", "context", "acceptance", "assigned_to", "close_reason"]) {
    fields[field].addEventListener("blur", () => patchCurrent({ [field]: fields[field].value }));
  }
  fields.parent_id.addEventListener("blur", () => {
    const value = fields.parent_id.value.trim();
    patchCurrent({ parent_id: value ? Number(value) : null });
  });
  for (const field of ["files", "tags", "skills"]) {
    fields[field].addEventListener("blur", () => patchCurrent({ [field]: parseList(fields[field].value) }));
  }
}

function renderNotes(notes) {
  const node = $("#notes");
  node.innerHTML = "";
  for (const note of notes) {
    const item = document.createElement("div");
    item.className = "note";
    item.innerHTML = `
      <div class="note-meta">#${note.id} ${escapeHtml(note.agent || "")} ${escapeHtml(note.created_at)}</div>
      <textarea rows="3">${escapeHtml(note.content)}</textarea>
      <div class="actions"><button data-action="save">Save</button><button data-action="delete">Delete</button></div>
    `;
    item.querySelector("[data-action=save]").addEventListener("click", async () => {
      try {
        const data = await api(`/api/notes/${note.id}`, {
          method: "PATCH",
          body: JSON.stringify({ content: item.querySelector("textarea").value }),
        });
        state.current = data.issue;
        renderDetail();
      } catch (error) {
        toast(error.message);
      }
    });
    item.querySelector("[data-action=delete]").addEventListener("click", async () => {
      try {
        const data = await api(`/api/notes/${note.id}`, { method: "DELETE" });
        state.current = data.issue;
        renderDetail();
      } catch (error) {
        toast(error.message);
      }
    });
    node.append(item);
  }
}

function renderDependencies(issue) {
  const node = $("#dependencies");
  const blockers = issue.blocked_by || [];
  const blocks = issue.blocks || [];
  node.innerHTML = [
    ...blockers.map((id) => `<div class="link-row">blocked by #${id} <button data-blocker="${id}">Remove</button></div>`),
    ...blocks.map((id) => `<div class="link-row">blocks #${id}</div>`),
  ].join("") || "<div class=\"muted\">none</div>";
  node.querySelectorAll("[data-blocker]").forEach((button) => {
    button.addEventListener("click", async () => {
      try {
        const data = await api(`/api/issues/${issue.id}/dependencies/${button.dataset.blocker}`, { method: "DELETE" });
        state.current = data.issue;
        renderDetail();
        await loadIssues(false);
      } catch (error) {
        toast(error.message);
      }
    });
  });
}

function renderRelations(relations) {
  const node = $("#relations");
  node.innerHTML = "";
  if (relations.length === 0) {
    node.innerHTML = "<div class=\"muted\">none</div>";
    return;
  }
  for (const rel of relations) {
    const outbound = rel.source_id === state.current.id;
    const other = outbound ? rel.target_id : rel.source_id;
    const row = document.createElement("div");
    row.className = "link-row";
    row.innerHTML = `${escapeHtml(rel.relation_type)} ${outbound ? "to" : "from"} #${other} ${outbound ? `<button data-target="${other}">Remove</button>` : ""}`;
    const button = row.querySelector("button");
    if (button) {
      button.addEventListener("click", async () => {
        try {
          const data = await api(`/api/issues/${state.current.id}/relations/${button.dataset.target}`, { method: "DELETE" });
          state.current = data.issue;
          renderDetail();
        } catch (error) {
          toast(error.message);
        }
      });
    }
    node.append(row);
  }
}

function wireActions() {
  $("#refresh").addEventListener("click", () => loadIssues().catch((error) => toast(error.message)));
  $("#newIssue").addEventListener("click", () => $("#newDialog").showModal());
  for (const id of ["search", "status", "priority", "kind", "tag", "skill", "assignee", "ready", "blocked", "all"]) {
    const eventName = id === "search" ? "input" : "change";
    $(`#${id}`).addEventListener(eventName, debounce(() => loadIssues(false).catch((error) => toast(error.message)), 180));
  }
  $("#closeIssue").addEventListener("click", () => closeCurrent(false));
  $("#wontfixIssue").addEventListener("click", () => closeCurrent(true));
  $("#addNote").addEventListener("click", addNote);
  $("#addDependency").addEventListener("click", addDependency);
  $("#addRelation").addEventListener("click", addRelation);
  $("#createIssue").addEventListener("click", createIssue);
  $("#bulkPreview").addEventListener("click", bulkPreview);
  $("#bulkApply").addEventListener("click", () => bulkApply(false));
  $("#bulkWontfix").addEventListener("click", () => bulkApply(true));
  $("#runSql").addEventListener("click", runSql);
  $("#clearSql").addEventListener("click", clearSql);
}

async function closeCurrent(wontfix) {
  if (!state.current) return;
  const reason = $("#detailForm").elements.close_reason.value;
  try {
    const data = await api(`/api/issues/${state.current.id}/close`, {
      method: "POST",
      body: JSON.stringify({ reason, wontfix }),
    });
    state.current = data.issue;
    renderDetail();
    await loadIssues(false);
  } catch (error) {
    toast(error.message);
  }
}

async function addNote() {
  const content = $("#newNote").value.trim();
  if (!content || !state.current) return;
  try {
    const data = await api(`/api/issues/${state.current.id}/notes`, {
      method: "POST",
      body: JSON.stringify({ content }),
    });
    $("#newNote").value = "";
    state.current = data.issue;
    renderDetail();
  } catch (error) {
    toast(error.message);
  }
}

async function addDependency() {
  const blockerId = Number($("#blockerId").value.trim());
  if (!Number.isFinite(blockerId) || !state.current) return;
  try {
    const data = await api(`/api/issues/${state.current.id}/dependencies`, {
      method: "POST",
      body: JSON.stringify({ blocker_id: blockerId }),
    });
    $("#blockerId").value = "";
    state.current = data.issue;
    renderDetail();
    await loadIssues(false);
  } catch (error) {
    toast(error.message);
  }
}

async function addRelation() {
  const targetId = Number($("#relationTarget").value.trim());
  if (!Number.isFinite(targetId) || !state.current) return;
  try {
    const data = await api(`/api/issues/${state.current.id}/relations`, {
      method: "POST",
      body: JSON.stringify({ target_id: targetId, relation_type: $("#relationType").value }),
    });
    $("#relationTarget").value = "";
    state.current = data.issue;
    renderDetail();
  } catch (error) {
    toast(error.message);
  }
}

async function createIssue(event) {
  event.preventDefault();
  const form = $("#newForm");
  const body = Object.fromEntries(new FormData(form).entries());
  body.files = parseList(body.files || "");
  body.tags = parseList(body.tags || "");
  body.skills = parseList(body.skills || "");
  body.blocked_by = parseIds(body.blocked_by || "");
  try {
    const data = await api("/api/issues", { method: "POST", body: JSON.stringify(body) });
    $("#newDialog").close();
    form.reset();
    await loadIssues(false);
    await selectIssue(data.issue.id);
  } catch (error) {
    toast(error.message);
  }
}

async function bulkPreview() {
  const ids = [...state.selected];
  if (ids.length === 0) return;
  try {
    const data = await api("/api/bulk/resolve/preview", {
      method: "POST",
      body: JSON.stringify({ ids }),
    });
    state.lastBulk = data.issues.map((issue) => issue.id);
    $("#bulkPreviewPanel").classList.remove("hidden");
    $("#bulkPreviewPanel").innerHTML = `<strong>${data.count} issues</strong><br>${data.issues.map((issue) => `#${issue.id} ${escapeHtml(issue.title)}`).join("<br>")}`;
    renderSelection();
  } catch (error) {
    toast(error.message);
  }
}

async function bulkApply(wontfix) {
  const ids = state.lastBulk;
  if (ids.length === 0) return;
  try {
    const data = await api("/api/bulk/resolve/apply", {
      method: "POST",
      body: JSON.stringify({ ids, reason: $("#bulkReason").value, wontfix }),
    });
    state.selected.clear();
    state.lastBulk = [];
    $("#bulkPreviewPanel").classList.add("hidden");
    toast(`Resolved ${data.count} issues`);
    await loadIssues(false);
  } catch (error) {
    toast(error.message);
  }
}

async function runSql() {
  const sql = $("#sqlText").value.trim();
  if (!sql) return;
  $("#sqlState").textContent = "running";
  try {
    const data = await api("/api/sql", {
      method: "POST",
      body: JSON.stringify({ sql }),
    });
    $("#sqlState").textContent = "done";
    renderSqlResult(data);
    if (Number(data.changes || 0) > 0) {
      await loadBootstrap();
      await loadIssues(false);
      if (state.current) {
        await selectIssue(state.current.id);
      }
    }
  } catch (error) {
    $("#sqlState").textContent = "error";
    toast(error.message);
  }
}

function clearSql() {
  $("#sqlText").value = "";
  $("#sqlState").textContent = "";
  $("#sqlResult").classList.add("hidden");
  $("#sqlResult").innerHTML = "";
}

function renderSqlResult(data) {
  const node = $("#sqlResult");
  const columns = data.columns || [];
  const rows = data.rows || [];
  const shown = rows.length;
  const total = Number(data.row_count || 0);
  const changes = Number(data.changes || 0);
  const truncated = data.truncated ? `, showing ${shown}` : "";
  const summary = `${total} row${total === 1 ? "" : "s"}${truncated}, ${changes} change${changes === 1 ? "" : "s"}`;

  node.classList.remove("hidden");
  if (columns.length === 0) {
    node.innerHTML = `<div class="muted">${summary}</div>`;
    return;
  }

  node.innerHTML = `
    <div class="muted">${summary}</div>
    <table class="sql-table">
      <thead><tr>${columns.map((column) => `<th>${escapeHtml(column)}</th>`).join("")}</tr></thead>
      <tbody>
        ${rows.map((row) => `<tr>${columns.map((_, index) => `<td>${formatSqlValue(row[index])}</td>`).join("")}</tr>`).join("")}
      </tbody>
    </table>
  `;
}

function formatSqlValue(value) {
  if (value === null || value === undefined) return "<span class=\"muted\">NULL</span>";
  if (typeof value === "object") return escapeHtml(JSON.stringify(value));
  return escapeHtml(value);
}

function debounce(fn, delay) {
  let timer = null;
  return (...args) => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => fn(...args), delay);
  };
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

async function init() {
  try {
    await loadBootstrap();
    wireDetailAutosave();
    wireActions();
    await loadIssues(false);
  } catch (error) {
    toast(error.message);
  }
}

init();

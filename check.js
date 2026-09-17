'use strict';

const $ = id => document.getElementById(id);

// ─────────────────────────────────────────────
// State
// ─────────────────────────────────────────────
const S = {
  connections: [],   // [{name, host, url, ...}]
  queries: [],       // [{id, name, connection_name, sql, format, output_dir}]
  formats: [],       // [{id, label}]
  activeConn: null,  // name string
  connData: null,    // full conn obj with password
  activeQuery: {id:'', name:'New Query', connection_name:'', sql:'', format:'csv', output_dir:''},
  expanded: new Set(),
  job: null,
  jobTimer: null,
  editingConn: null, // name of conn being edited, null for new
};

// ─────────────────────────────────────────────
// API helpers
// ─────────────────────────────────────────────
async function api(method, path, body) {
  const opts = {method, headers: {'Content-Type':'application/json'}};
  if (body !== undefined) opts.body = JSON.stringify(body);
  const res = await fetch(path, opts);
  if (!res.ok) {
    const txt = await res.text().catch(() => '');
    throw new Error(txt.slice(0, 300) || `HTTP ${res.status}`);
  }
  return res.json();
}

// ─────────────────────────────────────────────
// Toast
// ─────────────────────────────────────────────
let toastTimer;
function toast(msg, type='') {
  const el = $('toast');
  el.textContent = msg;
  el.className = 'toast show ' + type;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.className = 'toast'; }, 3000);
}

// ─────────────────────────────────────────────
// Status bar
// ─────────────────────────────────────────────
function setStatus(text, type='') {
  $('statusText').textContent = text;
  const dot = $('statusDot');
  dot.className = 'status-dot ' + type;
  $('progressBar').style.display = type === 'running' ? '' : 'none';
}

// ─────────────────────────────────────────────
// SIDEBAR render
// ─────────────────────────────────────────────
function renderSidebar() {
  const tree = $('sidebarTree');
  if (!S.connections.length) {
    tree.innerHTML = `<div class="empty-state" style="padding:30px 16px">
      <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="var(--dim)" stroke-width="1.5" style="margin:0 auto 8px;display:block">
        <path d="M21 5c0 1.657-4.03 3-9 3S3 6.657 3 5s4.03-3 9-3 9 1.343 9 3z"/>
        <path d="M3 5v6c0 1.657 4.03 3 9 3s9-1.343 9-3V5"/>
        <path d="M3 11v6c0 1.657 4.03 3 9 3s9-1.343 9-3v-6"/>
      </svg><p>No connections yet.<br>Click + to add one.</p></div>`;
    return;
  }

  const esc = s => String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
  let html = '';
  for (const conn of S.connections) {
    const isSelected = S.activeConn === conn.name;
    const isExpanded = S.expanded.has(conn.name);
    const myQueries = S.queries.filter(q => q.connection_name === conn.name);
    html += `<div class="conn-group">
      <div class="conn-row${isSelected ? ' selected' : ''}" data-conn="${esc(conn.name)}">
        <span class="conn-toggle${isExpanded ? ' open' : ''}">
          <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor"><polygon points="2,2 8,5 2,8"/></svg>
        </span>
        <svg class="conn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <polygon points="12,2 22,8.5 22,15.5 12,22 2,15.5 2,8.5"/>
          <polygon points="12,7 17,10 17,14 12,17 7,14 7,10"/>
        </svg>
        <span class="conn-name" title="${esc(conn.name)}">${esc(conn.name)}</span>
        <span class="conn-actions">
          <button class="conn-action" data-edit="${esc(conn.name)}" title="Edit connection">
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8">
              <path d="M11 2l3 3L5 14H2v-3L11 2z"/>
            </svg>
          </button>
          <button class="conn-action" data-del="${esc(conn.name)}" title="Delete connection" style="color:var(--err)">
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8">
              <polyline points="2,4 14,4"/><path d="M5 4V2h6v2"/>
              <path d="M12 4l-1 10H5L4 4"/>
            </svg>
          </button>
        </span>
      </div>`;

    if (isExpanded) {
      html += '<div class="queries-list">';
      for (const q of myQueries) {
        const isActive = S.activeQuery.id === q.id;
        html += `<div class="query-row${isActive ? ' active' : ''}" data-query="${esc(q.id)}">
          <svg class="query-icon" width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8">
            <rect x="2" y="2" width="12" height="12" rx="1"/>
            <line x1="5" y1="6" x2="11" y2="6"/><line x1="5" y1="9" x2="9" y2="9"/>
          </svg>
          <span class="query-name" title="${esc(q.name)}">${esc(q.name)}</span>
          <button class="query-del" data-qdel="${esc(q.id)}" title="Delete query">
            <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="4" y1="4" x2="12" y2="12"/><line x1="12" y1="4" x2="4" y2="12"/>
            </svg>
          </button>
        </div>`;
      }
      html += `<div class="new-query-btn" data-newquery="${esc(conn.name)}">
        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8">
          <line x1="8" y1="3" x2="8" y2="13"/><line x1="3" y1="8" x2="13" y2="8"/>
        </svg> New Query
      </div></div>`;
    }
    html += '</div>';
  }
  tree.innerHTML = html;
}

// ─────────────────────────────────────────────
// Sidebar click delegation
// ─────────────────────────────────────────────
$('sidebarTree').addEventListener('click', async e => {
  const connRow = e.target.closest('[data-conn]');
  const editBtn = e.target.closest('[data-edit]');
  const delBtn  = e.target.closest('[data-del]');
  const qRow    = e.target.closest('[data-query]');
  const qDel    = e.target.closest('[data-qdel]');
  const newQ    = e.target.closest('[data-newquery]');

  if (editBtn) { e.stopPropagation(); openConnModal(editBtn.dataset.edit); return; }
  if (delBtn)  { e.stopPropagation(); await deleteConn(delBtn.dataset.del); return; }
  if (qDel)    { e.stopPropagation(); await deleteQuery(qDel.dataset.qdel); return; }
  if (qRow)    { loadQuery(qRow.dataset.query); return; }
  if (newQ)    { newQueryFor(newQ.dataset.newquery); return; }
  if (connRow) { await selectConn(connRow.dataset.conn); }
});

// ─────────────────────────────────────────────
// Connection selection
// ─────────────────────────────────────────────
async function selectConn(name) {
  if (S.activeConn === name) {
    S.expanded.has(name) ? S.expanded.delete(name) : S.expanded.add(name);
  } else {
    S.activeConn = name;
    S.expanded.add(name);
    S.connData = null;
    try {
      S.connData = await api('GET', `/api/connections/${encodeURIComponent(name)}`);
    } catch(e) {
      S.connData = S.connections.find(c => c.name === name) || {};
    }
    $('toolbarConnName').textContent = name;
    $('toolbarConnName').style.color = '';
    // Update active query connection if it's new
    if (!S.activeQuery.connection_name) {
      S.activeQuery.connection_name = name;
    }
  }
  renderSidebar();
}

// ─────────────────────────────────────────────
// Query management
// ─────────────────────────────────────────────
function loadQuery(id) {
  const q = S.queries.find(q => q.id === id);
  if (!q) return;
  S.activeQuery = {...q};
  $('editor').value = q.sql || '';
  $('queryTitleInput').value = q.name;
  // Select format
  const fmtEl = $('fmtSelect');
  if (q.format) fmtEl.value = q.format;
  renderSidebar();
}

function newQueryFor(connName) {
  S.activeQuery = {id:'', name:'New Query', connection_name: connName, sql:'', format: $('fmtSelect').value || 'csv', output_dir: ''};
  $('editor').value = '';
  $('queryTitleInput').value = 'New Query';
  if (S.activeConn !== connName) selectConn(connName);
  else renderSidebar();
}

async function saveCurrentQuery() {
  const connName = S.activeConn || S.activeQuery.connection_name;
  if (!connName) { toast('Select a connection first', 'err'); return; }
  S.activeQuery.sql = $('editor').value;
  S.activeQuery.name = $('queryTitleInput').value.trim() || 'New Query';
  S.activeQuery.connection_name = connName;
  S.activeQuery.format = $('fmtSelect').value;
  try {
    const res = await api('POST', '/api/queries', S.activeQuery);
    S.activeQuery.id = res.id;
    S.queries = await api('GET', '/api/queries');
    renderSidebar();
    toast('Query saved', 'ok');
  } catch(e) { toast('Save failed: ' + e.message, 'err'); }
}

async function deleteQuery(id) {
  if (!confirm('Delete this query?')) return;
  await api('DELETE', `/api/queries/${id}`);
  if (S.activeQuery.id === id) {
    S.activeQuery = {id:'', name:'New Query', connection_name: S.activeConn||'', sql:'', format:'csv', output_dir:''};
    $('editor').value = '';
    $('queryTitleInput').value = 'New Query';
  }
  S.queries = await api('GET', '/api/queries');
  renderSidebar();
  toast('Query deleted');
}

// ─────────────────────────────────────────────
// Connection modal
// ─────────────────────────────────────────────
function openConnModal(editName) {
  S.editingConn = editName || null;
  $('connModalTitle').textContent = editName ? `Edit: ${editName}` : 'New Connection';
  $('testStatus').textContent = '';

  if (editName) {
    const conn = S.connections.find(c => c.name === editName) || {};
    $('mName').value = conn.name || '';
    $('mUrl').value  = conn.url  || '';
    $('mHost').value = conn.host || '';
    $('mPort').value = conn.port || 8443;
    $('mScheme').value = conn.http_scheme || 'https';
    $('mUser').value = conn.user || '';
    $('mCatalog').value = conn.catalog || '';
    $('mSchema').value = conn.schema || '';
    $('mInsecure').checked = conn.verify === false;
    $('mPassword').value = '';
    // Load password from backend
    api('GET', `/api/connections/${encodeURIComponent(editName)}`)
      .then(d => { if (d.password) $('mPassword').value = d.password; })
      .catch(() => {});
  } else {
    ['mName','mUrl','mHost','mUser','mCatalog','mSchema','mPassword'].forEach(id => $(id).value = '');
    $('mPort').value = 8443;
    $('mScheme').value = 'https';
    $('mInsecure').checked = false;
  }
  $('connModal').classList.add('open');
  setTimeout(() => $('mName').focus(), 50);
}

function closeConnModal() { $('connModal').classList.remove('open'); }

function modalPayload() {
  return {
    name: $('mName').value.trim(),
    url:  $('mUrl').value.trim(),
    host: $('mHost').value.trim(),
    port: parseInt($('mPort').value) || null,
    user: $('mUser').value.trim(),
    password: $('mPassword').value,
    catalog: $('mCatalog').value.trim(),
    schema: $('mSchema').value.trim(),
    http_scheme: $('mScheme').value,
    verify: !$('mInsecure').checked,
    sql: 'SELECT 1',
  };
}

$('btnNewConn').onclick = () => openConnModal(null);
$('connModalClose').onclick = closeConnModal;
$('connModalCancel').onclick = closeConnModal;
$('connModal').addEventListener('click', e => { if (e.target === $('connModal')) closeConnModal(); });

$('btnTestConn').onclick = async () => {
  const p = modalPayload();
  $('testStatus').textContent = 'Testing…';
  $('testStatus').className = 'test-status';
  try {
    const r = await api('POST', '/api/test', p);
    $('testStatus').textContent = r.ok ? '✓ Connected' : '✗ ' + r.error;
    $('testStatus').className = 'test-status ' + (r.ok ? 'ok' : 'err');
  } catch(e) {
    $('testStatus').textContent = '✗ ' + e.message;
    $('testStatus').className = 'test-status err';
  }
};

$('btnSaveConn').onclick = async () => {
  const name = $('mName').value.trim();
  if (!name) { $('mName').focus(); return; }
  try {
    await api('POST', '/api/connections', modalPayload());
    S.connections = await api('GET', '/api/connections');
    renderSidebar();
    closeConnModal();
    toast(`Connection "${name}" saved`, 'ok');
    // Auto-select the saved connection
    if (!S.activeConn) selectConn(name);
  } catch(e) { toast('Save failed: ' + e.message, 'err'); }
};

async function deleteConn(name) {
  if (!confirm(`Delete connection "${name}"? Saved queries for this connection will remain.`)) return;
  await api('DELETE', `/api/connections/${encodeURIComponent(name)}`);
  if (S.activeConn === name) {
    S.activeConn = null; S.connData = null;
    $('toolbarConnName').textContent = 'No connection';
    $('toolbarConnName').style.color = 'var(--dim)';
  }
  S.expanded.delete(name);
  S.connections = await api('GET', '/api/connections');
  renderSidebar();
  toast(`Connection "${name}" deleted`);
}

// URL ↔ field sync in modal
$('mUrl').addEventListener('blur', () => {
  const raw = $('mUrl').value.trim();
  if (!raw) return;
  try {
    const u = new URL(/^https?:\/\//i.test(raw) ? raw : 'https://' + raw);
    $('mScheme').value = u.protocol.replace(':','');
    $('mHost').value = u.hostname;
    $('mPort').value = u.port || (u.protocol === 'https:' ? 8443 : 8080);
    if (u.username) $('mUser').value = decodeURIComponent(u.username);
    if (u.password) $('mPassword').value = decodeURIComponent(u.password);
    const seg = u.pathname.split('/').filter(Boolean);
    if (seg[0]) $('mCatalog').value = seg[0];
    if (seg[1]) $('mSchema').value = seg[1];
  } catch {}
});

['mHost','mPort','mScheme'].forEach(id => $(id).addEventListener('change', () => {
  const h = $('mHost').value.trim();
  if (!h) return;
  const s = $('mScheme').value, p = $('mPort').value;
  $('mUrl').value = `${s}://${h}:${p}` +
    ($('mUser').value ? '' : '');
}));

// ─────────────────────────────────────────────
// Format + export options
// ─────────────────────────────────────────────
const FORMAT_OPTS = {
  txt:  [['delimiter','Delimiter','text','\\t'],['null_text','NULL as','text',''],['header','Header row','check',true]],
  csv:  [['delimiter','Delimiter','text',','],['null_text','NULL as','text',''],['header','Header row','check',true],['bom','UTF-8 BOM','check',false]],
  json: [['jsonl','Newline-delimited (jsonl)','check',false]],
  xml:  [], html: [],
  sql:  [['sql_table','Target table','text','']],
  xls:  [['sheet','Sheet name','text','Sheet1']],
  xlsx: [['sheet','Sheet name','text','Sheet1']],
  dbf:  [['dbf_char_width','Max char width','number',254]],
};

function renderFmtOpts() {
  const fmt = $('fmtSelect').value;
  const body = $('fmtOpts'); body.innerHTML = '';
  for (const [id,label,type,def] of (FORMAT_OPTS[fmt]||[])) {
    const w = document.createElement('div');
    w.className = 'ep-field';
    if (type === 'check') {
      w.innerHTML = `<div class="ep-row"><input type="checkbox" id="fo_${id}" ${def?'checked':''}><label for="fo_${id}" style="font-size:12.5px;color:var(--text)">${label}</label></div>`;
    } else {
      w.innerHTML = `<div class="ep-label">${label}</div><input type="${type}" class="ep-input" id="fo_${id}" value="${def}" style="width:${type==='number'?'80':'130'}px">`;
    }
    body.appendChild(w);
  }
}

$('fmtSelect').addEventListener('change', renderFmtOpts);

function getFormatOpts() {
  const fmt = $('fmtSelect').value;
  const opts = {};
  for (const [id,,type] of (FORMAT_OPTS[fmt]||[])) {
    const el = $('fo_' + id);
    if (!el) continue;
    opts[id] = type === 'check' ? el.checked : (type === 'number' ? +el.value : el.value);
  }
  if (opts.delimiter === '\\t') opts.delimiter = '\t';
  return opts;
}

$('btnExportOpts').onclick = () => {
  const panel = $('exportPanel');
  panel.classList.toggle('open');
};

// ─────────────────────────────────────────────
// Build request payload
// ─────────────────────────────────────────────
function buildPayload(sql) {
  const conn = S.connData || {};
  const fmtOpts = getFormatOpts();
  return {
    url: conn.url || '',
    host: conn.host || '',
    port: conn.port || null,
    user: conn.user || '',
    password: conn.password || '',
    catalog: conn.catalog || '',
    schema: conn.schema || '',
    http_scheme: conn.http_scheme || 'https',
    verify: conn.verify !== false,
    sql: sql,
    format: $('fmtSelect').value,
    name: $('epName').value.trim() || $('queryTitleInput').value.trim() || 'export',
    output_dir: $('epOutdir').value.trim(),
    batch_size: +$('epBatch').value || 10000,
    rows_per_file: $('epSplit').value ? +$('epSplit').value : null,
    retries: +$('epRetries').value || 5,
    ...fmtOpts,
  };
}

// ─────────────────────────────────────────────
// Run / Preview
// ─────────────────────────────────────────────
$('btnPreview').onclick = runPreview;

async function runPreview() {
  if (!S.connData) { toast('Select a connection first', 'err'); return; }
  const sql = $('editor').value.trim();
  if (!sql) { toast('Write a query first', 'err'); return; }

  $('btnPreview').disabled = true;
  setStatus('Running query…', 'running');
  $('resultsCount').textContent = '';
  $('previewHead').innerHTML = '';
  $('previewBody').innerHTML = '<tr><td colspan="99"><div class="empty-state">Running…</div></td></tr>';

  try {
    const res = await api('POST', '/api/preview', buildPayload(sql));
    if (res.ok) {
      renderPreview(res.columns, res.rows);
      const n = res.rows.length;
      $('resultsCount').textContent = `${n.toLocaleString()} rows${n >= 500 ? ' (first 500)' : ''}`;
      setStatus(`Preview: ${n.toLocaleString()} rows`, 'ok');
    } else {
      $('previewBody').innerHTML = `<tr><td colspan="99"><div class="empty-state" style="color:var(--err)">${esc(res.error)}</div></td></tr>`;
      setStatus(res.error, 'err');
    }
  } catch(e) {
    $('previewBody').innerHTML = `<tr><td colspan="99"><div class="empty-state" style="color:var(--err)">${esc(e.message)}</div></td></tr>`;
    setStatus(e.message, 'err');
  } finally {
    $('btnPreview').disabled = false;
  }
}

function esc(s) {
  return String(s||'').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function renderPreview(columns, rows) {
  $('previewHead').innerHTML = '<tr>' + columns.map(c => `<th>${esc(c)}</th>`).join('') + '</tr>';
  $('previewBody').innerHTML = rows.length
    ? rows.map(r => '<tr>' + r.map(v => v === null
        ? '<td class="null-cell">NULL</td>'
        : `<td title="${esc(v)}">${esc(v)}</td>`).join('') + '</tr>').join('')
    : '<tr><td colspan="99"><div class="empty-state">Query returned no rows.</div></td></tr>';
}

// ─────────────────────────────────────────────
// Export
// ─────────────────────────────────────────────
$('btnExport').onclick = async () => {
  if (!S.connData) { toast('Select a connection first', 'err'); return; }
  const sql = $('editor').value.trim();
  if (!sql) { toast('Write a query first', 'err'); return; }

  $('btnExport').disabled = true;
  $('btnCancel').disabled = false;
  $('btnDownload').disabled = true;
  setStatus('Starting export…', 'running');

  try {
    const res = await api('POST', '/api/export', buildPayload(sql));
    S.job = res.id;
    pollJob();
  } catch(e) {
    finishJob();
    setStatus('Export failed: ' + e.message, 'err');
    toast('Export failed', 'err');
  }
};

function finishJob() {
  clearTimeout(S.jobTimer);
  $('btnExport').disabled = false;
  $('btnCancel').disabled = true;
  $('progressBar').style.display = 'none';
}

$('btnCancel').onclick = async () => {
  if (S.job) { await api('POST', `/api/jobs/${S.job}/cancel`, {}).catch(()=>{}); }
};

$('btnDownload').onclick = () => {
  if (S.job) location.href = `/api/jobs/${S.job}/download`;
};

async function pollJob() {
  if (!S.job) return;
  const s = await api('GET', `/api/jobs/${S.job}`).catch(() => null);
  if (!s) { finishJob(); setStatus('Poll error', 'err'); return; }

  $('statusRows').textContent = s.rows ? `${s.rows.toLocaleString()} rows` : '';
  setStatus(`Exporting… ${s.rows ? s.rows.toLocaleString() + ' rows' : ''}`, 'running');

  if (s.status === 'running') {
    S.jobTimer = setTimeout(pollJob, 500);
    return;
  }
  finishJob();
  if (s.status === 'error') {
    setStatus(s.error, 'err');
    toast('Export failed', 'err');
    return;
  }
  s.warnings.forEach(w => toast(w, ''));
  const label = s.status === 'cancelled' ? 'Export cancelled (partial)' : 'Export done!';
  setStatus(label, s.status === 'cancelled' ? '' : 'ok');
  toast(label, s.status === 'cancelled' ? '' : 'ok');
  $('btnDownload').disabled = !s.download;
}

// ─────────────────────────────────────────────
// Toolbar: save query
// ─────────────────────────────────────────────
$('btnSaveQuery').onclick = saveCurrentQuery;
document.addEventListener('keydown', e => {
  if ((e.metaKey || e.ctrlKey) && e.key === 's') { e.preventDefault(); saveCurrentQuery(); }
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); runPreview(); }
});

// ─────────────────────────────────────────────
// Editor resize handle
// ─────────────────────────────────────────────
{
  const resizer = $('editorResizer');
  const wrap = $('editorWrap');
  let startY, startH;
  resizer.addEventListener('mousedown', e => {
    startY = e.clientY; startH = wrap.offsetHeight;
    resizer.classList.add('dragging');
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
    e.preventDefault();
  });
  function onMove(e) {
    const h = Math.max(80, Math.min(startH + e.clientY - startY, window.innerHeight * .8));
    wrap.style.height = h + 'px';
  }
  function onUp() {
    resizer.classList.remove('dragging');
    document.removeEventListener('mousemove', onMove);
    document.removeEventListener('mouseup', onUp);
  }
}

// ─────────────────────────────────────────────
// Quit
// ─────────────────────────────────────────────
$('btnQuit').onclick = () => {
  fetch('/api/quit', {method:'POST'}).catch(()=>{});
  document.body.innerHTML = '<p style="padding:60px;color:var(--muted);font:14px -apple-system">QueryHive server stopped. You can close this tab.</p>';
};

// ─────────────────────────────────────────────
// Init
// ─────────────────────────────────────────────
(async function init() {
  // Load formats
  try {
    S.formats = await api('GET', '/api/formats');
    $('fmtSelect').innerHTML = S.formats.map(f =>
      `<option value="${f.id}">${f.label.replace(/\s*\(\*.*\)$/,'')}</option>`
    ).join('');
    renderFmtOpts();
  } catch {}

  // Load default output dir
  try {
    const d = await api('GET', '/api/defaults');
    $('epOutdir').value = d.output_dir || '';
  } catch {}

  // Load connections
  try { S.connections = await api('GET', '/api/connections'); } catch {}

  // Load queries
  try { S.queries = await api('GET', '/api/queries'); } catch {}

  renderSidebar();
})();

// ═══════════════════════════════════════════════════════
// EXPORT WIZARD
// ═══════════════════════════════════════════════════════
const FORMAT_LIST = [
  {id:'txt',  label:'Text file (*.txt)'},
  {id:'csv',  label:'CSV file (*.csv)'},
  {id:'json', label:'JSON file (*.json)'},
  {id:'xml',  label:'XML file (*.xml)'},
  {id:'html', label:'HTML file (*.htm;*.html)'},
  {id:'sql',  label:'SQL script file (*.sql)'},
  {id:'xls',  label:'Excel file (*.xls)'},
  {id:'xlsx', label:'Excel file (2007 or later) (*.xlsx)'},
  {id:'dbf',  label:'DBase file (*.dbf)'},
];

const WZ_OPTS = {
  txt:  [{id:'delimiter',label:'Delimiter',type:'text',def:'\t'},{id:'null_text',label:'NULL as',type:'text',def:''},{id:'header',label:'Header row',type:'check',def:true}],
  csv:  [{id:'delimiter',label:'Delimiter',type:'text',def:','},{id:'null_text',label:'NULL as',type:'text',def:''},{id:'header',label:'Header row',type:'check',def:true},{id:'bom',label:'UTF-8 BOM',type:'check',def:false}],
  json: [{id:'jsonl',label:'Newline-delimited (jsonl)',type:'check',def:false}],
  xml:  [],html:[],
  sql:  [{id:'sql_table',label:'Target table',type:'text',def:''}],
  xls:  [{id:'sheet',label:'Sheet name',type:'text',def:'Sheet1'}],
  xlsx: [{id:'sheet',label:'Sheet name',type:'text',def:'Sheet1'}],
  dbf:  [{id:'dbf_char_width',label:'Max char width',type:'number',def:254}],
};

const WZ = {
  step: 1, format: 'csv',
  folder: '', filename: 'export', encoding: 'utf-8', addTs: false, tsFormat: 'YYYYMMDD',
  columns: null,   // null = all; string[] = selected
  job: null, jobTimer: null,
};

function wOpen() {
  // Build format list
  $('wFormatList').innerHTML = FORMAT_LIST.map(f =>
    `<label class="format-opt">
      <input type="radio" name="wFmt" value="${f.id}" ${f.id===WZ.format?'checked':''}>
      <label>${f.label}</label>
    </label>`
  ).join('');

  // Pre-fill filename from query title
  const name = ($('queryTitleInput').value||'export').replace(/[^\w\-]/g,'_').toLowerCase();
  $('wFilename').value = name;
  WZ.filename = name;

  // Set default folder
  WZ.folder = $('epOutdir').value || '';
  if (!WZ.folder) {
    api('GET','/api/defaults').then(d => { WZ.folder = d.output_dir||''; $('wFolderDisplay').textContent = WZ.folder; }).catch(()=>{});
  } else {
    $('wFolderDisplay').textContent = WZ.folder;
  }

  wGoTo(1);
  $('wizardModal').classList.add('open');
}

function wClose() {
  clearTimeout(WZ.jobTimer);
  $('wizardModal').classList.remove('open');
}

$('wClose').onclick = wClose;
$('wizardModal').addEventListener('click', e => { if (e.target === $('wizardModal')) wClose(); });

function wGoTo(step) {
  WZ.step = step;
  for (let i=1;i<=5;i++) {
    const c = $('wc'+i);
    if (c) c.style.display = i===step ? '' : 'none';
    const p = $('wsp'+i);
    if (p) p.className = 'wz-step-pill' + (i<step?' done':i===step?' active':'');
  }
  // Buttons
  $('wBtnBack').style.display  = step>1 && step<5 ? '' : 'none';
  $('wBtnNext').style.display  = step<4 ? '' : 'none';
  $('wBtnStart').style.display = step===4 ? '' : 'none';
  $('wBtnClose2').style.display= step===5 ? '' : 'none';
  $('wBtnDownload').style.display = 'none';

  if (step===3) wBuildColList();
  if (step===4) wBuildOpts();
}

$('wBtnBack').onclick = () => wGoTo(WZ.step-1);
$('wBtnNext').onclick = () => {
  if (WZ.step===1) {
    const r = document.querySelector('input[name=wFmt]:checked');
    if (r) WZ.format = r.value;
  }
  if (WZ.step===2) {
    WZ.folder = $('wFolderDisplay').textContent.trim();
    WZ.filename = $('wFilename').value.trim() || 'export';
    WZ.encoding = $('wEncoding').value;
    WZ.addTs = $('wAddTs').checked;
    WZ.tsFormat = $('wTsFormat').value;
  }
  wGoTo(WZ.step+1);
};

// ─── STEP 2 FOLDER BROWSER ───────────────────────
let dirBrowserOpen = false;

$('wBtnBrowse').onclick = async () => {
  dirBrowserOpen = !dirBrowserOpen;
  $('dirBrowser').style.display = dirBrowserOpen ? '' : 'none';
  if (dirBrowserOpen) await wDirLoad(WZ.folder || '');
};

$('wDirUp').onclick = async () => {
  const cur = $('wDirCrumb').dataset.path || '';
  if (!cur) return;
  // Get parent from backend
  const r = await api('GET', `/api/browse?path=${encodeURIComponent(cur)}`).catch(()=>null);
  if (r && r.parent) await wDirLoad(r.parent);
};

async function wDirLoad(path) {
  $('wDirList').innerHTML = '<div class="dir-entry" style="color:var(--muted)">Loading…</div>';
  try {
    const r = await api('GET', `/api/browse?path=${encodeURIComponent(path)}`);
    $('wDirCrumb').textContent = r.path;
    $('wDirCrumb').dataset.path = r.path;
    $('wDirList').innerHTML = r.entries.length
      ? r.entries.filter(e=>e.is_dir).map(e =>
          `<div class="dir-entry" data-dpath="${esc(e.path)}" title="${esc(e.path)}">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="#3d8ef0" opacity=".8">
              <path d="M20 6H12l-2-2H4a2 2 0 00-2 2v12a2 2 0 002 2h16a2 2 0 002-2V8a2 2 0 00-2-2z"/>
            </svg>${esc(e.name)}</div>`
        ).join('')
      : '<div class="dir-entry" style="color:var(--muted)">No sub-folders</div>';

    // Click folder to navigate
    $('wDirList').querySelectorAll('[data-dpath]').forEach(el =>
      el.addEventListener('dblclick', () => wDirLoad(el.dataset.dpath))
    );
    // Single click selects folder
    $('wDirList').querySelectorAll('[data-dpath]').forEach(el =>
      el.addEventListener('click', () => {
        $('wDirList').querySelectorAll('.selected').forEach(x=>x.classList.remove('selected'));
        el.classList.add('selected');
        $('wFolderDisplay').textContent = el.dataset.dpath;
      })
    );
  } catch(e) {
    $('wDirList').innerHTML = `<div class="dir-entry" style="color:var(--err)">${esc(e.message)}</div>`;
  }
}

// ─── STEP 3 COLUMN LIST ───────────────────────────
function wBuildColList() {
  const cols = (S.previewData && S.previewData.columns) || [];
  const list = $('wColList');
  if (!cols.length) {
    list.innerHTML = '<div class="dir-entry" style="color:var(--muted);padding:20px">Run query first to see column list. All columns will be exported.</div>';
    WZ.columns = null;
    return;
  }
  // Init selection
  if (!WZ.columns) WZ.columns = [...cols];
  list.innerHTML = cols.map((c,i) =>
    `<div class="col-item">
      <input type="checkbox" id="wc_${i}" value="${esc(c)}" ${WZ.columns.includes(c)?'checked':''}>
      <label for="wc_${i}">${esc(c)}</label>
    </div>`
  ).join('');
  // All-fields checkbox sync
  $('wAllFields').checked = WZ.columns.length === cols.length;
}

$('wSelAll').onclick = () => {
  const cols = (S.previewData&&S.previewData.columns)||[];
  WZ.columns = [...cols];
  document.querySelectorAll('#wColList input[type=checkbox]').forEach(cb=>cb.checked=true);
  $('wAllFields').checked = true;
};
$('wDeselAll').onclick = () => {
  WZ.columns = [];
  document.querySelectorAll('#wColList input[type=checkbox]').forEach(cb=>cb.checked=false);
  $('wAllFields').checked = false;
};
$('wAllFields').onchange = e => {
  if (e.target.checked) $('wSelAll').click();
  else $('wDeselAll').click();
};
$('wColList').addEventListener('change', () => {
  const cols = (S.previewData&&S.previewData.columns)||[];
  WZ.columns = cols.filter((_,i) => {
    const cb = $('wc_'+i);
    return cb && cb.checked;
  });
  $('wAllFields').checked = WZ.columns.length===cols.length;
});

// ─── STEP 4 FORMAT OPTIONS ────────────────────────
function wBuildOpts() {
  const opts = WZ_OPTS[WZ.format]||[];
  $('wFmtOptsGrid').innerHTML = opts.length
    ? opts.map(o => {
        if (o.type==='check')
          return `<div class="wz-opt-row">
            <label>${o.label}</label>
            <label class="wz-check"><input type="checkbox" id="wo_${o.id}" ${o.def?'checked':''}></label>
          </div>`;
        return `<div class="wz-opt-row">
          <label>${o.label}</label>
          <input type="${o.type}" id="wo_${o.id}" value="${o.def}">
        </div>`;
      }).join('')
    : '<p style="color:var(--muted);font-size:13px">No additional options for this format.</p>';
  $('wBtnStart').disabled = false;
}

function wGetOpts() {
  const out = {};
  for (const o of (WZ_OPTS[WZ.format]||[])) {
    const el = $('wo_'+o.id);
    if (!el) continue;
    let v = o.type==='check' ? el.checked : (o.type==='number' ? +el.value : el.value);
    if (o.id==='delimiter' && v==='\\t') v = '\t';
    out[o.id] = v;
  }
  return out;
}

// ─── STEP 5 PROGRESS ─────────────────────────────
$('wBtnStart').onclick = async () => {
  wGoTo(5);
  $('wProgressFill').classList.remove('done');
  $('wBtnDownload').style.display = 'none';

  // Build the effective filename with optional timestamp
  let fname = WZ.filename;
  if (WZ.addTs) {
    const now = new Date();
    const ts = WZ.tsFormat
      .replace('YYYY', now.getFullYear())
      .replace('MM', String(now.getMonth()+1).padStart(2,'0'))
      .replace('DD', String(now.getDate()).padStart(2,'0'))
      .replace('HH', String(now.getHours()).padStart(2,'0'))
      .replace('mm', String(now.getMinutes()).padStart(2,'0'))
      .replace('ss', String(now.getSeconds()).padStart(2,'0'));
    fname = fname + '_' + ts;
  }

  // Determine which columns
  const allCols = (S.previewData&&S.previewData.columns)||[];
  const selCols = WZ.columns && WZ.columns.length && WZ.columns.length!==allCols.length
    ? WZ.columns : [];

  const conn = S.connData||{};
  const payload = {
    url: conn.url||'', host: conn.host||'', port: conn.port||null,
    user: conn.user||'', password: conn.password||'',
    catalog: conn.catalog||'', schema: conn.schema||'',
    http_scheme: conn.http_scheme||'https', verify: conn.verify!==false,
    sql: $('editor').value.trim(),
    format: WZ.format, name: fname,
    output_dir: WZ.folder,
    encoding: WZ.encoding,
    columns: selCols,
    batch_size: +($('epBatch').value||10000),
    retries: +($('epRetries').value||5),
    ...wGetOpts(),
  };

  $('wProgressTitle').textContent = 'Exporting…';
  wLog('Starting export to ' + (WZ.folder||'temp folder') + '…');
  wStats({rows:0, status:'running'});

  try {
    const r = await api('POST','/api/export', payload);
    WZ.job = r.id;
    wPoll();
  } catch(e) {
    $('wProgressTitle').textContent = 'Export failed';
    wLog('Error: '+e.message);
    $('wProgressFill').style.width='100%';
    $('wProgressFill').style.background='var(--err)';
    $('wProgressFill').classList.remove('done');
    $('wBtnClose2').style.display='';
  }
};

function wLog(msg) {
  const el = $('wLog');
  el.textContent += (el.textContent?'\n':'') + msg;
  el.scrollTop = el.scrollHeight;
}
function wStats({rows, status, qid}) {
  $('wStats').innerHTML = [
    qid ? `<div class="wz-stat"><span class="wz-stat-key">Query ID</span><span class="wz-stat-val">${esc(qid)}</span></div>` : '',
    `<div class="wz-stat"><span class="wz-stat-key">Rows exported</span><span class="wz-stat-val">${(rows||0).toLocaleString()}</span></div>`,
    `<div class="wz-stat"><span class="wz-stat-key">Status</span><span class="wz-stat-val">${esc(status)}</span></div>`,
  ].join('');
}

async function wPoll() {
  if (!WZ.job) return;
  const s = await api('GET',`/api/jobs/${WZ.job}`).catch(()=>null);
  if (!s) { wLog('Lost contact with server.'); return; }
  wStats({rows:s.rows, status:s.status, qid:s.query_id});
  if (s.status==='running') { WZ.jobTimer=setTimeout(wPoll,500); return; }
  // Finished
  $('wProgressFill').classList.add('done');
  if (s.status==='error') {
    $('wProgressTitle').textContent = 'Export failed';
    wLog('Error: '+s.error);
    $('wProgressFill').style.background='var(--err)';
  } else {
    $('wProgressTitle').textContent = s.status==='cancelled' ? 'Export cancelled (partial)' : 'Export complete!';
    wLog(`Done — ${(s.rows||0).toLocaleString()} rows exported.`);
    if (s.warnings&&s.warnings.length) s.warnings.forEach(w=>wLog('Warning: '+w));
    if (s.download) { $('wBtnDownload').style.display=''; WZ.lastJobId=WZ.job; }
  }
  $('wBtnClose2').style.display='';
}

$('wBtnClose2').onclick = wClose;
$('wBtnDownload').onclick = () => {
  if (WZ.lastJobId) location.href=`/api/jobs/${WZ.lastJobId}/download`;
};

// Open wizard from results toolbar
$('btnWizard').onclick = wOpen;

// Preload preview data reference into WZ
const _origRenderPreview = renderPreview;
renderPreview = function(columns, rows) {
  _origRenderPreview(columns, rows);
  S.previewData = {columns, rows};
  $('btnWizard').disabled = false;
  WZ.columns = null; // reset column selection on new results
};


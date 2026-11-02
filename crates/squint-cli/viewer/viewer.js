// The static report viewer (SPEC "Report"): loads report.json from the directory it is served
// from, shows every image the model saw with every location overlaid in raw coordinates of its
// root image through the frame tree, every tool result, the checklist, the aggregation rows and
// the decision table. No build step; requests go only to the server that served this page.
// An exported page (`squint report export`) carries the same files in `SQUINT_BUNDLE` and needs
// no server at all.
'use strict';

const SUPPORTED_REPORT_VERSIONS = [1];

/// The decision directory's files: from the export's bundle when there is one, else the server.
const BUNDLE = globalThis.SQUINT_BUNDLE || null;

async function getText(path) {
  if (BUNDLE) {
    const f = BUNDLE.files[path];
    if (!f) throw new Error(`${path} is not in this export`);
    return f.text !== undefined ? f.text : atob(f.base64);
  }
  const resp = await fetch(path, { cache: 'no-store' });
  if (!resp.ok) throw new Error(`${resp.status} ${resp.statusText}`);
  return resp.text();
}

const $app = document.getElementById('app');
const $status = document.getElementById('status');

/// Styles go through CSSOM, never a `style` attribute: the server's CSP (`default-src 'self'`,
/// no `'unsafe-inline'`) blocks inline style attributes but not CSSOM. `style` takes an object of
/// CSS properties; fixed looks belong in viewer.css as classes.
function applyStyle(node, style) {
  if (typeof style !== 'object') throw new TypeError('style takes an object of CSS properties, not a string');
  for (const [prop, value] of Object.entries(style)) node.style.setProperty(prop, value);
}

function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === null || v === undefined) continue;
    if (k === 'class') node.className = v;
    else if (k === 'text') node.textContent = v;
    else if (k === 'style') applyStyle(node, v);
    else node.setAttribute(k, v);
  }
  for (const c of children) {
    if (c === null || c === undefined) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

function svgEl(tag, attrs) {
  const node = document.createElementNS('http://www.w3.org/2000/svg', tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k === 'style') applyStyle(node, v);
    else node.setAttribute(k, v);
  }
  return node;
}

function json(v) {
  return v === undefined ? '' : JSON.stringify(v, null, 1).replace(/\n\s*/g, ' ');
}

function cell(v) {
  if (v === null || v === undefined) return el('td', { text: '' });
  if (typeof v === 'object') return el('td', { class: 'json', text: json(v) });
  return el('td', { text: String(v) });
}

/// A table over an array of objects: the columns are the union of keys, in first-seen order.
function table(rows, columns) {
  if (!rows || rows.length === 0) return el('p', { class: 'mono', text: '(none)' });
  const cols = columns || [...rows.reduce((s, r) => { Object.keys(r).forEach(k => s.add(k)); return s; }, new Set())];
  const t = el('table');
  t.append(el('thead', null, el('tr', null, ...cols.map(c => el('th', { text: c })))));
  const body = el('tbody');
  for (const r of rows) body.append(el('tr', null, ...cols.map(c => cell(r[c]))));
  t.append(body);
  return t;
}

function fact(label, value) {
  return el('div', null, el('b', { text: label }), value instanceof Node ? value : el('span', { text: value === null || value === undefined ? '—' : String(value) }));
}

function badge(verdict) {
  const v = verdict || 'none';
  return el('span', { class: `badge ${v}`, text: v });
}

function money(x) {
  return x === null || x === undefined ? '—' : `$${Number(x).toFixed(4)}`;
}

function artifactPath(run, ref) {
  return ref ? `run-${run}/artifacts/${ref.sha256}` : null;
}

function artifactUrl(run, ref) {
  const path = artifactPath(run, ref);
  if (!path || !BUNDLE) return path;
  const f = BUNDLE.files[path];
  if (!f) return null;
  return f.text !== undefined
    ? `data:${f.type};charset=utf-8,${encodeURIComponent(f.text)}`
    : `data:${f.type};base64,${f.base64}`;
}

function artifactLink(run, ref, text) {
  if (!ref) return el('span', { text: '—' });
  const a = el('a', { href: artifactUrl(run, ref), target: '_blank', rel: 'noopener', text: text || ref.sha256.slice(0, 12) });
  return el('span', null, a, ref.truncated ? el('span', { class: 'warn', text: ' (truncated)' }) : null);
}

// ---- locations and frames --------------------------------------------------------------------

/// Maps a location to the raw frame of its root image through the frame tree: bbox crops by their
/// box, view crops by the sphere geometry (`geometry.js`), and a `view` location by the footprint
/// of the view it names on its own panorama. Returns { frame, kind, bbox, polygon, partial } or
/// null when the frame is unknown or a mapping is missing.
function toRoot(frames, loc) {
  const geometry = globalThis.squintGeometry;
  let frame = loc.frame;
  let kind = loc.kind;
  let bbox = loc.bbox ? [...loc.bbox] : null;
  let polygon = loc.polygon ? loc.polygon.map(p => [...p]) : null;
  let partial = false;
  if (kind === 'view') {
    if (!geometry) return null;
    const fp = geometry.mapThroughView({ kind: 'whole_image' }, { yaw: loc.yaw, pitch: loc.pitch, fov: loc.fov });
    kind = fp.kind; bbox = fp.bbox || null; polygon = fp.polygon || null; partial = fp.clipped;
  }
  let guard = 0;
  while (frames[frame] && frames[frame].parent) {
    if (++guard > 64) return null;
    const f = frames[frame];
    if (f.mapping && (f.mapping.view || f.mapping.homography || f.mapping.rotation)) {
      if (!geometry) return null;
      // A view crop maps into its parent; an aligned frame maps back to its own image's raw frame.
      const fp = f.mapping.view ? geometry.mapThroughView({ kind, bbox, polygon }, f.mapping.view)
        : f.mapping.homography ? geometry.mapThroughHomography({ kind, bbox, polygon }, f.mapping.homography)
        : geometry.mapThroughRotation({ kind, bbox, polygon }, f.mapping.rotation);
      if (!fp) return null;
      kind = fp.kind; bbox = fp.bbox || null; polygon = fp.polygon || null; partial = partial || fp.clipped;
      frame = f.mapping.view ? f.parent : `raw:${f.root}`;
      continue;
    }
    if (!f.mapping || !f.mapping.bbox) return null;
    const [x0, y0, x1, y1] = f.mapping.bbox;
    const w = x1 - x0, h = y1 - y0;
    const map = ([x, y]) => [x0 + x * w, y0 + y * h];
    if (kind === 'whole_image') { kind = 'bbox'; bbox = [x0, y0, x1, y1]; }
    else if (bbox) { const a = map([bbox[0], bbox[1]]), b = map([bbox[2], bbox[3]]); bbox = [a[0], a[1], b[0], b[1]]; }
    else if (polygon) polygon = polygon.map(map);
    frame = f.parent;
  }
  if (!frames[frame]) return null;
  const clip = v => { const c = Math.min(1, Math.max(0, v)); if (c !== v) partial = true; return c; };
  if (bbox) bbox = bbox.map(clip);
  if (polygon) polygon = polygon.map(([x, y]) => [clip(x), clip(y)]);
  return { frame, kind, bbox, polygon, partial };
}

/// Draws one location (already in the coordinates of the panel's frame) on an overlay.
function draw(svg, loc, cls, title) {
  const classes = [cls, loc.partial ? 'partial' : '', loc.kind === 'absent' ? 'absent' : ''].join(' ').trim();
  let shape = null;
  if (loc.kind === 'bbox' && loc.bbox) {
    const [x0, y0, x1, y1] = loc.bbox;
    shape = svgEl('rect', { x: x0, y: y0, width: Math.max(0, x1 - x0), height: Math.max(0, y1 - y0), class: classes });
  } else if (loc.kind === 'polygon' && loc.polygon) {
    shape = svgEl('polygon', { points: loc.polygon.map(p => p.join(',')).join(' '), class: classes });
  } else if (loc.kind === 'whole_image' || loc.kind === 'absent') {
    shape = svgEl('rect', { x: 0.004, y: 0.004, width: 0.992, height: 0.992, class: classes });
  }
  if (!shape) return;
  shape.setAttribute('data-kind', loc.kind);
  shape.append(svgEl('title'), document.createTextNode(''));
  shape.querySelector('title').textContent = title;
  svg.append(shape);
}

// ---- sections ----------------------------------------------------------------------------------

function header(r) {
  const d = r.decision || {};
  const facts = el('div', { class: 'facts' },
    fact('verdict', badge(r.verdict)),
    fact('exit code', r.exit_code),
    fact('use case', r.use_case),
    fact('provider', r.provider),
    fact('tools', (r.tools || []).join(', ') || 'none'),
    fact('decision runs', `${r.decision_runs} asked, ${(r.runs || []).length} found`),
    fact('runs agree', d.agree === null || d.agree === undefined ? '—' : String(d.agree)),
    fact('policy', `unsure → ${r.policy && r.policy.on_unsure}, error → ${r.policy && r.policy.on_error}`),
    fact('spent', money(r.cost && r.cost.spent_usd)),
    fact('envelope', r.cost && r.cost.envelope ? money(r.cost.envelope.usd) : 'not planned'),
    fact('cap', money(r.cost && r.cost.cap_usd)),
    fact('cache', r.cache && (r.cache.status || json(r.cache))),
    fact('prompt version', r.prompt_version),
    fact('harness', r.harness_version),
    fact('report version', r.report_version),
  );
  const sec = el('section', null, el('h2', { text: 'Decision' }), facts);
  if (d.disagreements && d.disagreements.length) {
    sec.append(el('p', { class: 'warn', text: `disagreements: ${d.disagreements.join('; ')}` }));
  }
  const rows = (r.runs || []).map(run => ({
    run: run.run, terminal: run.terminal, verdict: run.verdict, error: run.error ? `${run.error.class}: ${run.error.message}` : '',
    turns: (run.turns || []).length, tool_calls: (run.tool_calls || []).length, repairs: (run.repairs || []).length, wall_ms: run.wall_ms,
  }));
  sec.append(table(rows));
  if (r.rules) sec.append(el('details', null, el('summary', { text: 'rules' }), el('pre', { class: 'mono', text: JSON.stringify(r.rules, null, 2) })));
  sec.append(el('details', null, el('summary', { text: 'job' }), el('pre', { class: 'mono', text: JSON.stringify(r.job, null, 2) })));
  sec.append(el('details', null, el('summary', { text: 'config as resolved' }), el('pre', { class: 'mono', text: JSON.stringify(r.config, null, 2) })));
  return sec;
}

/// The colour class of a judged location: a violation is red whether it answers a checklist item
/// or is a finding of its own; what was observed or allowed is grey; anything else is blue.
function judgedClass(disposition) {
  if (disposition === 'violation') return 'finding';
  if (disposition === 'observed' || disposition === 'allowed_change') return 'observed';
  return 'item';
}

/// Every location of a run with where it came from: checklist items, findings, tool regions.
/// Items that put the same box on the same frame (one defect answering several obligations) are
/// drawn once, with every item named in its hover text.
async function locations(run) {
  const out = [];
  const same = new Map();
  const judged = (loc, cls, title, root) => {
    const key = `${cls} ${JSON.stringify(loc)}`;
    const seen = same.get(key);
    if (seen) { seen.title += `\n${title}`; return; }
    const entry = { loc, cls, title, root };
    same.set(key, entry);
    out.push(entry);
  };
  for (const item of run.items || []) {
    if (item.location) judged(item.location, judgedClass(item.disposition), `${item.id}: ${item.disposition} (${item.confidence})`, item.root);
  }
  for (const f of run.findings || []) {
    judged(f.location, judgedClass(f.disposition), `finding ${(f.obligation_ids || []).join(',')}: ${f.disposition}`, f.root);
  }
  for (const call of run.tool_calls || []) {
    if (!call.envelope) continue;
    try {
      const env = JSON.parse(await getText(artifactPath(run.run, call.envelope)));
      call._envelope = env;
      for (const region of env.regions || []) {
        if (region.location) out.push({ loc: region.location, cls: 'region', title: `${call.tool} ${region.id}: ${region.label || ''}` });
      }
    } catch (e) {
      call._envelope_error = String(e);
    }
  }
  return out;
}

function imagesSection(run, locs) {
  const frames = run.frames || {};
  const sec = el('section', null, el('h2', { text: `Run ${run.run}: images the model saw` }));
  sec.append(el('p', { class: 'legend' },
    el('span', null, el('i', { class: 'finding' }), 'violation'),
    el('span', null, el('i', { class: 'item' }), 'other answer (held, hidden, can\'t tell)'),
    el('span', null, el('i', { class: 'observed' }), 'observed / allowed'),
    el('span', null, el('i', { class: 'region' }), 'tool region (segmenter)'),
    el('span', { text: 'dashed: absent; dotted: clipped at the parent' })));
  const grid = el('div', { class: 'frames' });
  const frameIds = Object.keys(run.images_sent || {}).sort((a, b) => (a.startsWith('raw:') ? 0 : 1) - (b.startsWith('raw:') ? 0 : 1) || a.localeCompare(b));
  for (const frameId of frameIds) {
    const ref = run.images_sent[frameId];
    const frame = frames[frameId];
    const svg = svgEl('svg', { class: 'overlay', viewBox: '0 0 1 1', preserveAspectRatio: 'none' });
    svg.dataset.frame = frameId;
    const isRaw = frameId.startsWith('raw:');
    for (const { loc, cls, title } of locs) {
      if (loc.frame === frameId && loc.kind !== 'view') draw(svg, { kind: loc.kind, bbox: loc.bbox, polygon: loc.polygon, partial: false }, cls, `${title} (as submitted)`);
      if (isRaw && (loc.frame !== frameId || loc.kind === 'view')) {
        const mapped = toRoot(frames, loc);
        if (mapped && mapped.frame === frameId) draw(svg, mapped, cls, loc.kind === 'view' ? `${title} (view footprint)` : `${title} (from ${loc.frame})`);
      }
    }
    const img = el('img', { src: artifactUrl(run.run, ref), alt: frameId });
    const stage = el('div', { class: 'stage' }, img, svg);
    const dims = frame ? `${frame.width}×${frame.height} ${frame.projection}` : '';
    const mappingText = m => (m && m.bbox ? `[${m.bbox.map(v => v.toFixed(3)).join(', ')}]` : m && m.view ? `view yaw ${m.view.yaw}° pitch ${m.view.pitch}° fov ${m.view.fov}°` : m && m.homography ? 'aligned by homography' : m && m.rotation ? 'aligned by rotation' : '');
    const parent = frame && frame.parent ? ` ⊂ ${frame.parent} ${mappingText(frame.mapping)}` : '';
    grid.append(el('figure', null, stage, el('figcaption', null, el('b', { text: frameId }), ` ${dims}${parent}`, ref.truncated ? el('span', { class: 'warn', text: ' truncated' }) : null)));
  }
  sec.append(grid);
  const unmapped = locs.filter(l => !toRoot(frames, l.loc) && l.loc.kind !== 'absent').map(l => `${l.title} in ${l.loc.frame} (${l.loc.kind})`);
  if (unmapped.length) sec.append(el('p', { class: 'warn', text: `not drawn in raw coordinates (unknown frame or mapping): ${unmapped.join('; ')}` }));
  return sec;
}

function toolsSection(run) {
  const sec = el('section', null, el('h2', { text: `Run ${run.run}: tool results` }));
  const rows = (run.tool_calls || []).map(c => ({
    id: c.id, tool: c.tool, version: c.version, executed: c.executed, status: c.status, reason: c.reason,
    duration_ms: c.duration_ms, arguments: c.arguments, invalid_args: c.invalid_args, added_items: c.added_items, dropped: c.dropped,
    summary: c._envelope ? c._envelope.summary : (c._envelope_error || ''), regions: c._envelope ? (c._envelope.regions || []).length : '',
    envelope: c.envelope,
  }));
  const t = table(rows);
  // Replace the envelope column's JSON with a link.
  const cols = t.querySelectorAll('thead th');
  let idx = -1; cols.forEach((th, i) => { if (th.textContent === 'envelope') idx = i; });
  if (idx >= 0) t.querySelectorAll('tbody tr').forEach((tr, i) => { const td = tr.children[idx]; td.className = ''; td.textContent = ''; td.append(artifactLink(run.run, rows[i].envelope)); });
  sec.append(t);
  return sec;
}

function checklistSection(run) {
  const sec = el('section', null, el('h2', { text: `Run ${run.run}: checklist and submission` }));
  const items = (run.checklist && run.checklist.items) || [];
  const byId = new Map((run.items || []).map(i => [i.id, i]));
  const rows = items.map(c => {
    const sub = byId.get(c.id) || {};
    return { ...c, disposition: sub.disposition, confidence: sub.confidence, reason: sub.reason, allowed_by: sub.allowed_by, location: sub.location };
  });
  sec.append(table(rows));
  if (run.findings && run.findings.length) {
    sec.append(el('h3', { text: 'findings' }));
    sec.append(table(run.findings));
  }
  return sec;
}

function aggregationSection(run) {
  const sec = el('section', null, el('h2', { text: `Run ${run.run}: aggregation` }));
  // A row is a name (`recorded`, `fail`, …) or an object; either way one column per field.
  const spread = (row) => (row && typeof row === 'object') ? row : { row };
  const rows = (run.items || []).map(i => ({ id: i.id, ...spread(i.row) }));
  sec.append(table(rows));
  if (run.finding_rows && run.finding_rows.length) {
    sec.append(el('h3', { text: 'finding rows' }));
    sec.append(table(run.finding_rows.map(spread)));
  }
  return sec;
}

function turnsSection(run) {
  const sec = el('section', null, el('h2', { text: `Run ${run.run}: turns` }));
  const rows = (run.turns || []).map(t => ({
    turn: t.turn, stop_reason: t.stop_reason, tool_calls: t.tool_calls, attempts: t.attempts, cost_usd: t.cost_usd, bound_usd: t.bound_usd,
    max_output_tokens: t.max_output_tokens, usage: t.usage, in_history: t.entered_history, provider_request_id: t.provider_request_id,
    request: t.request, response: t.response,
  }));
  const t = table(rows);
  const cols = [...t.querySelectorAll('thead th')].map(th => th.textContent);
  for (const name of ['request', 'response']) {
    const idx = cols.indexOf(name);
    if (idx < 0) continue;
    t.querySelectorAll('tbody tr').forEach((tr, i) => { const td = tr.children[idx]; td.className = ''; td.textContent = ''; td.append(artifactLink(run.run, rows[i][name])); });
  }
  sec.append(t);
  if (run.repairs && run.repairs.length) { sec.append(el('h3', { text: 'repairs' })); sec.append(table(run.repairs)); }
  if (run.error) sec.append(el('p', { class: 'err', text: `${run.error.class}: ${run.error.message}` }));
  if (run.instances && run.instances.length) {
    sec.append(el('details', null, el('summary', { text: `${run.instances.length} obligation instances` }), table(run.instances.map(i => ({ id: i.id, ...i.instance })))));
  }
  return sec;
}

// ---- timeline -----------------------------------------------------------------------------

/// The run's funnel as a waterfall from its journal: every step with its start, duration,
/// outcome and, for a model turn, its cost (`timeline.js` builds the rows).
async function timelineSection(run) {
  const sec = el('section', null, el('h2', { text: `Run ${run.run}: timeline` }));
  let events;
  try {
    events = squintTimeline.parse(await getText(`run-${run.run}/journal.jsonl`));
  } catch (e) {
    sec.append(el('p', { class: 'mono', text: `no journal: ${e.message}` }));
    return sec;
  }
  const rows = squintTimeline.rows(events);
  const total = Math.max(1, ...rows.map(r => r.start_ms + r.duration_ms));
  const turnsCost = rows.filter(r => r.step === 'turn').reduce((s, r) => s + (r.cost_usd || 0), 0);
  sec.append(el('div', { class: 'facts' },
    fact('wall time', `${(total / 1000).toFixed(1)} s`),
    fact('model turns', rows.filter(r => r.step === 'turn').length),
    fact('tool calls', rows.filter(r => r.step === 'tool').length),
    fact('turn cost', money(turnsCost))));
  const t = el('table', { class: 'timeline' });
  t.append(el('thead', null, el('tr', null, ...['step', 'status', 'start', 'duration', 'cost', '', 'detail'].map(c => el('th', { text: c })))));
  const body = el('tbody');
  for (const r of rows) {
    const bar = el('div', { class: `bar ${r.status}`, style: { left: `${(100 * r.start_ms / total).toFixed(3)}%`, width: `${Math.max(0.3, 100 * r.duration_ms / total).toFixed(3)}%` }, title: `${r.duration_ms} ms` });
    body.append(el('tr', null,
      el('td', { text: r.label }),
      el('td', { class: `status ${r.status}`, text: r.status }),
      el('td', { class: 'num', text: `${(r.start_ms / 1000).toFixed(2)} s` }),
      el('td', { class: 'num', text: `${r.duration_ms} ms` }),
      el('td', { class: 'num', text: r.cost_usd === undefined || r.cost_usd === null ? '' : money(r.cost_usd) }),
      el('td', { class: 'track' }, bar),
      el('td', { class: 'detail', text: r.detail })));
  }
  t.append(body);
  sec.append(t);
  return sec;
}

// Save as PDF goes through the browser's print dialog. The PDF holds what the page shows: a
// collapsed section (rules, job, config) is printed collapsed unless it is opened first.
document.getElementById('print').addEventListener('click', () => window.print());

async function main() {
  let report;
  try {
    report = JSON.parse(await getText('report.json'));
  } catch (e) {
    $status.textContent = `cannot load report.json: ${e.message}`;
    $status.className = 'err';
    return;
  }
  if (!SUPPORTED_REPORT_VERSIONS.includes(report.report_version)) {
    $status.textContent = `report_version ${report.report_version} is not supported by this viewer (supported: ${SUPPORTED_REPORT_VERSIONS.join(', ')})`;
    $status.className = 'err';
    return;
  }
  document.title = `squint report: ${report.use_case} ${report.verdict || 'error'}`;
  $status.textContent = `${report.use_case} · ${(report.runs || []).length} run(s) · report_version ${report.report_version}`;
  $app.append(header(report));
  for (const run of report.runs || []) {
    $app.append(await timelineSection(run));
    const locs = await locations(run);
    $app.append(imagesSection(run, locs));
    $app.append(toolsSection(run));
    $app.append(checklistSection(run));
    $app.append(aggregationSection(run));
    $app.append(turnsSection(run));
  }
  document.body.dataset.ready = 'true';
}

main();

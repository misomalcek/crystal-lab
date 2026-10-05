import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createVerse } from './verse.js';

const statusEl = document.getElementById('status');
const pathEl = document.getElementById('path');
const sideEl = document.getElementById('side');
const logEl = document.getElementById('log');
const canvas = document.getElementById('g');
const flightBtn = document.getElementById('flight');

let folder = null;
let graph = { nodes: [], edges: [] };
let focus = null;
let highlight = new Set();
let hits = [];
let lastStatus = null;

const verse = createVerse(canvas, {
  onSelect(id) {
    focus = id;
    highlight = new Set([id]);
    verse.setFocus(id);
    verse.setHighlight(highlight);
    document.getElementById('hop').disabled = !folder;
    document.getElementById('blast').disabled = !folder;
    log(`focus ${id}`);
    showDetail(id);
  },
});
verse.attachHud(document.getElementById('mpcHud'));

function log(msg) {
  logEl.textContent = msg;
}

function paintStatus(h) {
  lastStatus = h;
  const ep = h.endpoint ? ' · endpoint ●' : ' · endpoint ○';
  const line = `qdrant ${h.qdrant ? '●' : '○'}:${h.qdrant_port}  embed ${h.embed ? '●' : '○'}:${h.embed_port}  ${h.embed_model} ${h.embed_dims || '?'}d  ${h.collection} ${h.crystal_points}${ep}`;
  statusEl.textContent = h.error ? `${line}  · ${h.error}` : line;
  const starting = h.error === 'starting local engines…';
  statusEl.className = h.qdrant && h.embed ? 'ok' : starting ? 'wait' : 'bad';
  document.getElementById('nowQ').textContent = String(h.qdrant_port);
  document.getElementById('nowE').textContent = String(h.embed_port);
  const canChat = !!(h.endpoint && folder);
  document.getElementById('chatq').disabled = !canChat;
  document.getElementById('ask').disabled = !canChat;
}

function setReady(on) {
  document.getElementById('ingest').disabled = !on;
  document.getElementById('q').disabled = !on;
  document.getElementById('go').disabled = !on;
  const canChat = !!(on && lastStatus?.endpoint);
  document.getElementById('chatq').disabled = !canChat;
  document.getElementById('ask').disabled = !canChat;
}

function renderSide(census) {
  if (hits.length) {
    sideEl.innerHTML = hits
      .map((h) => {
        const lists = (h.lists || []).map((x) => `<span class="tag">${x}</span>`).join('');
        const rrf = h.rrf_score != null ? `rrf ${h.rrf_score.toFixed(4)} · ` : '';
        return `<div class="hit">${lists}<b>${rrf}cos ${Number(h.score).toFixed(3)} · ${h.path}#${h.chunk_index}</b><p>${escapeHtml(h.preview)}</p></div>`;
      })
      .join('');
    return;
  }
  if (!census) {
    sideEl.textContent = 'Pick a folder of notes or code.';
    return;
  }
  sideEl.textContent = [
    `markdown ${census.markdown}`,
    `code ${census.code}`,
    `pdf ${census.pdf}`,
    `html ${census.html}`,
    `text ${census.text}`,
    `image ${census.image} (graph only, no OCR)`,
    `office ${census.office} (not ingested)`,
    `skipped ${census.skipped}`,
    census.truncated ? 'truncated' : '',
    '',
    ...(census.samples || []),
  ]
    .filter((x) => x !== '')
    .join('\n');
}

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function weightLabel(rel, weight) {
  if (rel === 'conflicts_with') return `similarity ${(weight / 100).toFixed(2)}`;
  return `w ${weight}`;
}

function showDetail(id) {
  const card = document.getElementById('detail');
  card.classList.remove('hidden');
  card.innerHTML = `<p class="meta">loading ${escapeHtml(id)}</p>`;
  invoke('node_detail', { id })
    .then((d) => {
      const neighbors = (d.neighbors || [])
        .slice()
        .sort((a, b) => (b.weight || 0) - (a.weight || 0))
        .slice(0, 16)
        .map(
          (n) =>
            `<li><span class="kicker">${escapeHtml(n.rel)}</span> ${weightLabel(n.rel, n.weight)} · ${escapeHtml(n.label)}</li>`,
        )
        .join('');
      const passages = (d.passages || [])
        .slice(0, 8)
        .map((p) => {
          const hit = hits.find((h) => h.path === p.path && h.chunk_index === p.chunk_index);
          const scores = hit
            ? `<p class="meta">raw ${Number(hit.score).toFixed(3)}${hit.rrf_score != null ? ` · fused ${Number(hit.rrf_score).toFixed(4)}` : ''}${(hit.lists || []).length ? ` · ${hit.lists.join('+')}` : ''}</p>`
            : '';
          const surface = p.surface ? `<p class="meta">seen as ${escapeHtml(p.surface)}</p>` : '';
          return `<div class="hit"><b>${escapeHtml(p.path)}#${p.chunk_index}</b>${surface}${scores}<p>${escapeHtml(p.excerpt || '')}</p></div>`;
        })
        .join('');
      const aliases = (d.aliases || []).length
        ? `<p class="meta">aliases ${escapeHtml(d.aliases.join(', '))}</p>`
        : '';
      const files = (d.files || []).length
        ? `<p class="meta">${d.files.length} files · ${escapeHtml(d.files.slice(0, 8).join(', '))}</p>`
        : '';
      const flags = (d.flags || []).length
        ? `<p class="meta flag">${escapeHtml(d.flags.join(' · '))}</p>`
        : '';
      const merges = (d.merges || [])
        .map((m) => `<li>${escapeHtml(m.reason)}: ${escapeHtml((m.surfaces || []).join(' → '))}</li>`)
        .join('');
      card.innerHTML = `
        <button class="x" id="detailClose" type="button" title="close">×</button>
        <div class="kicker">${escapeHtml(d.kind || 'node')}</div>
        <h3>${escapeHtml(d.label || d.id)}</h3>
        <p class="meta">${d.mentions ? `${d.mentions} mentions · ` : ''}degree ${d.degree}${d.withheld ? ' · withheld' : ''}</p>
        ${aliases}${files}${flags}
        ${neighbors ? `<ul>${neighbors}</ul>` : '<p class="meta">no edges</p>'}
        ${passages}
        <details>
          <summary>graph metrics</summary>
          <p class="meta" title="Share of shortest paths that pass through this node. Degree counts frequency; betweenness counts bridges.">betweenness ${Number(d.betweenness || 0).toFixed(3)} · degree ${d.degree}</p>
        </details>
        ${merges ? `<details open><summary>merge history</summary><ul>${merges}</ul></details>` : ''}
        <details>
          <summary>payload</summary>
          <pre>${escapeHtml(JSON.stringify(d.payload, null, 2))}</pre>
        </details>`;
      document.getElementById('detailClose').addEventListener('click', () => card.classList.add('hidden'));
    })
    .catch((err) => {
      card.innerHTML = `<p class="meta">${escapeHtml(String(err))}</p>`;
    });
}

function show(el, on) {
  el.classList.toggle('show', on);
}

flightBtn.addEventListener('click', () => {
  const next = !verse.isFlight();
  verse.setFlight(next);
  flightBtn.textContent = next ? '🚀 flight' : '🛰 orbit';
  flightBtn.title = next
    ? 'arrows move · space thrust · B brake · drag look · type mpcmcp'
    : 'Switch to spaceship flight (6DOF)';
});

document.getElementById('pick').addEventListener('click', async () => {
  try {
    log('choose a folder…');
    const path = await invoke('pick_folder');
    folder = path;
    pathEl.textContent = path;
    loadTune();
    log('scanning…');
    const r = await invoke('scan_folder', { path });
    graph = r.graph;
    hits = [];
    focus = null;
    highlight = new Set();
    setReady(true);
    verse.setGraph(graph);
    verse.setFocus(null);
    verse.setHighlight(highlight);
    renderSide(r.census);
    const extra = graph.truncated ? ' · truncated at 400 nodes' : '';
    log(`${graph.nodes.length} nodes · ${graph.edges.length} edges${extra}`);
  } catch (err) {
    log(String(err));
  }
});

let ingesting = false;

document.getElementById('ingest').addEventListener('click', async () => {
  if (ingesting) {
    log('ingest already running…');
    return;
  }
  if (!folder) {
    log('pick a folder first');
    return;
  }
  ingesting = true;
  const btn = document.getElementById('ingest');
  btn.textContent = 'Ingesting…';
  btn.disabled = true;
  log('ingesting into local qdrant…');
  try {
    const r = await invoke('ingest_folder', { path: folder });
    if (r.graph) {
      graph = r.graph;
      verse.setGraph(graph);
    }
    log(
      `upserted ${r.points} chunks from ${r.files} files → ${r.collection} (${r.crystal_points} total)` +
        (r.graph ? `\nverse ${r.graph.nodes.length} nodes · ${r.graph.edges.length} edges` : '') +
        `\nentities ${r.entity_stats?.visible ?? 0} · cross-file ${r.entity_stats?.cross_file ?? 0} · relates_to ${r.entity_stats?.relates_to ?? 0} · conflicts ${r.entity_stats?.conflicts ?? 0}` +
        (r.skipped.length ? `\n${r.skipped.slice(0, 8).join('\n')}` : ''),
    );
    paintStatus(await invoke('lab_status'));
  } catch (err) {
    log(String(err));
  }
  ingesting = false;
  btn.textContent = 'Ingest';
  btn.disabled = !folder;
});

document.getElementById('go').addEventListener('click', async () => {
  const query = document.getElementById('q').value.trim();
  if (!query || !folder) return;
  try {
    const r = await invoke('retrieve', { query, folder, limit: 10 });
    hits = r.hits || [];
    highlight = new Set();
    for (const h of hits) {
      highlight.add(h.path);
      highlight.add(`${h.path}#${h.chunk_index}`);
    }
    verse.setHighlight(highlight);
    renderSide(null);
    const extra = r.graph_only_files?.length
      ? `\ngraph-only files: ${r.graph_only_files.join(', ')}`
      : '';
    log(
      `retrieve ${hits.length}  vector ${r.via_vector} · lexical ${r.via_lexical} · graph ${r.via_graph}${extra}`,
    );
  } catch (err) {
    log(String(err));
  }
});

function tuneKey() {
  return `crystal-lab-tune:${folder || ''}`;
}

function paintTune(t) {
  document.getElementById('hubSpread').value = String(t.hubSpread);
  document.getElementById('glow').value = String(t.glow);
  document.getElementById('hubVal').textContent = Number(t.hubSpread).toFixed(1);
  document.getElementById('glowVal').textContent = Number(t.glow).toFixed(2);
}

function currentTune() {
  return {
    hubSpread: Number(document.getElementById('hubSpread').value),
    glow: Number(document.getElementById('glow').value),
  };
}

function loadTune() {
  let t = { hubSpread: 4, glow: 0.45 };
  try {
    const raw = localStorage.getItem(tuneKey());
    if (raw) t = { ...t, ...JSON.parse(raw) };
  } catch {
    /* keep the starting tune */
  }
  t.hubSpread = Math.min(15, Math.max(1, Number(t.hubSpread) || 4));
  t.glow = Math.min(1.5, Math.max(0, Number(t.glow) || 0));
  paintTune(t);
  verse.setTune(t);
}

function onTune() {
  const t = currentTune();
  paintTune(t);
  verse.setTune(t);
  if (folder) localStorage.setItem(tuneKey(), JSON.stringify(t));
}

document.getElementById('hubSpread').addEventListener('input', onTune);
document.getElementById('glow').addEventListener('input', onTune);

document.getElementById('ask').addEventListener('click', async () => {
  const question = document.getElementById('chatq').value.trim();
  if (!question) return;
  try {
    log('asking…');
    const reply = await invoke('chat_ask', { question });
    const answer = typeof reply === 'string' ? reply : reply.answer;
    sideEl.innerHTML = `<div class="hit"><b>answer</b><p style="white-space:pre-wrap">${escapeHtml(answer || '')}</p></div>`;
    if (reply.highlight?.length) {
      highlight = new Set(reply.highlight);
      verse.setHighlight(highlight);
    }
    log((reply.trace || []).join('\n') || 'ask done');
  } catch (err) {
    log(String(err));
  }
});

document.getElementById('hop').addEventListener('click', async () => {
  if (!folder || !focus) return;
  try {
    const ids = await invoke('hop_cmd', { focus });
    highlight = new Set([focus, ...ids]);
    verse.setHighlight(highlight);
    log(`hop ${ids.length}: ${ids.join(', ') || '(none)'}`);
  } catch (err) {
    log(String(err));
  }
});

document.getElementById('blast').addEventListener('click', async () => {
  if (!folder || !focus) return;
  try {
    const ids = await invoke('blast_cmd', { focus });
    highlight = new Set([focus, ...ids]);
    verse.setHighlight(highlight);
    log(`blast depth 2 · ${ids.length} nodes`);
  } catch (err) {
    log(String(err));
  }
});

document.getElementById('btnEndpoint').addEventListener('click', async () => {
  const s = await invoke('get_settings');
  document.getElementById('epUrl').value = s.endpoint?.url || '';
  document.getElementById('epKey').value = s.endpoint?.api_key || '';
  document.getElementById('epModel').value = s.endpoint?.model || '';
  show(document.getElementById('endpointSheet'), true);
});
document.getElementById('btnPorts').addEventListener('click', async () => {
  const s = await invoke('get_settings');
  document.getElementById('portQ').value = s.qdrant_port || 0;
  document.getElementById('portE').value = s.embed_port || 0;
  document.getElementById('portHint').textContent = lastStatus
    ? `running qdrant ${lastStatus.qdrant_port} · embed ${lastStatus.embed_port} · ${lastStatus.embed_model}`
    : '';
  show(document.getElementById('portsSheet'), true);
});
document.getElementById('epClose').addEventListener('click', () => show(document.getElementById('endpointSheet'), false));
document.getElementById('portClose').addEventListener('click', () => show(document.getElementById('portsSheet'), false));
document.getElementById('welcomeYes').addEventListener('click', async () => {
  await invoke('dismiss_welcome');
  show(document.getElementById('welcome'), false);
  show(document.getElementById('endpointSheet'), true);
});
document.getElementById('welcomeNo').addEventListener('click', async () => {
  await invoke('dismiss_welcome');
  show(document.getElementById('welcome'), false);
});
document.getElementById('epSave').addEventListener('click', async () => {
  try {
    log('checking endpoint…');
    const saved = await invoke('save_endpoint', {
      url: document.getElementById('epUrl').value,
      apiKey: document.getElementById('epKey').value,
      model: document.getElementById('epModel').value,
    });
    show(document.getElementById('endpointSheet'), false);
    paintStatus(await invoke('lab_status'));
    if (saved.graph) {
      graph = saved.graph;
      verse.setGraph(graph);
    }
    const s = saved.entity_stats || {};
    log(`endpoint saved — visible ${s.visible ?? 0} · withheld ${s.withheld ?? 0} · conflicts ${s.conflicts ?? 0}`);
  } catch (err) {
    log(String(err));
  }
});
document.getElementById('epClear').addEventListener('click', async () => {
  try {
    const cleared = await invoke('clear_endpoint');
    show(document.getElementById('endpointSheet'), false);
    paintStatus(await invoke('lab_status'));
    if (cleared.graph) {
      graph = cleared.graph;
      verse.setGraph(graph);
    }
    log('endpoint removed — names stay linked, types return to concept');
  } catch (err) {
    log(String(err));
  }
});
document.getElementById('portSave').addEventListener('click', async () => {
  try {
    await invoke('save_ports', {
      qdrantPort: Number(document.getElementById('portQ').value) || 0,
      embedPort: Number(document.getElementById('portE').value) || 0,
    });
    show(document.getElementById('portsSheet'), false);
    log('ports saved for next launch');
  } catch (err) {
    log(String(err));
  }
});

window.addEventListener('resize', () => verse.resize());

async function refreshStatus() {
  try {
    paintStatus(await invoke('lab_status'));
  } catch (err) {
    statusEl.textContent = String(err);
    statusEl.className = 'bad';
  }
}

listen('engines', (ev) => {
  const payload = ev.payload;
  if (!ingesting) log(payload === 'ready' ? 'local engines ready' : String(payload));
  refreshStatus();
}).catch(() => {});

listen('ingest-progress', (ev) => {
  log(String(ev.payload));
}).catch(() => {});

const bootPoll = setInterval(async () => {
  await refreshStatus();
  if (lastStatus?.qdrant && lastStatus?.embed) clearInterval(bootPoll);
}, 1000);
setTimeout(() => clearInterval(bootPoll), 90000);

refreshStatus();
invoke('get_settings')
  .then((s) => {
    if (!s.welcome_seen) show(document.getElementById('welcome'), true);
  })
  .catch(() => {});
renderSide(null);
verse.resize();

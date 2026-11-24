const names = {codex: 'Codex', claude_code: 'Claude Code', pi: 'Pi', copilot: 'GitHub Copilot', opencode: 'OpenCode'};
const providerColors = {codex: palette[0], claude_code: palette[1], pi: palette[2], opencode: palette[3], copilot: palette[4]};
const ranges = {today: 'Today', '7d': 'Last 7 days', '30d': 'Last 30 days', all: 'All time'};
const detail = location.pathname.startsWith('/provider/') ? location.pathname.slice(10) : '';
const content = document.querySelector('#content');
const params = new URLSearchParams(location.search);
let range = Object.hasOwn(ranges, params.get('window')) ? params.get('window') : 'today';
let generation = 0;
let loaded = [];
function preference(key, fallback) {
  try { return JSON.parse(localStorage.getItem(key)) ?? fallback; } catch { return fallback; }
}
function save(key, value) { try { localStorage.setItem(key, value); } catch {} }
const stored = preference('agentusage-hidden-providers', []);
const hidden = new Set(Array.isArray(stored) ? stored.filter(p => Object.hasOwn(names, p)) : []);
let theme = 'light';
try { theme = localStorage.getItem('agentusage-theme') || theme; } catch {}
function applyTheme() {
  document.documentElement.dataset.theme = theme;
  document.querySelector('#theme-toggle').textContent = theme === 'dark' ? 'Light theme' : 'Dark theme';
}
document.querySelector('#theme-toggle').onclick = () => {
  theme = theme === 'dark' ? 'light' : 'dark';
  save('agentusage-theme', theme);
  applyTheme();
};
applyTheme();
function providerUrl(id) { return '/provider/' + encodeURIComponent(id) + '?window=' + range; }
async function getJson(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error('Request failed (' + response.status + ')');
  return response.json();
}
function money(value) {
  return '$' + Number(value).toLocaleString(undefined, {minimumFractionDigits: 2, maximumFractionDigits: 6});
}
function section(cls) { const el = document.createElement('section'); el.className = cls; return el; }
function note(text, error = false) {
  const p = document.createElement('p'); p.className = 'empty' + (error ? ' error' : ''); p.textContent = text; return p;
}
function control(label, action, cls = '') {
  const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.className = cls;
  b.addEventListener('click', action); return b;
}
const modelsColumns = [
  ['Model', 'name'], ['Input', 'input_tokens'], ['Output', 'output_tokens'], ['Cache read', 'cache_read_tokens'],
  ['Cache write', 'cache_write_tokens'], ['Total', 'total_tokens']
];
function modelColumnsFor({name, summary}) {
  if (name === 'codex' && !Object.values(summary.models).some(model => model.cache_write_tokens > 0)) {
    return modelsColumns.filter(([, key]) => key !== 'cache_write_tokens');
  }
  return modelsColumns;
}
const providerColumns = [['Provider', 'name'], ['Input', 'input_tokens'], ['Output', 'output_tokens'], ['Total', 'total_tokens'], ['Est. cost', 'cost_usd']];
function rowsFor(buckets, columns) {
  return Object.entries(buckets).sort((a,b) => b[1].total_tokens - a[1].total_tokens).map(([name, v]) =>
    columns.map(([,key]) => key === 'name' ? name : key === 'cost_usd' ? money(v[key]) : exact.format(v[key])));
}
function breakdown(title, buckets, columns) {
  return '<h3>'+title+'</h3><div class="table-wrap"><table><thead><tr>' +
    columns.map(([label]) => '<th scope="col">'+label+'</th>').join('') + '</tr></thead><tbody>' +
    rowsFor(buckets, columns).map(row => '<tr>'+row.map(value => '<td>'+escapeHtml(value)+'</td>').join('')+'</tr>').join('') +
    '</tbody></table></div>';
}
function metrics({name, summary: s}) {
  const items = [
    ['Tokens', exact.format(s.total_tokens)], ['Requests', exact.format(s.requests)],
    ['Sessions', exact.format(s.sessions)], ['Prompts', exact.format(s.prompts)]
  ];
  if (name === 'codex') {
    const share = s.input_tokens > 0
      ? (s.cache_read_tokens / s.input_tokens).toLocaleString(undefined, {style: 'percent', maximumFractionDigits: 1})
      : '—';
    items.push(['Cached input', share]);
  } else {
    items.push(['Cache read', exact.format(s.cache_read_tokens)]);
  }
  if (name !== 'codex' || s.cache_write_tokens > 0) {
    items.push(['Cache write', exact.format(s.cache_write_tokens)]);
  }
  items.push(['Est. cost', money(s.cost_usd)]);
  return items;
}
function modelSeries(points) {
  const ids = [...new Set(points.flatMap(p => Object.keys(p.models)))];
  return ids.map((name, i) => ({name, color: palette[i % palette.length], values: points.map(p => Object.hasOwn(p.models,name) ? p.models[name] : 0)}))
    .sort((a,b) => sum(b.values) - sum(a.values));
}
function chart(data, isDetail) {
  const hourly = range === 'today';
  const times = new Map();
  data.forEach(d => d.points.forEach(p => times.set(p.bucket_start, p)));
  const points = [...times.values()].sort((a,b) => a.bucket_start.localeCompare(b.bucket_start));
  let series, total;
  if (isDetail) {
    series = modelSeries(points);
    total = points.map(p => p.total_tokens);
  } else {
    series = data.map(d => {
      const byTime = new Map(d.points.map(p => [p.bucket_start, p.total_tokens]));
      return {name: names[d.name], color: providerColors[d.name], values: points.map(p => byTime.get(p.bucket_start) ?? 0)};
    }).sort((a,b) => sum(b.values) - sum(a.values));
    total = points.map((_,i) => sum(series.map(s => s.values[i])));
  }
  const period = range === 'all' ? 'Last 90 days' : ranges[range];
  const grouping = (isDetail ? 'model' : 'provider') + (series.length === 1 ? '' : 's');
  const subtitle = period + ' · ' + series.length + ' ' + grouping;
  const requests = sum(data.map(d => d.summary.requests));
  const context = range === 'all' ? 'In the last 90 days' : exact.format(requests) + (requests === 1 ? ' request' : ' requests');
  return activityPanel(points, series, total, hourly, subtitle, context);
}
function historyPanel(id) {
  const box = section('prompt-browser');
  box.id = 'history-' + id;
  box.innerHTML = '<div class="history-head"><h3>Prompt history</h3><form><input type="search" aria-label="Search prompt text" placeholder="Search prompt text"><button>Search</button></form></div>' +
    '<div class="prompt-list" aria-live="polite"></div>';
  const input = box.querySelector('input'), list = box.querySelector('.prompt-list');
  const windowAtOpen = range;
  let version = 0, query = '', cursor = null;
  const more = control('Load more prompts', () => fetchPage(true)); more.hidden = true; box.append(more);
  async function fetchPage(append) {
    const current = ++version;
    if (!append) { query = input.value.trim(); cursor = null; list.textContent = 'Loading prompts…'; }
    more.hidden = true;
    try {
      const args = new URLSearchParams({provider: id, window: windowAtOpen, limit: detail ? '10' : '3'});
      if (query) args.set('search', query);
      if (append && cursor) args.set('cursor', cursor);
      const page = await getJson('/api/prompts?' + args);
      if (current !== version) return;
      if (!append) list.replaceChildren();
      for (const prompt of page.prompts) {
        const item = document.createElement('details'); item.className = 'prompt-item';
        item.innerHTML = '<summary><small></small><span class="preview"></span></summary><pre></pre>';
        item.querySelector('small').textContent = new Date(prompt.occurred_at).toLocaleString() + ' · ' + (prompt.model || 'Unknown model');
        item.querySelector('.preview').textContent = prompt.text.replace(/\s+/g, ' ').trim();
        item.querySelector('pre').textContent = prompt.text;
        list.append(item);
      }
      if (!list.childElementCount) list.textContent = query ? 'No matching prompts.' : 'No recorded prompts in this period.';
      cursor = page.next_cursor;
      more.hidden = !cursor;
    } catch (err) {
      if (current !== version) return;
      if (!append) list.replaceChildren();
      list.append(note(err.message, true));
      more.hidden = !append;
    }
  }
  box.querySelector('form').onsubmit = e => { e.preventDefault(); fetchPage(false); };
  fetchPage(false);
  return box;
}
function historyButton(id, parent) {
  let box;
  const b = control('Prompt history', () => {
    if (box) { box.remove(); box = null; } else { box = historyPanel(id); parent.append(box); }
    b.setAttribute('aria-expanded', String(Boolean(box)));
  }, 'history-toggle');
  b.setAttribute('aria-expanded','false');
  return b;
}
function actions(d) {
  const group = document.createElement('details'); group.className = 'actions';
  group.innerHTML = '<summary aria-label="Actions for '+escapeHtml(names[d.name])+'" title="Provider actions">···</summary><div class="menu"></div>';
  const menu = group.querySelector('.menu');
  const link = document.createElement('a'); link.textContent = 'View full page'; link.href = providerUrl(d.name); menu.append(link);
  if (d.summary) menu.append(control('Download SVG', () => { group.open = false; downloadCard(d); }));
  menu.append(control('Hide provider', () => {
    hidden.add(d.name); save('agentusage-hidden-providers',JSON.stringify([...hidden])); render();
  }));
  return group;
}
function directory(data) {
  const dir = section('panel directory');
  dir.innerHTML = '<div class="section-head"><h2>Providers</h2></div>';
  const head = dir.firstElementChild;
  if (hidden.size) {
    const select = document.createElement('select'); select.id = 'show-hidden'; select.setAttribute('aria-label','Show hidden providers');
    select.append(new Option('Show hidden ('+hidden.size+')',''),new Option('Show all','*'));
    hidden.forEach(id => select.append(new Option(names[id], id)));
    select.onchange = () => {
      if (!select.value) return;
      if (select.value === '*') hidden.clear(); else hidden.delete(select.value);
      save('agentusage-hidden-providers',JSON.stringify([...hidden])); render();
    };
    head.append(select);
  }
  if (!data.length) { dir.append(note('All providers are hidden.')); return dir; }
  dir.insertAdjacentHTML('beforeend','<div class="provider-line colheads" aria-hidden="true"><span>Provider</span><span>Tokens</span><span class="requests">Requests</span><span class="cost">Est. cost</span><span></span></div>');
  for (const d of data) {
    const group = document.createElement('div');
    const row = document.createElement('div'); row.className = 'provider-line';
    const s = d.summary;
    row.innerHTML = '<a class="provider-link"><i class="dot"></i><span></span></a><span class="tokens"></span><span class="requests"></span><span class="cost"></span><div class="row-actions"></div>';
    const link = row.querySelector('a'); link.href = providerUrl(d.name);
    link.querySelector('span').textContent = names[d.name]; link.style.setProperty('--series',providerColors[d.name]);
    row.querySelector('.tokens').textContent = s ? compact.format(s.total_tokens) : d.error || 'Unavailable';
    row.querySelector('.requests').textContent = s ? exact.format(s.requests) : '—';
    row.querySelector('.cost').textContent = s ? money(s.cost_usd) : '—';
    for (const [key, label] of [['tokens','Tokens'],['requests','Requests'],['cost','Estimated cost']]) {
      const field = row.querySelector('.'+key);
      const caption = document.createElement('span'); caption.className = 'sr-only'; caption.textContent = label+' ';
      field.prepend(caption);
    }
    const controls = row.querySelector('.row-actions');
    if (s) controls.append(historyButton(d.name, group));
    controls.append(actions(d)); group.append(row); dir.append(group);
  }
  return dir;
}
function providerDetail(d) {
  const box = section('panel detail-panel');
  box.innerHTML = '<div class="section-head"><h2>Usage breakdown</h2></div><div class="detail-body"></div>';
  const head = box.firstElementChild;
  head.append(control('Download SVG', () => downloadCard(d)));
  const s = d.summary, body = box.querySelector('.detail-body');
  body.innerHTML = '<dl class="metrics">'+metrics(d).map(([k,v]) => '<div><dt>'+k+'</dt><dd>'+escapeHtml(v)+'</dd></div>').join('')+'</dl>';
  if (Object.keys(s.models).length) body.insertAdjacentHTML('beforeend',breakdown('Models',s.models,modelColumnsFor(d)));
  if (d.name === 'pi' && Object.keys(s.providers).length) body.insertAdjacentHTML('beforeend',breakdown('Model providers',s.providers,providerColumns));
  body.insertAdjacentHTML('beforeend','<p class="muted">Cache counts are recorded tokens. A zero can also mean the source omitted the counter.</p>');
  const history = section('panel');
  history.innerHTML = '<div class="section-head"><h2>Prompt history</h2></div>';
  const b = historyButton(d.name, history); b.textContent = 'Show prompts'; history.firstElementChild.append(b);
  return [box,history];
}
function render() {
  content.replaceChildren();
  const data = loaded.filter(d => detail || !hidden.has(d.name));
  if (detail && !data.length) { content.append(note('Unknown provider.',true)); return; }
  const ready = data.filter(d => d.summary);
  if (ready.length) content.append(chart(ready, Boolean(detail)));
  if (detail) {
    if (ready.length) content.append(...providerDetail(ready[0]));
    else content.append(note(data[0].error || 'Provider storage is unavailable.',true));
  } else content.append(directory(data));
}
async function load() {
  const current = ++generation, requestedRange = range;
  document.querySelectorAll('[data-window]').forEach(b => b.setAttribute('aria-pressed',String(b.dataset.window === range)));
  document.querySelector('#back').href = '/?window=' + range;
  content.setAttribute('aria-busy','true');
  content.replaceChildren(note('Loading usage…'));
  try {
    const available = (await getJson('/api/providers')).filter(p => !detail || p.name === detail);
    const data = await Promise.all(available.map(async p => {
      if (!p.available) return p;
      const query = '?provider=' + encodeURIComponent(p.name) + '&window=' + requestedRange;
      try {
        const [summary,points] = await Promise.all([
          getJson('/api/summary' + query), getJson('/api/trend' + query + (requestedRange === 'today' ? '&interval=hour' : ''))
        ]);
        return {...p, summary, points};
      } catch (e) { return {...p, error: e.message}; }
    }));
    if (current === generation) { loaded = data; render(); }
  } catch (err) { if (current === generation) content.replaceChildren(note(err.message,true)); }
  finally { if (current === generation) content.setAttribute('aria-busy','false'); }
}
document.querySelector('#back').hidden = !detail;
if (detail) document.querySelector('#title').textContent = names[detail] || 'Provider';
document.querySelectorAll('[data-window]').forEach(b => b.onclick = () => {
  if (range === b.dataset.window) return;
  range = b.dataset.window; history.replaceState(null,'','?window='+range); load();
});

// Use native SVG elements so the downloaded report opens in image viewers offline.
function downloadCard(d) {
  const series = modelSeries(d.points), totals = d.points.map(p => p.total_tokens);
  const tables = [['Models',modelColumnsFor(d),d.summary.models]];
  if (d.name === 'pi') tables.push(['Model providers',providerColumns,d.summary.providers]);
  const longest = Math.max(12,...tables.flatMap(([, ,entries]) => Object.keys(entries).map(s => s.length)));
  const width = Math.max(800, longest * 8 + 570), pad = 28;
  const ns = 'http://www.w3.org/2000/svg', svg = document.createElementNS(ns,'svg');
  svg.setAttribute('xmlns',ns); svg.setAttribute('width',width);
  let y = 40;
  function add(tag, attrs, value) {
    const el = document.createElementNS(ns,tag);
    for (const [k,v] of Object.entries(attrs)) el.setAttribute(k,v);
    if (value !== undefined) el.textContent = value;
    svg.append(el); return el;
  }
  function label(x,baseline,txt,size=14,anchor='start',color='#253448') {
    add('text',{x,y:baseline,fill:color,'font-family':'sans-serif','font-size':size,'text-anchor':anchor},txt);
  }
  add('rect',{width:'100%',height:'100%',fill:'white'});
  label(pad,y,names[d.name]+' · '+ranges[range],24); y+=35;
  for (const [k,v] of metrics(d)) { label(pad,y,k); label(width-pad,y,v,14,'end'); y+=23; }
  y+=20; label(pad,y,'Token activity'+(range === 'all' ? ' · Last 90 days' : ''),19); y+=18;
  const ceiling = Math.max(1,...totals), h=135;
  const x = i => pad+(i+.5)*(width-2*pad)/totals.length;
  for (const s of [{name:'Total',color:'#0878d8',values:totals},...series]) {
    const points = s.values.map((v,i) => x(i)+','+(y+h-v/ceiling*h));
    add('polyline',{points:points.join(' '),fill:'none',stroke:s.color,'stroke-width':2});
    if (points.length === 1) add('circle',{cx:x(0),cy:y+h-s.values[0]/ceiling*h,r:3,fill:s.color});
  }
  y+=h+18;
  if (d.points.length) {
    label(pad,y,timeLabel(d.points[0],range==='today'),12);
    label(width-pad,y,timeLabel(d.points.at(-1),range==='today'),12,'end');
  }
  y+=25; label(pad,y,'Total',13,'start','#0878d8');
  series.forEach(s=>{y+=18; label(pad,y,s.name,13,'start',s.color);});
  for (const [title,cols,entries] of tables) {
    if (!Object.keys(entries).length) continue;
    y+=33; label(pad,y,title,19); y+=25;
    const firstWidth=longest*8+20, numericWidth=(width-2*pad-firstWidth)/(cols.length-1);
    for (const row of [cols.map(([k])=>k),...rowsFor(entries,cols)]) {
      row.forEach((value,i)=>label(i ? pad+firstWidth+i*numericWidth : pad,y,value,13,i?'end':'start'));
      y+=23;
    }
  }
  svg.setAttribute('height',y+pad);
  const url = URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(svg)],{type:'image/svg+xml'}));
  const a = document.createElement('a'); a.href=url; a.download='agentusage-'+d.name+'-'+range+'.svg';
  document.body.append(a); a.click(); a.remove(); setTimeout(()=>URL.revokeObjectURL(url),10000);
}
load();

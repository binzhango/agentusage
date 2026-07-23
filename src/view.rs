//! Browser presentation for the local usage dashboard.
//!
//! Keeping the page in its own module means the HTTP server owns routing and
//! data access while this module owns markup, styling, and browser behavior.

/// Render the dashboard document served from `GET /`.
pub fn index_html() -> &'static str {
    INDEX_HTML
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Agentusage</title>
<style>
:root{color-scheme:dark;--bg:#181923;--panel:#1e1f2b;--border:#454557;--muted:#8b8b9e;--accent:#e9b4c9;--text:#ddd;--strong:#fff;--value:#fff;--button-bg:#292a3a;--rule:#30313f;--table-rule:#38394a;--heading:#d9a7c0;--chart-grid:#363746}
:root[data-theme="light"]{color-scheme:light;--bg:#f3f4f7;--panel:#fff;--border:#c8cbd6;--muted:#626777;--accent:#9b3f6b;--text:#252733;--strong:#20212b;--value:#171923;--button-bg:#fff;--rule:#e0e2e8;--table-rule:#d8dae2;--heading:#87435f;--chart-grid:#dfe1e8}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:15px ui-monospace,SFMono-Regular,Menlo,monospace}
header{padding:18px 24px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between;gap:16px;flex-wrap:wrap}
header strong{color:var(--strong);font-size:16px}.controls{display:flex;gap:8px;flex-wrap:wrap}button{background:var(--button-bg);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:7px 11px;cursor:pointer}button:hover,button.active{border-color:var(--accent);color:var(--accent)}.card-actions{display:flex;justify-content:flex-end;margin:-4px 0 8px}.card-actions button{font-size:12px;padding:5px 8px}
main{padding:18px;display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,520px),1fr));gap:16px}.card{border:1px solid var(--border);border-radius:8px;padding:16px;background:var(--panel);min-width:0}.card h2{margin:0 0 12px;color:var(--accent);font-size:18px}.card h3{margin:22px 0 8px;color:var(--heading);font-size:15px}.muted{color:var(--muted)}.error{color:#c44747}
.metric{display:flex;justify-content:space-between;padding:7px 0;border-bottom:1px solid var(--rule)}.metric:last-of-type{border-bottom:0}.metric b{color:var(--value)}.table-wrap{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:12px}th,td{border-bottom:1px solid var(--table-rule);padding:7px 8px;text-align:right;white-space:nowrap}th{color:var(--muted);font-weight:normal}th:first-child,td:first-child{text-align:left}td:first-child{color:var(--accent);max-width:230px;overflow:hidden;text-overflow:ellipsis}.status{font-size:12px;color:var(--muted);margin:0 0 12px}.loading{grid-column:1/-1;padding:24px;text-align:center}
.trend{margin-top:22px}.trend h3{margin-top:0}.chart{width:100%;height:auto;display:block;overflow:visible}.chart-grid{stroke:var(--chart-grid);stroke-width:1}.chart-axis{fill:var(--muted);font-size:10px}.chart-line{fill:none;stroke:var(--accent);stroke-width:2.5;stroke-linecap:round;stroke-linejoin:round}.model-line{fill:none;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round;opacity:.9}.chart-area{fill:url(#trend-fill);opacity:.35}.chart-dot{fill:var(--panel);stroke:var(--accent);stroke-width:2}.model-dot{fill:var(--panel);stroke-width:1.5}.legend{display:flex;gap:14px;flex-wrap:wrap;color:var(--muted);font-size:11px;margin-top:7px}.legend span:before{content:'';display:inline-block;width:8px;height:8px;border-radius:50%;background:var(--legend-color,var(--accent));margin-right:5px}.chart-empty{height:150px;display:grid;place-items:center;border:1px dashed var(--border);border-radius:6px;color:var(--muted);font-size:12px}
@media(max-width:600px){header{padding:14px 16px}main{padding:12px}.card{padding:13px}}
</style>
<style>.card{position:relative}.card h2{cursor:pointer}.card-actions{gap:8px}.detail-nav{padding:12px 24px 0}.detail-nav a{color:var(--accent)}main.detail-view{grid-template-columns:minmax(0,1fr)}select{background:var(--button-bg);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:7px 11px;cursor:pointer}select:hover{border-color:var(--accent);color:var(--accent)}</style>
<style>
:root{--accent-contrast:#241821;--control-height:36px}
:root[data-theme="light"]{--accent-contrast:#fff}
body{font-family:ui-sans-serif,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;font-size:14px;line-height:1.5;letter-spacing:-.01em}
header strong{font-size:15px;letter-spacing:.01em}
.controls{align-items:center;gap:9px}
button,select{appearance:none;min-height:var(--control-height);border:1px solid var(--border);border-radius:8px;background:var(--button-bg);color:var(--text);font:600 13px/1.2 ui-sans-serif,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;letter-spacing:0;padding:8px 12px;transition:background-color .16s ease,border-color .16s ease,color .16s ease,box-shadow .16s ease,transform .16s ease}
button:hover:not(:disabled),select:hover{border-color:var(--accent);background:color-mix(in srgb,var(--accent) 8%,var(--button-bg));color:var(--accent)}
button:active:not(:disabled){transform:translateY(1px)}
button:focus-visible,select:focus-visible{outline:0;border-color:var(--accent);box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 25%,transparent)}
button.active{border-color:var(--accent);background:color-mix(in srgb,var(--accent) 14%,var(--button-bg));color:var(--accent)}
#theme-toggle{color:var(--muted);font-weight:600}
#show-hidden{min-width:190px;text-align:left;color:var(--muted);padding-right:30px;background-image:linear-gradient(45deg,transparent 50%,var(--muted) 50%),linear-gradient(135deg,var(--muted) 50%,transparent 50%);background-position:calc(100% - 16px) 15px,calc(100% - 11px) 15px;background-size:5px 5px,5px 5px;background-repeat:no-repeat}
.card-actions{align-items:center;flex-wrap:wrap;margin:-2px 0 12px}
.card-actions button{min-height:32px;padding:7px 10px;font-size:12px}
.card-actions button:first-child{border-color:var(--accent);background:var(--accent);color:var(--accent-contrast);font-weight:700}
.card-actions button:nth-child(2){background:transparent;color:var(--text)}
.card-actions button:nth-child(3){background:transparent;color:var(--muted);border-color:transparent}
.card-actions button:nth-child(3):hover{border-color:var(--border);color:var(--accent)}
.status,.muted{font-size:13px}
.card h2{font-size:17px;font-weight:700;letter-spacing:-.02em}
.card h3{font-size:13px;font-weight:700;letter-spacing:.01em;text-transform:uppercase}
.metric{font-size:13px}.metric b{font-size:14px;font-weight:700}
.chart-tooltip{position:fixed;z-index:10;pointer-events:none;max-width:280px;padding:9px 11px;border:1px solid var(--border);border-radius:8px;background:var(--panel);box-shadow:0 8px 24px rgba(0,0,0,.24);color:var(--text);font:600 12px/1.45 ui-sans-serif,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;white-space:pre-line}
#show-hidden:disabled{cursor:not-allowed;opacity:.62}
</style>
</head>
<body>
<header><strong>● Agentusage</strong><span class="controls" aria-label="Date range">
<button data-window="today">Today</button><button data-window="7d">7 Days</button><button data-window="30d">30 Days</button><button data-window="all">All Time</button><select id="show-hidden" aria-label="Show hidden provider cards" hidden><option value="">Show hidden card…</option></select><button id="theme-toggle" type="button" aria-label="Switch color theme">Light theme</button>
</span></header>
<nav id="detail-nav" class="detail-nav" hidden><a href="/">← All providers</a></nav>
<main id="app"><p class="muted loading">Loading provider data…</p></main>
<div id="chart-tooltip" class="chart-tooltip" hidden></div>
<script>
let windowName='today';
const app=document.querySelector('#app');
const detailProvider=location.pathname.startsWith('/provider/')?decodeURIComponent(location.pathname.slice('/provider/'.length)):null;
const buttons=[...document.querySelectorAll('[data-window]')];
const themeToggle=document.querySelector('#theme-toggle');
const showHidden=document.querySelector('#show-hidden');
let hiddenProviders=new Set();try{hiddenProviders=new Set(JSON.parse(localStorage.getItem('agentusage-hidden-providers')||'[]'))}catch(error){}
function updateHiddenControl(){showHidden.hidden=Boolean(detailProvider);showHidden.disabled=hiddenProviders.size===0;showHidden.replaceChildren(new Option(hiddenProviders.size?'Show hidden card…':'No hidden cards',''));if(hiddenProviders.size){showHidden.append(new Option('Show all hidden cards','__all__'));[...hiddenProviders].sort().forEach(provider=>showHidden.append(new Option('Show '+provider,provider)))}showHidden.value=''}
showHidden.addEventListener('change',()=>{if(!showHidden.value)return;if(showHidden.value==='__all__')hiddenProviders.clear();else hiddenProviders.delete(showHidden.value);try{localStorage.setItem('agentusage-hidden-providers',JSON.stringify([...hiddenProviders]))}catch(error){}updateHiddenControl();load()});
let themeName='dark';try{themeName=localStorage.getItem('agentusage-theme')||((window.matchMedia&&window.matchMedia('(prefers-color-scheme: light)').matches)?'light':'dark')}catch(error){}
function applyTheme(){document.documentElement.dataset.theme=themeName;themeToggle.textContent=themeName==='dark'?'Light theme':'Dark theme';themeToggle.setAttribute('aria-label','Switch to '+(themeName==='dark'?'light':'dark')+' theme')}
const formatTokens=value=>Number(value||0).toLocaleString();
const escapeHtml=value=>String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const chartTooltip=document.querySelector('#chart-tooltip');
function prepareChartTooltips(){app.querySelectorAll('.chart-dot,.model-dot').forEach(point=>{const title=point.querySelector('title');if(title){point.dataset.tooltip=title.textContent;title.remove()}})}
function showChartTooltip(point,event){const title=point.dataset.tooltip;if(!title)return;chartTooltip.textContent=title;chartTooltip.hidden=false;chartTooltip.style.left=Math.min(event.clientX+14,window.innerWidth-chartTooltip.offsetWidth-12)+'px';chartTooltip.style.top=Math.min(event.clientY+14,window.innerHeight-chartTooltip.offsetHeight-12)+'px'}
app.addEventListener('pointerover',event=>{const point=event.target.closest('.chart-dot,.model-dot');if(point)showChartTooltip(point,event)});
app.addEventListener('pointermove',event=>{const point=event.target.closest('.chart-dot,.model-dot');if(point&&!chartTooltip.hidden)showChartTooltip(point,event)});
app.addEventListener('pointerout',event=>{if(event.target.closest('.chart-dot,.model-dot'))chartTooltip.hidden=true});
buttons.forEach(button=>button.addEventListener('click',()=>{windowName=button.dataset.window;load()}));
themeToggle.addEventListener('click',()=>{themeName=themeName==='dark'?'light':'dark';try{localStorage.setItem('agentusage-theme',themeName)}catch(error){}applyTheme()});
function syncButtons(){buttons.forEach(button=>button.classList.toggle('active',button.dataset.window===windowName))}
function modelBurnTable(models){const entries=Object.entries(models||{});if(!entries.length)return '<p class="muted">No model token usage in this window.</p>';const rows=entries.map(([name,v])=>'<tr><td title="'+escapeHtml(name)+'">'+escapeHtml(name)+'</td><td>'+formatTokens(v.input_tokens)+'</td><td>'+formatTokens(v.output_tokens)+'</td><td>'+formatTokens(v.cache_read_tokens)+'</td><td>'+formatTokens(v.cache_write_tokens)+'</td><td>'+formatTokens(v.total_tokens)+'</td></tr>').join('');return '<div class="table-wrap"><table><thead><tr><th>model</th><th>input</th><th>output</th><th>cache_read</th><th>cache_write</th><th>total</th></tr></thead><tbody>'+rows+'</tbody></table></div>'}
function providerBurnTable(providers,show){const entries=Object.entries(providers||{});if(!show||!entries.length)return '';const rows=entries.map(([name,v])=>'<tr><td title="'+escapeHtml(name)+'">'+escapeHtml(name)+'</td><td>'+formatTokens(v.input_tokens)+'</td><td>'+formatTokens(v.output_tokens)+'</td><td>'+formatTokens(v.total_tokens)+'</td><td>$'+Number(v.cost_usd||0).toFixed(6)+'</td></tr>').join('');return '<h3>Providers</h3><div class="table-wrap"><table><thead><tr><th>provider</th><th>input</th><th>output</th><th>total</th><th>cost</th></tr></thead><tbody>'+rows+'</tbody></table></div>'}
function trendChart(points){const width=760,height=250,left=48,right=16,top=14,bottom=30;const palette=['#e9b4c9','#8ed1c5','#f6c177','#9db5ff','#c9a7e9','#f28f8f','#8bd3dd','#b8d47d'];const modelNames=[...new Set(points.flatMap(p=>Object.keys(p.models||{})))];const values=points.flatMap(p=>[p.total_tokens,...modelNames.map(name=>p.models?.[name]||0)]);const max=Math.max(...values,1);const x=i=>left+(points.length===1?0:(width-left-right)*i/(points.length-1));const y=value=>top+(height-top-bottom)*(1-value/max);const pathFor=values=>points.map((_,i)=>(i?'L':'M')+x(i).toFixed(1)+' '+y(values[i]).toFixed(1)).join(' ');const totalValues=points.map(p=>p.total_tokens);const area=pathFor(totalValues)+' L '+x(points.length-1).toFixed(1)+' '+(height-bottom)+' L '+x(0).toFixed(1)+' '+(height-bottom)+' Z';const modelLines=modelNames.map((name,index)=>{const color=palette[index%palette.length];const values=points.map(p=>p.models?.[name]||0);const line='<path class="model-line" stroke="'+color+'" d="'+pathFor(values)+'"/><g>'+points.filter((p,i)=>values[i]>0).map((p,i)=>{const pointIndex=points.indexOf(p);return '<circle class="model-dot" stroke="'+color+'" cx="'+x(pointIndex)+'" cy="'+y(values[pointIndex])+'" r="2"><title>'+escapeHtml(name)+'\n'+escapeHtml(p.date)+'\n'+formatTokens(values[pointIndex])+' tokens</title></circle>'}).join('')+'</g>';return {name,color,line}});const labels=points.filter((_,i)=>i===0||i===points.length-1||i===Math.floor((points.length-1)/2)).map((p)=>{const i=points.indexOf(p);return '<text class="chart-axis" x="'+x(i)+'" y="'+(height-8)+'" text-anchor="middle">'+escapeHtml(p.date.slice(5))+'</text>'}).join('');const dots=points.filter(p=>p.total_tokens>0).map((p)=>{const i=points.indexOf(p);return '<circle class="chart-dot" cx="'+x(i)+'" cy="'+y(p.total_tokens)+'" r="3"><title>'+'Total tokens\n'+escapeHtml(p.date)+'\n'+formatTokens(p.total_tokens)+' tokens</title></circle>'}).join('');const grid=[0,.5,1].map(v=>'<line class="chart-grid" x1="'+left+'" x2="'+(width-right)+'" y1="'+y(max*v)+'" y2="'+y(max*v)+'"/>').join('');const legend='<span style="--legend-color:var(--accent)">total</span>'+modelLines.map(model=>'<span style="--legend-color:'+model.color+'">'+escapeHtml(model.name)+'</span>').join('');return '<div class="trend"><h3>Token usage trend</h3><svg class="chart" viewBox="0 0 '+width+' '+height+'" role="img" aria-label="Daily token usage by model"><defs><linearGradient id="trend-fill" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="#e9b4c9"/><stop offset="1" stop-color="#e9b4c9" stop-opacity="0"/></linearGradient></defs>'+grid+'<path class="chart-area" d="'+area+'"/><path class="chart-line" d="'+pathFor(totalValues)+'"/>'+modelLines.map(model=>model.line).join('')+dots+labels+'</svg><div class="legend">'+legend+'</div></div>'}
function addExportButton(card,providerName){const actions=document.createElement('div');actions.className='card-actions';const viewPath='/provider/'+encodeURIComponent(providerName);const view=document.createElement('button');view.type='button';view.textContent='View full page';view.addEventListener('click',()=>location.href=viewPath);const download=document.createElement('button');download.type='button';download.textContent='Download SVG';download.setAttribute('aria-label','Download '+providerName+' card as SVG');download.addEventListener('click',event=>{event.stopPropagation();downloadCard(card,providerName)});actions.append(view,download);if(!detailProvider){const hide=document.createElement('button');hide.type='button';hide.textContent='Hide card';hide.setAttribute('aria-label','Hide '+providerName+' card');hide.addEventListener('click',event=>{event.stopPropagation();hiddenProviders.add(providerName);try{localStorage.setItem('agentusage-hidden-providers',JSON.stringify([...hiddenProviders]))}catch(error){}card.remove();updateHiddenControl()});actions.append(hide)}card.prepend(actions);card.addEventListener('click',event=>{if(!event.target.closest('button'))location.href=viewPath})}
async function getJson(url){const response=await fetch(url);if(!response.ok)throw new Error('Request failed ('+response.status+')');return response.json()}
async function load(){syncButtons();app.innerHTML='<p class="muted loading">Loading provider data…</p>';try{const providers=await getJson('/api/providers');app.innerHTML='';for(const p of providers){const card=document.createElement('section');card.className='card';if(!p.available){card.innerHTML='<h2>'+escapeHtml(p.name)+'</h2><p class="muted">Unavailable</p>';addExportButton(card,p.name);app.append(card);continue}try{const [s,points]=await Promise.all([getJson('/api/summary?provider='+encodeURIComponent(p.name)+'&window='+windowName),getJson('/api/trend?provider='+encodeURIComponent(p.name)+'&window='+windowName)]);card.innerHTML='<h2>'+escapeHtml(p.name)+'</h2><p class="status">Window: '+windowName+'</p><div class="metric"><span>tokens</span><b>'+formatTokens(s.total_tokens)+'</b></div><div class="metric"><span>requests</span><b>'+formatTokens(s.requests)+'</b></div><div class="metric"><span>sessions</span><b>'+formatTokens(s.sessions)+'</b></div><div class="metric"><span>cost</span><b>$'+Number(s.cost_usd).toFixed(6)+'</b></div>'+(points.length?trendChart(points):'<div class="trend"><h3>Token usage trend</h3><div class="chart-empty">No token usage in this window.</div></div>')+providerBurnTable(s.providers,p.name==='pi')+'<h3>Model burn</h3>'+modelBurnTable(s.models)}catch(error){card.innerHTML='<h2>'+escapeHtml(p.name)+'</h2><p class="error">'+escapeHtml(error.message)+'</p>'}addExportButton(card,p.name);app.append(card)}}catch(error){app.innerHTML='<p class="error loading">'+escapeHtml(error.message)+'</p>'}}
function downloadCard(card,providerName){const light=document.documentElement.dataset.theme==='light';const c=light?{bg:'#fff',border:'#c8cbd6',text:'#252733',muted:'#626777',accent:'#9b3f6b',heading:'#87435f',value:'#171923',rule:'#e0e2e8',grid:'#dfe1e8'}:{bg:'#1e1f2b',border:'#555568',text:'#ddd',muted:'#b9b9c8',accent:'#e9b4c9',heading:'#d9a7c0',value:'#fff',rule:'#38394a',grid:'#363746'};const longestLabel=Math.max(0,...[...card.querySelectorAll('td:first-child')].map(cell=>cell.textContent.length));const width=Math.max(720,Math.ceil(card.getBoundingClientRect().width),longestLabel*7.2+560);const padding=24;const contentWidth=width-padding*2;const title=card.querySelector('h2')?.textContent||providerName;const status=card.querySelector('.status')?.textContent||'';const metrics=[...card.querySelectorAll('.metric')].map(metric=>({label:metric.querySelector('span')?.textContent||'',value:metric.querySelector('b')?.textContent||''}));const tables=[...card.querySelectorAll('table')];const chart=card.querySelector('svg.chart');let y=padding;let body=`<rect width="100%" height="100%" rx="10" fill="${c.bg}" stroke="${c.border}" stroke-width="2"/><text x="${padding}" y="${y+28}" class="title">${escapeHtml(title)}</text>`;y+=55;body+=`<text x="${padding}" y="${y+2}" class="muted">${escapeHtml(status)}</text>`;y+=30;metrics.forEach(metric=>{body+=`<text x="${padding}" y="${y}" class="label">${escapeHtml(metric.label)}</text><text x="${width-padding}" y="${y}" text-anchor="end" class="value">${escapeHtml(metric.value)}</text><line x1="${padding}" x2="${width-padding}" y1="${y+12}" y2="${y+12}" class="rule"/>`;y+=38});if(chart){const chartMarkup=chart.innerHTML.replaceAll('#e9b4c9',c.accent).replaceAll('#1e1f2b',c.bg).replaceAll('#363746',c.grid);body+=`<text x="${padding}" y="${y+20}" class="heading">Token usage trend</text><svg x="${padding}" y="${y+34}" width="${contentWidth}" height="250" viewBox="0 0 760 250">${chartMarkup}</svg>`;const legend=[...card.querySelectorAll('.legend span')];const colors=[c.accent,'#6aaea4','#d18b2f','#637fc4','#9271bc','#c45b5b'];if(legend.length)body+='<g class="legend">'+legend.map((item,index)=>`<circle cx="${padding+index*135}" cy="${y+304}" r="5" fill="${colors[index%colors.length]}"/><text x="${padding+index*135+12}" y="${y+309}">${escapeHtml(item.textContent||'')}</text>`).join('')+'</g>';y+=330}tables.forEach((table,tableIndex)=>{body+=`<text x="${padding}" y="${y+20}" class="heading">${tableIndex?'Providers':'Model burn'}</text>`;y+=42;const rows=[...table.querySelectorAll('tr')];const columns=Math.max(...rows.map(row=>row.querySelectorAll('th,td').length),1);rows.forEach((row,rowIndex)=>{[...row.querySelectorAll('th,td')].forEach((cell,columnIndex)=>{const x=columnIndex===0?padding:padding+contentWidth*columnIndex/(columns-1||1);const anchor=columnIndex===0?'start':'end';const color=rowIndex===0?c.muted:columnIndex===0?c.accent:c.text;body+=`<text x="${x}" y="${y}" text-anchor="${anchor}" fill="${color}" class="table-text">${escapeHtml(cell.textContent||'')}</text>`});body+=`<line x1="${padding}" x2="${width-padding}" y1="${y+10}" y2="${y+10}" class="rule"/>`;y+=28});y+=12});const height=y+padding;const svg=`<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><style>.title{fill:${c.accent};font:700 20px ui-monospace,SFMono-Regular,Menlo,monospace}.heading{fill:${c.heading};font:700 15px ui-monospace,SFMono-Regular,Menlo,monospace}.muted{fill:${c.muted};font:12px ui-monospace,SFMono-Regular,Menlo,monospace}.label,.table-text{font:12px ui-monospace,SFMono-Regular,Menlo,monospace}.label{fill:${c.text}}.value{fill:${c.value};font:700 12px ui-monospace,SFMono-Regular,Menlo,monospace}.rule{stroke:${c.rule}}.legend text{fill:${c.muted};font:11px ui-monospace,SFMono-Regular,Menlo,monospace}.chart-grid{stroke:${c.grid};stroke-width:1}.chart-axis{fill:${c.muted};font-size:10px}.chart-line{fill:none;stroke:${c.accent};stroke-width:2.5;stroke-linecap:round;stroke-linejoin:round}.model-line{fill:none;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round;opacity:.9}.chart-area{fill:url(#trend-fill);opacity:.35}.chart-dot{fill:${c.bg};stroke:${c.accent};stroke-width:2}.model-dot{fill:${c.bg};stroke-width:1.5}</style>${body}</svg>`;const blob=new Blob([svg],{type:'image/svg+xml;charset=utf-8'});const link=document.createElement('a');link.download='agentusage-'+providerName.toLowerCase().replace(/[^a-z0-9]+/g,'-')+'-'+windowName+'.svg';link.href=URL.createObjectURL(blob);document.body.appendChild(link);link.click();setTimeout(()=>{URL.revokeObjectURL(link.href);link.remove()},10000)}
const renderDashboard=load;load=async()=>{await renderDashboard();prepareChartTooltips();const nav=document.querySelector('#detail-nav');nav.hidden=!detailProvider;app.classList.toggle('detail-view',Boolean(detailProvider));const cards=[...app.querySelectorAll('.card')];cards.forEach(card=>{const title=card.querySelector('h2')?.textContent;if(detailProvider?title!==detailProvider:hiddenProviders.has(title))card.remove()});updateHiddenControl();if(detailProvider&&!app.querySelector('.card'))app.innerHTML='<p class="error loading">Unknown provider</p>'};
applyTheme();load();
</script>
</body>
</html>"##;

#[cfg(test)]
mod tests {
    use super::index_html;

    #[test]
    fn dashboard_contains_expected_mount_and_api_calls() {
        let page = index_html();
        assert!(page.contains("id=\"app\""));
        assert!(page.contains("/api/providers"));
        assert!(page.contains("/api/summary"));
        assert!(page.contains("/api/trend"));
        assert!(page.contains("Token usage trend"));
        assert!(page.contains("model-line"));
        assert!(page.contains("Download SVG"));
        assert!(page.contains("downloadCard"));
        assert!(page.contains("View full page"));
        assert!(page.contains("Hide card"));
        assert!(page.contains("show-hidden"));
        assert!(page.contains("main.detail-view"));
        assert!(!page.contains("Download PNG"));
        assert!(page.contains("theme-toggle"));
        assert!(page.contains("data-theme=\"light\""));
    }
}

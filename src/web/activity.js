// Each column represents one measured interval. The curve and every lane use its center.
const compact = new Intl.NumberFormat(undefined, {notation: 'compact', maximumFractionDigits: 1});
const exact = new Intl.NumberFormat();
const palette = ['#1885d0', '#9762c8', '#0e9d92', '#d08037', '#d26395', '#717ad3'];
const escapeHtml = value => String(value).replace(/[&<>"']/g, c =>
  ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));
const sum = values => values.reduce((a, b) => a + b, 0);
const hourlyLabel = new Intl.DateTimeFormat(undefined, {hour: '2-digit', minute: '2-digit', hourCycle: 'h23'});
const dailyLabel = new Intl.DateTimeFormat(undefined, {month: 'short', day: 'numeric'});
function timeLabel(point, hourly) {
  return hourly ? hourlyLabel.format(new Date(point.bucket_start))
    : dailyLabel.format(new Date(point.date + 'T12:00:00'));
}
function activityPanel(points, series, total, hourly, subtitle, kind) {
  const panel = document.createElement('section');
  panel.className = 'panel activity';
  const n = points.length;
  const labels = points.map(p => timeLabel(p, hourly));
  panel.innerHTML = '<div class="panel-head"><div><h2>Token activity</h2><p class="sub"></p></div>' +
    '<span class="period"></span></div>';
  panel.querySelector('.sub').textContent = subtitle;
  panel.querySelector('.period').textContent = hourly ? 'Tokens per hour' : 'Tokens per day';
  if (!n || !total.some(v => v > 0)) {
    panel.insertAdjacentHTML('beforeend', '<p class="empty">No token usage recorded in this period.</p>');
    return panel;
  }
  panel.insertAdjacentHTML('beforeend', '<div class="plot-row"><div class="readout"><span class="caption">Total tokens</span>' +
    '<strong class="big"></strong><small></small></div><div class="graph"><div class="plot" role="slider" tabindex="0" ' +
    'aria-label="Inspect token activity" aria-valuemin="1"><svg viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden="true"></svg>' +
    '<span class="cursor"></span><i class="point"></i><span class="callout"></span></div><div class="ticks" aria-hidden="true"></div></div></div>' +
    '<div class="lanes"></div><p class="plot-note"></p>');
  const plot = panel.querySelector('.plot');
  const svg = plot.querySelector('svg');
  const lanes = panel.querySelector('.lanes');
  const center = i => (i + .5) / n;
  const tickCount = n > 4 ? 5 : n;
  for (let t = 0; t < tickCount; t++) {
    const i = Math.round(t * (n - 1) / Math.max(1, tickCount - 1));
    const el = document.createElement('span');
    el.textContent = labels[i];
    el.style.left = center(i) * 100 + '%';
    if (t === 0) el.style.transform = 'none';
    else if (t === tickCount - 1) el.style.transform = 'translateX(-100%)';
    if (t % 2 && tickCount === 5) el.className = 'minor';
    panel.querySelector('.ticks').append(el);
  }
  let selected = null, values = total, peak = 0, inspected = 0, max = 1;
  const laneMax = Math.max(1, ...series.map(s => Math.max(...s.values)));
  const laneButtons = series.map(s => {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'lane';
    button.style.setProperty('--series', s.color);
    button.style.setProperty('--bins', n);
    button.setAttribute('aria-label', 'Filter chart to ' + s.name);
    button.innerHTML = '<span class="lane-key"><i class="dot"></i><span class="lane-name"></span><small></small></span><span class="track" aria-hidden="true"></span>';
    button.querySelector('.lane-name').textContent = s.name;
    button.querySelector('small').textContent = compact.format(sum(s.values));
    const track = button.querySelector('.track');
    for (const [i, v] of s.values.entries()) {
      const bin = document.createElement('span');
      bin.className = 'bin';
      bin.title = s.name + ' · ' + labels[i] + ' · ' + exact.format(v) + ' tokens';
      if (v > 0) {
        const fill = document.createElement('i');
        fill.style.opacity = .24 + .76 * Math.sqrt(v / laneMax);
        bin.append(fill);
      }
      track.append(bin);
    }
    button.addEventListener('click', () => { selected = selected === s ? null : s; draw(); });
    lanes.append(button);
    return button;
  });
  function inspect(i) {
    inspected = Math.min(n - 1, Math.max(0, i));
    const v = values[inspected], left = center(inspected) * 100 + '%';
    plot.querySelector('.cursor').style.left = left;
    lanes.style.setProperty('--cursor-x', left);
    const dot = plot.querySelector('.point');
    dot.style.left = left;
    dot.style.top = (100 - v / max * 88) + '%';
    const callout = plot.querySelector('.callout');
    callout.textContent = (inspected === peak && v ? 'Peak ' : '') + compact.format(v) + ' · ' + labels[inspected];
    callout.style.left = 'clamp(50px,' + left + ',calc(100% - 50px))';
    plot.setAttribute('aria-valuemax', n);
    plot.setAttribute('aria-valuenow', inspected + 1);
    plot.setAttribute('aria-valuetext', labels[inspected] + ', ' + exact.format(v) + ' tokens');
    const top = series.reduce((best, s) => !best || s.values[inspected] > best.values[inspected] ? s : best, null);
    const parts = [labels[inspected], exact.format(v) + ' tokens'];
    if (!selected && top && v) parts.push(top.name + ' contributed ' + exact.format(top.values[inspected]));
    panel.querySelector('.plot-note').textContent = parts.join('  ·  ');
  }
  function draw() {
    values = selected ? selected.values : total;
    peak = values.indexOf(Math.max(...values));
    max = Math.max(1, values[peak]);
    const color = selected?.color || 'var(--blue)';
    panel.style.setProperty('--series', color);
    panel.querySelector('.caption').textContent = selected ? selected.name + ' tokens' : 'Total tokens';
    panel.querySelector('.big').textContent = compact.format(sum(values));
    panel.querySelector('.big').title = exact.format(sum(values));
    panel.querySelector('.readout small').textContent = selected ? 'Select row again to show all' : kind;
    laneButtons.forEach((b, i) => b.setAttribute('aria-pressed', String(series[i] === selected)));
    const path = values.map((v, i) => (i ? 'L' : 'M') + (center(i)*1000).toFixed(2) + ',' + (100-v/max*88).toFixed(2)).join(' ');
    const grid = [12,56,100].map(y => '<line class="gridline" x1="0" x2="1000" y1="'+y+'" y2="'+y+'"/>').join('');
    svg.innerHTML = '<defs><linearGradient id="activity-fill" x1="0" x2="0" y1="0" y2="1"><stop stop-color="'+color+'" stop-opacity=".17"/>' +
      '<stop offset="1" stop-color="'+color+'" stop-opacity=".015"/></linearGradient></defs>' + grid +
      '<path fill="url(#activity-fill)" d="'+path+' L'+center(n-1)*1000+',100 L'+center(0)*1000+',100 Z"/><path class="curve" d="'+path+'"/>';
    plot.querySelectorAll('.scale').forEach(node => node.remove());
    for (const [at, v] of [[12,max],[56,max/2],[100,0]]) {
      const label = document.createElement('span');
      label.className = 'scale';
      label.style.top = at + '%';
      label.textContent = compact.format(v);
      plot.append(label);
    }
    inspect(peak);
  }
  function inspectPointer(event) {
    const box = plot.getBoundingClientRect();
    inspect(Math.floor((event.clientX - box.left) / box.width * n));
  }
  plot.addEventListener('pointermove', inspectPointer);
  plot.addEventListener('pointerdown', inspectPointer);
  plot.addEventListener('pointerleave', () => inspect(peak));
  plot.addEventListener('keydown', event => {
    const next = {ArrowLeft: inspected - 1, ArrowRight: inspected + 1, Home: 0, End: n - 1}[event.key];
    if (next !== undefined) { event.preventDefault(); inspect(next); }
  });
  draw();
  return panel;
}

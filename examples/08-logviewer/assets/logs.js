(function () {
  'use strict';

  const processSelect = document.getElementById('lv-process');
  const autoscrollBox = document.getElementById('lv-autoscroll');
  const clearBtn      = document.getElementById('lv-clear');
  const tbody         = document.getElementById('lv-body');
  const status        = document.getElementById('lv-status');

  // Per-process byte offsets. Reset when process selection changes.
  const offsets = {};

  let pollTimer = null;

  function currentProcess() {
    return processSelect.value;
  }

  function setStatus(msg) {
    status.textContent = msg;
  }

  function escapeHtml(s) {
    return s
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function levelClass(level) {
    switch ((level || '').toUpperCase()) {
      case 'ERROR': return 'lv-error';
      case 'WARN':  return 'lv-warn';
      case 'DEBUG': return 'lv-debug';
      case 'TRACE': return 'lv-trace';
      default:      return 'lv-info';
    }
  }

  function appendEntry(proc, entry) {
    // tracing-subscriber JSON format:
    // {"timestamp":"…","level":"INFO","fields":{"message":"…"},"target":"…"}
    const ts     = entry.timestamp ? entry.timestamp.replace('T', ' ').replace(/\.\d+Z$/, 'Z') : '—';
    const level  = entry.level || '?';
    const target = entry.target || proc;
    const msg    = (entry.fields && entry.fields.message) ? entry.fields.message : JSON.stringify(entry.fields || entry);

    // Extra fields (everything except message).
    const extras = [];
    if (entry.fields) {
      for (const [k, v] of Object.entries(entry.fields)) {
        if (k !== 'message') extras.push(k + '=' + JSON.stringify(v));
      }
    }
    const fullMsg = extras.length ? msg + '  ' + extras.join(' ') : msg;

    const tr = document.createElement('tr');
    tr.className = levelClass(level);
    tr.innerHTML =
      '<td class="col-time">'   + escapeHtml(ts)      + '</td>' +
      '<td class="col-level">'  + escapeHtml(level)   + '</td>' +
      '<td class="col-target">' + escapeHtml(target)  + '</td>' +
      '<td class="col-msg">'    + escapeHtml(fullMsg) + '</td>';
    tbody.appendChild(tr);
  }

  async function poll() {
    const proc   = currentProcess();
    const offset = offsets[proc] || 0;
    let resp;
    try {
      resp = await fetch('/logs/tail/' + proc + '.log?offset=' + offset);
    } catch (e) {
      setStatus('Network error: ' + e.message);
      return;
    }

    if (!resp.ok) {
      setStatus(proc + '.log — HTTP ' + resp.status);
      return;
    }

    const newOffset = resp.headers.get('X-Log-End');
    if (newOffset !== null) offsets[proc] = parseInt(newOffset, 10);

    const text = await resp.text();
    if (!text.trim()) {
      setStatus('Watching ' + proc + '.log — no new entries');
      return;
    }

    let count = 0;
    for (const line of text.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const entry = JSON.parse(trimmed);
        appendEntry(proc, entry);
        count++;
      } catch (_) {
        // Non-JSON line (e.g. text format); show as plain row.
        const tr = document.createElement('tr');
        tr.className = 'lv-plain';
        tr.innerHTML =
          '<td class="col-time">—</td>' +
          '<td class="col-level">—</td>' +
          '<td class="col-target">—</td>' +
          '<td class="col-msg">' + escapeHtml(trimmed) + '</td>';
        tbody.appendChild(tr);
        count++;
      }
    }

    if (count > 0 && autoscrollBox.checked) {
      tbody.lastElementChild && tbody.lastElementChild.scrollIntoView({ behavior: 'smooth' });
    }
    setStatus('Watching ' + proc + '.log — ' + (offsets[proc] || 0) + ' bytes read');
  }

  function startPolling() {
    if (pollTimer) clearInterval(pollTimer);
    poll();
    pollTimer = setInterval(poll, 1000);
  }

  processSelect.addEventListener('change', function () {
    tbody.innerHTML = '';
    setStatus('Switching to ' + currentProcess() + '.log…');
    startPolling();
  });

  clearBtn.addEventListener('click', function () {
    tbody.innerHTML = '';
  });

  startPolling();
})();

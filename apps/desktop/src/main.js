// FH6 HUD 配置界面 —— 前端逻辑（无框架、无构建步骤）。
//
// 设计要点：
//  * 用 `data-path` 属性把每个控件绑定到配置里的一个 JSON 路径，
//    填表与收集都走同一条路径解析函数，新增字段只需加一个 input；
//  * 配置的唯一真相在 Rust 侧（`AppConfig`），这里只做「读出来 → 填表 →
//    收集 → 存回去」，不在前端做任何校验逻辑，避免两处规则不一致；
//  * 所有后端调用都包一层 try/catch，失败用 toast 明示，不静默。

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

let currentConfig = null;
let currentStatus = null;

// ---------------------------------------------------------------- 工具

function getByPath(obj, path) {
  return path.split('.').reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

function setByPath(obj, path, value) {
  const parts = path.split('.');
  const last = parts.pop();
  let cur = obj;
  for (const k of parts) {
    if (cur[k] == null || typeof cur[k] !== 'object') cur[k] = {};
    cur = cur[k];
  }
  cur[last] = value;
}

function toast(msg, kind = 'ok') {
  const el = document.getElementById('toast');
  el.textContent = msg;
  el.className = `toast show ${kind}`;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => { el.className = 'toast'; }, 3200);
}

// 常驻动作消息（不自动消失）。
//
// 与 toast 的分工：toast 只用于「已保存」这类一眼即知的结果；
// **失败**必须留在界面上 —— 启动失败的原因（找不到 exe、端口被占）
// 需要用户能反复阅读，弹一下就消失等于没提示。
// 复制文本到剪贴板。
//
// 不用 navigator.clipboard：它在 tauri:// 自定义协议下可能不在安全上下文里而被拒。
// 隐藏 textarea + document.execCommand('copy') 在 WebView2 里表现稳定。
function copyText(text) {
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    document.body.removeChild(ta);
    return true;
  } catch (e) {
    console.error('复制失败', e);
    return false;
  }
}

function setActionMsg(text, kind = '') {
  const el = document.getElementById('action-msg');
  if (!text) {
    el.hidden = true;
    el.textContent = '';
    el.className = 'action-msg';
    return;
  }
  el.hidden = false;
  el.textContent = text;
  el.className = `action-msg ${kind}`;
}

function looksLikeError(text) {
  return /失败|错误|找不到|拒绝|占用|already|error/i.test(text);
}

const OUT_IDS = {
  scale: 'scale-out', offx: 'offx-out', offy: 'offy-out',
  'op-input': 'op-input-out', 'op-slip': 'op-slip-out', 'op-flow': 'op-flow-out',
  'op-g': 'op-g-out', 'op-pop': 'op-pop-out', scan: 'scan-out',
};

function updateOutputs() {
  for (const [id, outId] of Object.entries(OUT_IDS)) {
    const inp = document.getElementById(id);
    const out = document.getElementById(outId);
    if (!inp || !out) continue;
    const v = parseFloat(inp.value);
    out.textContent = Number.isFinite(v) ? v.toFixed(2) : '—';
  }
}

// ---------------------------------------------------------------- 表单填/收集

function fillForm(cfg) {
  currentConfig = cfg;
  document.querySelectorAll('[data-path]').forEach((el) => {
    const path = el.dataset.path;
    let v = getByPath(cfg, path);
    if (v == null) return;
    if (path === 'logging.max_bytes') {
      // 界面以 KB 展示，配置里存字节
      v = Math.round(Number(v) / 1024);
    }
    if (el.type === 'checkbox') el.checked = Boolean(v);
    else el.value = String(v);
  });
  renderTargets(cfg.relay.targets || []);
  updateOutputs();
}

function collectForm() {
  const cfg = structuredClone(currentConfig);
  document.querySelectorAll('[data-path]').forEach((el) => {
    const path = el.dataset.path;
    let v;
    if (el.type === 'checkbox') v = el.checked;
    else if (el.type === 'number' || el.type === 'range') {
      v = parseFloat(el.value);
      if (!Number.isFinite(v)) v = 0;
    } else v = el.value;
    if (path === 'logging.max_bytes') v = Math.round(v * 1024);
    setByPath(cfg, path, v);
  });
  cfg.relay.targets = collectTargets();
  return cfg;
}

// ---------------------------------------------------------------- 分发目标

function renderTargets(targets) {
  const tb = document.querySelector('#targets tbody');
  tb.innerHTML = '';
  targets.forEach((t) => tb.appendChild(makeTargetRow(t)));
  updateTargetCount();
}

function makeTargetRow(t) {
  const tr = document.createElement('tr');

  const tdOn = document.createElement('td');
  const on = document.createElement('input');
  on.type = 'checkbox';
  on.className = 't-enabled';
  on.checked = Boolean(t.enabled);
  tdOn.appendChild(on);

  const tdLabel = document.createElement('td');
  const label = document.createElement('input');
  label.type = 'text';
  label.className = 't-label';
  label.value = t.label ?? '';
  label.placeholder = '例如：飞智自适应扳机';
  tdLabel.appendChild(label);

  const tdHost = document.createElement('td');
  const host = document.createElement('input');
  host.type = 'text';
  host.className = 't-host';
  host.value = t.host ?? '';
  host.placeholder = '127.0.0.1';
  tdHost.appendChild(host);

  const tdPort = document.createElement('td');
  const port = document.createElement('input');
  port.type = 'number';
  port.className = 't-port';
  port.min = '1';
  port.max = '65535';
  port.value = String(t.port ?? 0);
  tdPort.appendChild(port);

  const tdDel = document.createElement('td');
  const del = document.createElement('button');
  del.className = 'row-del';
  del.textContent = '删除';
  del.addEventListener('click', () => { tr.remove(); updateTargetCount(); });
  tdDel.appendChild(del);

  tr.append(tdOn, tdLabel, tdHost, tdPort, tdDel);
  return tr;
}

function collectTargets() {
  return [...document.querySelectorAll('#targets tbody tr')].map((tr) => ({
    enabled: tr.querySelector('.t-enabled').checked,
    label: tr.querySelector('.t-label').value,
    host: tr.querySelector('.t-host').value,
    port: parseInt(tr.querySelector('.t-port').value, 10) || 0,
  }));
}

function updateTargetCount() {
  const rows = document.querySelectorAll('#targets tbody tr');
  const enabled = [...rows].filter((tr) => tr.querySelector('.t-enabled').checked).length;
  document.getElementById('target-count').textContent =
    `共 ${rows.length} 个目标，启用 ${enabled} 个`;
}

// ---------------------------------------------------------------- 状态

function applyStatus(s) {
  currentStatus = s;
  document.getElementById('cfg-path').textContent = s.config_path || '—';

  const relayExe = document.getElementById('relay-exe');
  relayExe.textContent = s.relay_exe + (s.relay_exe_ok ? '' : '（未找到）');
  relayExe.className = 'path ' + (s.relay_exe_ok ? 'ok' : 'bad');

  const hudExe = document.getElementById('hud-exe');
  hudExe.textContent = s.hud_exe + (s.hud_exe_ok ? '' : '（未找到）');
  hudExe.className = 'path ' + (s.hud_exe_ok ? 'ok' : 'bad');

  setPill('pill-relay', s.relay_running);
  setPill('pill-hud', s.hud_running);
}

function setPill(id, running) {
  const el = document.getElementById(id);
  el.classList.toggle('on', running);
  el.classList.toggle('off', !running);
  el.querySelector('.state').textContent = running ? '运行中' : '已停止';
}

async function refreshStatus() {
  try {
    const cfg = currentConfig ? collectForm() : null;
    if (!cfg) return;
    applyStatus(await invoke('status', { config: cfg }));
    // auto_start 等开关即时反映
  } catch (e) {
    console.error(e);
  }
}

// ---------------------------------------------------------------- 动作

async function loadAll() {
  try {
    currentConfig = await invoke('load_config');
    fillForm(currentConfig);
    await refreshStatus();
  } catch (e) {
    toast(`读取配置失败：${e}`, 'err');
  }
}

// 保存配置。
//
// `restart` 为真时：若已有进程在运行，保存后**自动重启**它们 —— 因为 HUD 与
// relay 都只在启动时读一次配置，不重启的话改动永远不生效，用户会以为
// 「界面上改了但没反应」。这正是最初的反馈。
async function save(showToast = true, restart = true) {
  try {
    const cfg = collectForm();
    await invoke('save_config', { config: cfg });
    currentConfig = cfg;

    let msg = '配置已保存';
    if (restart) {
      const st = await invoke('status', { config: cfg });
      if (st.relay_running || st.hud_running) {
        try {
          await invoke('stop', { which: 'all' });
          await invoke('start', { config: cfg, which: 'all' });
          msg = '配置已保存，并已重启 HUD + relay';
        } catch (e) {
          msg = `配置已保存，但重启失败：${e}`;
        }
      }
    }
    await refreshStatus();
    if (showToast) toast(msg, msg.includes('失败') ? 'err' : 'ok');
  } catch (e) {
    toast(`保存失败：${e}`, 'err');
    throw e;
  }
}

// ---------------------------------------------------------------- 初始化

function bind() {
  // 滑杆数值实时显示
  Object.keys(OUT_IDS).forEach((id) => {
    const el = document.getElementById(id);
    if (el) el.addEventListener('input', updateOutputs);
  });

  // 目标启用状态变化时刷新计数
  document.querySelector('#targets').addEventListener('change', (e) => {
    if (e.target.classList.contains('t-enabled')) updateTargetCount();
  });

  document.getElementById('btn-add-target').addEventListener('click', () => {
    const tb = document.querySelector('#targets tbody');
    tb.appendChild(makeTargetRow({ enabled: true, label: '', host: '127.0.0.1', port: 5302 }));
    updateTargetCount();
  });

  document.getElementById('btn-save').addEventListener('click', () => save());

  document.getElementById('btn-reload').addEventListener('click', async () => {
    await loadAll();
    toast('已重载');
  });

  document.getElementById('btn-start').addEventListener('click', async () => {
    try {
      await save(false, false); // 先落盘，子进程读的是文件；此处不自动重启
      const msg = await invoke('start', { config: collectForm(), which: 'all' });
      // 部分成功（例如 relay 起来了、HUD 失败）也要留在界面上
      if (looksLikeError(msg)) setActionMsg(`启动结果：${msg}`, 'warn');
      else setActionMsg(`启动结果：${msg}`, 'ok');
      await refreshStatus();
    } catch (e) {
      setActionMsg(`启动失败：${e}`, 'err');
      await refreshStatus();
    }
  });

  document.getElementById('btn-restart').addEventListener('click', async () => {
    try {
      await save(false, false); // 先确保磁盘上是最新配置
      await invoke('stop', { which: 'all' });
      const msg = await invoke('start', { config: collectForm(), which: 'all' });
      if (looksLikeError(msg)) setActionMsg(`重启结果：${msg}`, 'warn');
      else setActionMsg(`重启结果：${msg}`, 'ok');
      await refreshStatus();
    } catch (e) {
      setActionMsg(`重启失败：${e}`, 'err');
      await refreshStatus();
    }
  });
  document.getElementById('btn-stop').addEventListener('click', async () => {
    try {
      const msg = await invoke('stop', { which: 'all' });
      setActionMsg(msg, 'ok');
      await refreshStatus();
    } catch (e) {
      setActionMsg(`停止失败：${e}`, 'err');
    }
  });

  // ---- 界面内日志查看（不依赖 explorer）----
  let logViews = [];
  let logTab = 'hud';

  function renderLogTab() {
    const view = logViews.find((v) => v.name === logTab);
    const pre = document.getElementById('log-content');
    const pathEl = document.getElementById('log-path');
    if (!view) {
      pre.textContent = '（没有数据）';
      pathEl.textContent = '';
      return;
    }
    pathEl.textContent = view.path;
    if (!view.exists) {
      pre.textContent = `日志文件不存在：\n${view.path}\n\n请在「日志文件」区启用日志并保存，然后重启 HUD/relay。`;
    } else if (!view.content) {
      pre.textContent = '（文件是空的）';
    } else {
      pre.textContent = view.content;
      pre.scrollTop = pre.scrollHeight;
    }
    document.getElementById('log-tab-hud').classList.toggle('active', logTab === 'hud');
    document.getElementById('log-tab-relay').classList.toggle('active', logTab === 'relay');
  }

  async function refreshLogs() {
    try {
      logViews = await invoke('read_logs', { config: collectForm(), maxLines: 200 });
      renderLogTab();
    } catch (e) {
      document.getElementById('log-content').textContent = `读取日志失败：${e}`;
    }
  }

  document.getElementById('btn-logview').addEventListener('click', async () => {
    document.getElementById('log-overlay').classList.remove('hidden');
    await refreshLogs();
  });
  document.getElementById('log-tab-hud').addEventListener('click', () => {
    logTab = 'hud';
    renderLogTab();
  });
  document.getElementById('log-tab-relay').addEventListener('click', () => {
    logTab = 'relay';
    renderLogTab();
  });
  document.getElementById('log-reload').addEventListener('click', refreshLogs);
  document.getElementById('log-close').addEventListener('click', () => {
    document.getElementById('log-overlay').classList.add('hidden');
  });

  document.getElementById('btn-log').addEventListener('click', async () => {
    try {
      const r = await invoke('open_log_dir', { config: collectForm() });
      if (r.opened) {
        setActionMsg(`已在资源管理器中打开：${r.path}`, 'ok');
      } else {
        // 本机实测：从本程序启动 explorer 会被拒绝（错误 5），但打开文件正常。
        // 这种情况不该只报错 —— 把路径交给用户，并支持一键复制。
        setActionMsg(
          `无法自动打开资源管理器（${r.error || '未知原因'}）。日志目录：${r.path}`,
          'warn',
        );
        copyText(r.path);
      }
    } catch (e) {
      setActionMsg(`打开目录失败：${e}`, 'err');
    }
  });

  document.getElementById('btn-cfg-default').addEventListener('click', async () => {
    try {
      const p = await invoke('default_path');
      await invoke('set_config_path', { path: p });
      await loadAll();
      toast('已切换到默认配置位置');
    } catch (e) {
      toast(`切换失败：${e}`, 'err');
    }
  });

  // Ctrl+S 保存
  window.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      save();
    }
  });
}

window.addEventListener('DOMContentLoaded', async () => {
  bind();
  await loadAll();
  setInterval(refreshStatus, 2000);
});

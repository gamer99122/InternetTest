// 前端只負責「畫」。所有判斷、所有給使用者看的文字都在 Rust 那邊決定好了，
// 這裡不做任何門檻比較，也不組任何診斷句子 —— 否則同一套規則會散在兩個地方。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const el = {
  brandDot: $("brandDot"),
  topbarMeta: $("topbarMeta"),
  ringValue: $("ringValue"),
  ringNum: $("ringNum"),
  headline: $("headline"),
  explanation: $("explanation"),
  advice: $("advice"),
  startBtn: $("startBtn"),
  startBtnText: $("startBtnText"),
  reportBtn: $("reportBtn"),
  heroStats: $("heroStats"),
  elapsedText: $("elapsedText"),
  outageText: $("outageText"),
  path: $("path"),
  chartCard: $("chartCard"),
  chart: $("chart"),
  chartLegend: $("chartLegend"),
  eventsCard: $("eventsCard"),
  events: $("events"),
  extrasCard: $("extrasCard"),
  wifiExtra: $("wifiExtra"),
  wifiValue: $("wifiValue"),
  dnsValue: $("dnsValue"),
  ifaceValue: $("ifaceValue"),
  folderBtn: $("folderBtn"),
  toast: $("toast"),
  externalTests: $("externalTests"),
};

// 圓環半徑 52 的周長，和 CSS 裡的 stroke-dasharray 對應
const RING_CIRCUMFERENCE = 2 * Math.PI * 52;

let busy = false;
let latest = null;
let toastTimer = null;

// ---------- 小工具 ----------

function showToast(message) {
  el.toast.textContent = message;
  el.toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    el.toast.hidden = true;
  }, 3600);
}

function setClass(node, base, variant) {
  node.className = variant ? `${base} ${variant}` : base;
}

function msText(value) {
  return value === null || value === undefined ? "—" : `${Math.round(value)} 毫秒`;
}

// ---------- 畫面更新 ----------

function render(snap) {
  latest = snap;
  el.externalTests.disabled = snap.running || busy;

  // 標題列
  setClass(el.brandDot, "brand-dot", snap.running
    ? `${snap.verdict.grade} live`
    : snap.verdict.grade);
  el.topbarMeta.textContent = snap.running ? "檢測中" : "";

  // 圓環。還沒開始檢測時不要顯示 100%，那會讓人以為已經測過了
  const showRing = snap.running || snap.elapsed_secs > 0;
  const pct = showRing ? snap.stability_pct : 0;
  el.ringNum.textContent = showRing ? `${pct.toFixed(0)}%` : "—";
  el.ringValue.setAttribute("class", `ring-value ${snap.verdict.grade}`);
  el.ringValue.style.strokeDashoffset =
    RING_CIRCUMFERENCE * (1 - pct / 100);

  // 結論
  el.headline.textContent = snap.verdict.headline;
  el.explanation.textContent = snap.verdict.explanation;
  if (snap.verdict.suggestion) {
    el.advice.textContent = `建議：${snap.verdict.suggestion}`;
    el.advice.hidden = false;
  } else {
    el.advice.hidden = true;
  }

  // 按鈕
  el.startBtnText.textContent = snap.running ? "停止檢測" : "開始檢測";
  el.startBtn.classList.toggle("running", snap.running);
  el.reportBtn.hidden = snap.elapsed_secs === 0;

  // 統計
  el.heroStats.hidden = snap.elapsed_secs === 0;
  el.elapsedText.textContent = snap.elapsed_text;
  el.outageText.textContent = snap.outage_text;

  renderPath(snap);
  renderChart(snap);
  renderEvents(snap);
  renderExtras(snap);
}

function renderPath(snap) {
  if (!snap.segments.length) return;

  el.path.replaceChildren();

  // 最前面補一張代表使用者這台電腦的卡片，路徑才完整
  el.path.appendChild(makeHop({
    label: "這台電腦",
    description: "你正在使用的裝置",
    status_text: "起點",
    color: "",
    last_ms: null,
    loss_pct: 0,
  }, false));

  snap.segments.forEach((seg) => {
    el.path.appendChild(makeArrow());
    el.path.appendChild(makeHop(seg, snap.verdict.culprit === seg.kind));
  });
}

function makeArrow() {
  const div = document.createElement("div");
  div.className = "arrow";
  div.textContent = "→";
  return div;
}

function makeHop(seg, isCulprit) {
  const box = document.createElement("div");
  box.className = `hop ${seg.color}${isCulprit ? " culprit" : ""}`;

  const name = document.createElement("div");
  name.className = "hop-name";
  name.textContent = seg.label;

  const desc = document.createElement("div");
  desc.className = "hop-desc";
  desc.textContent = seg.description;

  const status = document.createElement("div");
  status.className = "hop-status";
  const led = document.createElement("span");
  led.className = "hop-led";
  const statusText = document.createElement("span");
  statusText.textContent = seg.status_text;
  status.append(led, statusText);

  box.append(name, desc, status);

  // 起點那張卡沒有延遲數字
  if (seg.last_ms !== null && seg.last_ms !== undefined) {
    const ms = document.createElement("div");
    ms.className = "hop-ms";
    ms.textContent = `反應 ${msText(seg.last_ms)}`;
    box.appendChild(ms);
  }

  return box;
}

function renderEvents(snap) {
  el.eventsCard.hidden = snap.elapsed_secs === 0;
  el.events.replaceChildren();

  if (!snap.events.length) {
    const li = document.createElement("li");
    li.className = "events-empty";
    li.textContent = snap.running
      ? "目前一切正常，還沒有發生過斷線。"
      : "這次檢測期間沒有發生斷線。";
    el.events.appendChild(li);
    return;
  }

  snap.events.forEach((event) => {
    const li = document.createElement("li");

    const time = document.createElement("span");
    time.className = "event-time";
    time.textContent = event.started_text;

    const text = document.createElement("span");
    text.className = "event-text";
    text.textContent = event.text;

    li.append(time, text);
    el.events.appendChild(li);
  });
}

function renderExtras(snap) {
  el.extrasCard.hidden = snap.elapsed_secs === 0;

  if (snap.wifi_text) {
    el.wifiExtra.hidden = false;
    el.wifiValue.textContent = snap.wifi_text;
  } else {
    el.wifiExtra.hidden = true;
  }

  el.dnsValue.textContent = snap.dns.avg_ms === null
    ? snap.dns.status_text
    : `${snap.dns.status_text}（${msText(snap.dns.avg_ms)}）`;
  setClass(el.dnsValue, "extra-value", snap.dns.color);

  el.ifaceValue.textContent = snap.interface_name || "—";
}

// ---------- 折線圖 ----------

function renderChart(snap) {
  const points = snap.chart || [];
  el.chartCard.hidden = snap.elapsed_secs === 0;
  if (el.chartCard.hidden) return;

  const canvas = el.chart;
  const ctx = canvas.getContext("2d");

  // 高解析螢幕上不做這件事，線條會糊掉
  const dpr = window.devicePixelRatio || 1;
  const cssWidth = canvas.clientWidth || 900;
  const cssHeight = 180;
  if (canvas.width !== Math.round(cssWidth * dpr)) {
    canvas.width = Math.round(cssWidth * dpr);
    canvas.height = Math.round(cssHeight * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cssWidth, cssHeight);

  const style = getComputedStyle(document.documentElement);
  const lineColor = style.getPropertyValue(`--${snap.verdict.grade}`).trim() || "#1d4ed8";
  const gridColor = style.getPropertyValue("--line").trim() || "#e5e7eb";
  const failColor = style.getPropertyValue("--poor").trim() || "#b91c1c";

  const padding = { top: 14, right: 8, bottom: 18, left: 8 };
  const w = cssWidth - padding.left - padding.right;
  const h = cssHeight - padding.top - padding.bottom;

  const values = points.filter((v) => v !== null && v !== undefined);
  // 上限至少留 50ms，否則區網那種個位數延遲會被放大成劇烈震盪，看起來很嚇人
  const maxValue = Math.max(50, ...values) * 1.15;

  // 水平參考線
  ctx.strokeStyle = gridColor;
  ctx.lineWidth = 1;
  for (let i = 0; i <= 2; i++) {
    const y = padding.top + (h / 2) * i;
    ctx.beginPath();
    ctx.moveTo(padding.left, y);
    ctx.lineTo(padding.left + w, y);
    ctx.stroke();
  }

  if (!points.length) return;

  const stepX = points.length > 1 ? w / (points.length - 1) : w;
  const xAt = (i) => padding.left + stepX * i;
  const yAt = (v) => padding.top + h - (Math.min(v, maxValue) / maxValue) * h;

  // 失敗的點畫成紅色直條，一眼就看得出什麼時候斷了
  ctx.fillStyle = failColor;
  points.forEach((v, i) => {
    if (v === null || v === undefined) {
      ctx.globalAlpha = 0.22;
      ctx.fillRect(xAt(i) - stepX / 2, padding.top, Math.max(stepX, 2), h);
      ctx.globalAlpha = 1;
    }
  });

  // 折線本身。遇到失敗的點就斷開，不要用直線硬連過去造成誤導
  ctx.strokeStyle = lineColor;
  ctx.lineWidth = 2.2;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";

  let drawing = false;
  ctx.beginPath();
  points.forEach((v, i) => {
    if (v === null || v === undefined) {
      drawing = false;
      return;
    }
    if (!drawing) {
      ctx.moveTo(xAt(i), yAt(v));
      drawing = true;
    } else {
      ctx.lineTo(xAt(i), yAt(v));
    }
  });
  ctx.stroke();

  const target = snap.segments.find((s) => s.kind === "international")
    || snap.segments[snap.segments.length - 1];
  el.chartLegend.textContent = target
    ? `以「${target.label}」為準　平均 ${msText(target.avg_ms)}　最慢 ${msText(target.max_ms)}`
    : "";
}

// ---------- 操作 ----------

async function refresh() {
  try {
    render(await invoke("get_snapshot"));
  } catch (err) {
    showToast(`讀取狀態失敗：${err}`);
  }
}

async function toggleMonitoring() {
  if (busy) return;
  busy = true;
  el.externalTests.disabled = true;
  el.startBtn.disabled = true;

  try {
    if (latest && latest.running) {
      render(await invoke("stop_monitoring"));
      showToast("已停止檢測。可以按「產生報告」把結果存下來。");
    } else {
      el.startBtnText.textContent = "正在偵測網路…";
      await invoke("start_monitoring", { external: el.externalTests.checked });
      await refresh();
    }
  } catch (err) {
    showToast(String(err));
    await refresh();
  } finally {
    busy = false;
    el.startBtn.disabled = false;
    el.externalTests.disabled = Boolean(latest && latest.running);
  }
}

async function saveReport() {
  el.reportBtn.disabled = true;
  try {
    await invoke("save_report");
    showToast("報告已存到「文件」的「網路健檢報告」資料夾，並已為你打開。");
  } catch (err) {
    showToast(String(err));
  } finally {
    el.reportBtn.disabled = false;
  }
}

// ---------- 啟動 ----------

el.startBtn.addEventListener("click", toggleMonitoring);
el.reportBtn.addEventListener("click", saveReport);
el.folderBtn.addEventListener("click", async () => {
  try {
    await invoke("open_output_folder");
  } catch (err) {
    showToast(String(err));
  }
});

// 視窗大小改變時折線圖要重畫，否則會被拉扯變形
window.addEventListener("resize", () => {
  if (latest) renderChart(latest);
});

listen("snapshot", (event) => render(event.payload));

// 偵測網路環境要花幾秒（要做路徑追蹤），期間給使用者一點回饋
listen("detecting", () => {
  el.headline.textContent = "正在檢查你的網路…";
  el.explanation.textContent = "正在確認本次測試的網路目標，請稍候。";
  el.advice.hidden = true;
});

refresh();

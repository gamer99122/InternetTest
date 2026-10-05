//! 監測引擎。
//!
//! 一個背景 task，每秒同時探測四段路徑，把結果推給前端。
//! 所有給使用者看的字串都在這裡組好，前端只負責畫，不做任何判斷。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::icmp::{IcmpHandle, Outcome};
use crate::stats::{Grade, Sample, Summary, Window};
use crate::storage::Logger;
use crate::targets::{Segment, SegmentKind, Topology};
use crate::traceroute::{self, Hop};
use crate::verdict::{self, DnsHealth, Reading, Verdict};
use crate::wifi::WifiStatus;

/// 每秒探測一次。再密會變成在測自己的 CPU，再疏會漏掉短暫的斷線。
const TICK: Duration = Duration::from_secs(1);
/// 單次探測的等待上限。超過這個時間即使回來了也已經不能用了。
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// 滑動視窗長度：最近 60 秒。
const WINDOW: usize = 60;
/// 折線圖上顯示幾個點。
const CHART_POINTS: usize = 60;
/// DNS 每幾拍量一次。每秒都查會給 DNS 伺服器不必要的壓力。
const DNS_EVERY: u64 = 5;
/// Wi-Fi 訊號每幾拍讀一次。開子行程有成本，不需要每秒讀。
const WIFI_EVERY: u64 = 5;
/// 事件清單最多留幾筆。
const MAX_EVENTS: usize = 500;
/// 判定斷線的依據段落：連不到國外就是使用者認知的「沒網路」。
const OUTAGE_SEGMENT: SegmentKind = SegmentKind::International;

/// 一次斷線事件。
#[derive(Debug, Clone, Serialize)]
pub struct OutageEvent {
    pub started_at: DateTime<Local>,
    pub ended_at: Option<DateTime<Local>>,
    pub duration_secs: Option<i64>,
    /// 斷線當下判定的問題來源
    pub culprit: Option<SegmentKind>,
    /// 顯示在事件列表上的一句話
    pub text: String,
    /// 開始時間的顯示格式
    pub started_text: String,
    /// 斷線當下自動跑的路徑追蹤，用來事後追究斷在哪一跳
    pub trace: Option<Vec<Hop>>,
}

/// 一段路徑在畫面上的呈現。所有文字都已經轉成白話。
#[derive(Debug, Clone, Serialize)]
pub struct SegmentView {
    pub kind: SegmentKind,
    pub label: String,
    pub description: String,
    pub detail: String,
    pub target: String,
    /// 「正常」「有點慢」「不穩定」「不通」
    pub status_text: String,
    /// CSS class：good / fair / poor / down
    pub color: String,
    pub last_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub loss_pct: f64,
    pub jitter_ms: Option<f64>,
    pub uptime_pct: f64,
    /// 小折線圖的資料點
    pub spark: Vec<Option<f64>>,
}

/// DNS 在畫面上的呈現。
#[derive(Debug, Clone, Serialize)]
pub struct DnsView {
    pub working: bool,
    pub avg_ms: Option<f64>,
    pub status_text: String,
    pub color: String,
}

/// 推給前端的完整狀態。
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub running: bool,
    /// 開始檢測到現在經過的秒數
    pub elapsed_secs: i64,
    /// 「已檢測 12 分鐘」
    pub elapsed_text: String,
    pub verdict: Verdict,
    pub segments: Vec<SegmentView>,
    /// 整體穩定度百分比，畫在中央的圓環
    pub stability_pct: f64,
    pub outage_count: usize,
    pub total_outage_secs: i64,
    /// 「斷線 2 次，總共 47 秒」
    pub outage_text: String,
    pub events: Vec<OutageEvent>,
    pub wifi: Option<WifiStatus>,
    pub wifi_text: Option<String>,
    pub dns: DnsView,
    /// 主折線圖（以國外那一段為準，最貼近使用者體感）
    pub chart: Vec<Option<f64>>,
    pub interface_name: String,
    pub isp_name: Option<String>,
    pub notes: Vec<String>,
    pub log_path: Option<String>,
}

/// 引擎的內部狀態。
struct State {
    running: bool,
    topology: Option<Topology>,
    windows: HashMap<SegmentKind, Window>,
    dns_samples: VecDeque<Option<f64>>,
    dns_working: bool,
    wifi: Option<WifiStatus>,
    started_at: Option<DateTime<Local>>,
    tick: u64,
    events: Vec<OutageEvent>,
    /// 目前正在進行中的斷線在 `events` 裡的位置
    open_outage: Option<usize>,
    logger: Option<Logger>,
    log_path: Option<PathBuf>,
}

impl State {
    fn new() -> Self {
        State {
            running: false,
            topology: None,
            windows: HashMap::new(),
            dns_samples: VecDeque::new(),
            dns_working: true,
            wifi: None,
            started_at: None,
            tick: 0,
            events: Vec::new(),
            open_outage: None,
            logger: None,
            log_path: None,
        }
    }

    fn reset(&mut self) {
        self.windows.clear();
        self.dns_samples.clear();
        self.dns_working = true;
        self.wifi = None;
        self.tick = 0;
        self.events.clear();
        self.open_outage = None;
        self.logger = None;
        self.log_path = None;
    }

    fn dns_health(&self) -> DnsHealth {
        let oks: Vec<f64> = self.dns_samples.iter().filter_map(|s| *s).collect();
        DnsHealth {
            working: self.dns_working,
            avg_ms: if oks.is_empty() {
                None
            } else {
                Some(oks.iter().sum::<f64>() / oks.len() as f64)
            },
        }
    }
}

/// 監測引擎的對外介面。
pub struct Monitor {
    state: Arc<Mutex<State>>,
    stop: Mutex<Arc<AtomicBool>>,
    start_gate: tokio::sync::Mutex<()>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            state: Arc::new(Mutex::new(State::new())),
            stop: Mutex::new(Arc::new(AtomicBool::new(false))),
            start_gate: tokio::sync::Mutex::new(()),
            task: Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        self.state.lock().running
    }

    /// 開始監測。會先花一兩秒偵測網路環境，然後啟動背景 task。
    pub async fn start(
        &self,
        app: AppHandle,
        log_dir: PathBuf,
        external: bool,
    ) -> anyhow::Result<()> {
        let _gate = self.start_gate.lock().await;
        if self.is_running() {
            return Ok(());
        }
        // Finish the previous run before reusing state or switching to internal-only mode.
        let previous = self.task.lock().take();
        if let Some(previous) = previous {
            let _ = previous.await;
        }
        let stop = Arc::new(AtomicBool::new(false));
        *self.stop.lock() = stop.clone();

        // 偵測環境要花時間（traceroute 找電信商），先讓前端知道在忙
        let _ = app.emit("detecting", ());
        let topology = crate::targets::detect(external).await;
        if stop.load(Ordering::SeqCst) {
            anyhow::bail!("檢測已取消");
        }
        if topology.segments.is_empty() {
            anyhow::bail!("找不到預設閘道，請交由資訊人員確認網路設定");
        }

        let started = Local::now();
        let logger = match Logger::create(&log_dir, started) {
            Ok(l) => Some(l),
            // 寫不了檔不該讓整個監測停擺，頂多之後產不出 CSV
            Err(_) => None,
        };

        {
            let mut s = self.state.lock();
            if stop.load(Ordering::SeqCst) {
                anyhow::bail!("檢測已取消");
            }
            s.reset();
            s.running = true;
            s.started_at = Some(started);
            for seg in &topology.segments {
                s.windows.insert(seg.kind, Window::new(seg.kind, WINDOW));
            }
            s.log_path = logger.as_ref().map(|l| l.path().to_path_buf());
            s.logger = logger;
            s.topology = Some(topology);
        }

        let state = self.state.clone();
        let task = tokio::spawn(async move {
            run_loop(state, stop, app).await;
        });
        *self.task.lock() = Some(task);

        Ok(())
    }

    /// 停止監測。已經收集到的資料會保留，還可以產生報告。
    pub fn stop(&self) {
        self.stop.lock().store(true, Ordering::SeqCst);
        let mut s = self.state.lock();
        s.running = false;
        // 結束時把還沒寫進磁碟的紀錄補上
        if let Some(logger) = s.logger.as_mut() {
            let _ = logger.flush();
        }
        // 收尾還在進行中的斷線事件
        close_open_outage(&mut s, Local::now());
    }

    pub fn snapshot(&self) -> Snapshot {
        build_snapshot(&self.state.lock())
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

/// 背景主迴圈。
async fn run_loop(state: Arc<Mutex<State>>, stop: Arc<AtomicBool>, app: AppHandle) {
    let mut ticker = tokio::time::interval(TICK);
    // 落後時直接跳過，不要為了補齊次數而連續猛打
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let (segments, tick_no) = {
            let s = state.lock();
            let segs = s
                .topology
                .as_ref()
                .map(|t| t.segments.clone())
                .unwrap_or_default();
            (segs, s.tick)
        };

        probe_round(&state, &segments).await;

        if stop.load(Ordering::SeqCst) {
            break;
        }
        if segments
            .iter()
            .any(|s| s.kind == SegmentKind::International)
            && tick_no % DNS_EVERY == 0
        {
            probe_dns(&state, tick_no).await;
        }
        if tick_no % WIFI_EVERY == 0 {
            let wifi = tokio::task::spawn_blocking(crate::wifi::status)
                .await
                .unwrap_or(None);
            state.lock().wifi = wifi;
        }

        if stop.load(Ordering::SeqCst) {
            break;
        }
        detect_outage(&state, &app, &stop).await;

        {
            let mut s = state.lock();
            s.tick += 1;
        }

        let snapshot = build_snapshot(&state.lock());
        let _ = app.emit("snapshot", &snapshot);
    }

    // 停下來之後再推一次，讓畫面上的狀態正確
    let snapshot = build_snapshot(&state.lock());
    let _ = app.emit("snapshot", &snapshot);
}

/// 同時探測所有段落。
///
/// 四段各自送出，不能一段一段來 —— 逾時各 1.5 秒的話循序做會超過一拍的時間。
async fn probe_round(state: &Arc<Mutex<State>>, segments: &[Segment]) {
    let mut tasks = Vec::with_capacity(segments.len());

    for seg in segments {
        let addr = seg.addr;
        let kind = seg.kind;
        tasks.push(tokio::task::spawn_blocking(move || {
            // IcmpSendEcho 是阻塞呼叫，一定要在 blocking 執行緒上跑
            let rtt = match IcmpHandle::open() {
                Ok(h) => {
                    let echo = h.echo(addr, PROBE_TIMEOUT, None);
                    match echo.outcome {
                        Outcome::Reached => echo.rtt.map(|d| d.as_secs_f64() * 1000.0),
                        _ => None,
                    }
                }
                // 開不了 handle 等同這次探測失敗
                Err(_) => None,
            };
            (kind, addr, rtt)
        }));
    }

    let at = Local::now();
    for task in tasks {
        let Ok((kind, addr, rtt)) = task.await else {
            continue;
        };
        let mut s = state.lock();
        if let Some(w) = s.windows.get_mut(&kind) {
            w.push(Sample { at, rtt_ms: rtt });
        }
        if let Some(logger) = s.logger.as_mut() {
            let _ = logger.log(at, kind, &addr.to_string(), rtt);
        }
    }
}

/// 量一次 DNS 解析耗時。
async fn probe_dns(state: &Arc<Mutex<State>>, seq: u64) {
    let probe = crate::dns::measure(seq).await;
    let mut s = state.lock();
    s.dns_working = probe.resolved;
    if s.dns_samples.len() >= WINDOW {
        s.dns_samples.pop_front();
    }
    s.dns_samples.push_back(probe.elapsed_ms);
}

/// 判斷斷線的開始與結束，並在開始時觸發路徑追蹤與桌面通知。
async fn detect_outage(state: &Arc<Mutex<State>>, app: &AppHandle, stop: &Arc<AtomicBool>) {
    let now = Local::now();

    let (culprit_label, just_started, just_ended) = {
        let mut s = state.lock();

        let reference = if s.windows.contains_key(&OUTAGE_SEGMENT) {
            OUTAGE_SEGMENT
        } else {
            SegmentKind::Router
        };
        let down = s
            .windows
            .get(&reference)
            .map(|w| w.summarize().grade == Grade::Down)
            .unwrap_or(false);

        // 找出當下最近的壞掉段落，這才是真正的斷點
        let culprit = nearest_broken(&s);
        let label = culprit
            .and_then(|k| segment_label(&s, k))
            .unwrap_or_else(|| "網路".to_string());

        let mut started = false;
        let mut ended = None;
        match (down, s.open_outage) {
            // 剛剛斷線
            (true, None) => {
                let text = match culprit {
                    Some(k) if k != OUTAGE_SEGMENT => {
                        format!("斷線了，看起來是{label}那一段的問題")
                    }
                    _ => "斷線了".to_string(),
                };
                s.events.push(OutageEvent {
                    started_at: now,
                    ended_at: None,
                    duration_secs: None,
                    culprit,
                    text,
                    started_text: now.format("%H:%M:%S").to_string(),
                    trace: None,
                });
                if s.events.len() > MAX_EVENTS {
                    s.events.remove(0);
                }
                s.open_outage = Some(s.events.len() - 1);
                started = true;
            }
            // 剛剛恢復
            (false, Some(_)) => {
                close_open_outage(&mut s, now);
                // 只有真的在這一拍結案，才會拿到秒數，所以通知不會重複發
                ended = s.events.last().and_then(|e| e.duration_secs);
            }
            _ => {}
        }

        (label, started, ended)
    };

    if let Some(secs) = just_ended {
        notify(
            app,
            "網路恢復了",
            &format!("剛才斷線了 {}", human_duration(secs)),
        );
    }

    if !just_started {
        return;
    }

    // --- 以下是「剛斷線」才做的事 ---
    notify(
        app,
        "網路斷線",
        &format!("目前連不上網路，可能是{culprit_label}的問題"),
    );

    // 趁還斷著的時候追蹤路徑，才看得出斷在哪一跳。
    // 丟到背景做，不能卡住主迴圈。
    let state2 = state.clone();
    let cancel = stop.clone();
    let index = state.lock().open_outage;
    tokio::spawn(async move {
        let target = {
            let s = state2.lock();
            s.topology.as_ref().and_then(|t| {
                t.segments
                    .iter()
                    .find(|seg| seg.kind == OUTAGE_SEGMENT)
                    .map(|seg| seg.addr)
            })
        };
        let Some(target) = target else { return };

        if let Ok(mut hops) = traceroute::trace_cancellable(
            target,
            15,
            Duration::from_millis(700),
            2,
            Some(cancel.clone()),
        )
        .await
        {
            traceroute::resolve_hostnames(&mut hops, &cancel).await;
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            let mut s = state2.lock();
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            if let Some(i) = index {
                if let Some(event) = s.events.get_mut(i) {
                    event.trace = Some(hops);
                }
            }
        }
    });
}

/// 把進行中的斷線事件結案。
fn close_open_outage(s: &mut State, now: DateTime<Local>) {
    let Some(i) = s.open_outage.take() else {
        return;
    };
    let Some(event) = s.events.get_mut(i) else {
        return;
    };

    let secs = (now - event.started_at).num_seconds().max(0);
    event.ended_at = Some(now);
    event.duration_secs = Some(secs);
    event.text = format!("{}（持續 {}）", event.text, human_duration(secs));
}

/// 目前最靠近使用者的壞掉段落。
fn nearest_broken(s: &State) -> Option<SegmentKind> {
    let mut kinds: Vec<SegmentKind> = s.windows.keys().copied().collect();
    kinds.sort();
    kinds
        .into_iter()
        .find(|k| s.windows[k].summarize().grade >= Grade::Poor)
}

fn segment_label(s: &State, kind: SegmentKind) -> Option<String> {
    s.topology
        .as_ref()?
        .segments
        .iter()
        .find(|seg| seg.kind == kind)
        .map(|seg| seg.label.clone())
}

/// 顯示桌面通知。通知權限被關掉時安靜地失敗即可。
fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

/// 把秒數講成人話。
pub fn human_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m} 分鐘")
        } else {
            format!("{m} 分 {s} 秒")
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h} 小時")
        } else {
            format!("{h} 小時 {m} 分")
        }
    }
}

/// 組出要推給前端的完整狀態。
fn build_snapshot(s: &State) -> Snapshot {
    let topology = s.topology.clone();
    let segments_cfg = topology
        .as_ref()
        .map(|t| t.segments.clone())
        .unwrap_or_default();

    // 先算出每一段的統計，後面歸因和畫面都用同一份資料
    let summaries: Vec<(Segment, Summary)> = segments_cfg
        .iter()
        .map(|seg| {
            let summary = s
                .windows
                .get(&seg.kind)
                .map(|w| w.summarize())
                .unwrap_or_else(Summary::empty);
            (seg.clone(), summary)
        })
        .collect();

    let readings: Vec<Reading<'_>> = summaries
        .iter()
        .map(|(seg, summary)| Reading {
            kind: seg.kind,
            label: &seg.label,
            summary,
        })
        .collect();

    let verdict = verdict::diagnose(&readings, s.wifi.as_ref(), s.dns_health());

    let segment_views: Vec<SegmentView> = summaries
        .iter()
        .map(|(seg, summary)| SegmentView {
            kind: seg.kind,
            label: seg.label.clone(),
            description: seg.kind.description().to_string(),
            detail: seg.detail.clone(),
            target: seg.addr.to_string(),
            status_text: summary.grade.segment_text().to_string(),
            color: summary.grade.color().to_string(),
            last_ms: summary.last_ms,
            avg_ms: summary.avg_ms,
            max_ms: summary.max_ms,
            loss_pct: summary.loss_pct,
            jitter_ms: summary.jitter_ms,
            uptime_pct: s
                .windows
                .get(&seg.kind)
                .map(|w| w.lifetime_uptime_pct())
                .unwrap_or(100.0),
            spark: s
                .windows
                .get(&seg.kind)
                .map(|w| w.recent_rtts(CHART_POINTS))
                .unwrap_or_default(),
        })
        .collect();

    let elapsed_secs = s
        .started_at
        .map(|t| (Local::now() - t).num_seconds().max(0))
        .unwrap_or(0);

    let total_outage_secs: i64 = s
        .events
        .iter()
        .map(|e| {
            e.duration_secs
                .unwrap_or_else(|| (Local::now() - e.started_at).num_seconds().max(0))
        })
        .sum();

    // 穩定度以「沒有斷線的時間比例」表示，比丟包率好懂
    let stability_pct = if elapsed_secs <= 0 {
        100.0
    } else {
        ((elapsed_secs - total_outage_secs).max(0) as f64 / elapsed_secs as f64 * 100.0)
            .clamp(0.0, 100.0)
    };

    let dns_health = s.dns_health();
    let mut dns_view = DnsView {
        working: dns_health.working,
        avg_ms: dns_health.avg_ms,
        status_text: if !dns_health.working {
            "沒有回應".into()
        } else {
            match dns_health.avg_ms {
                Some(ms) if ms > 300.0 => "偏慢".into(),
                Some(_) => "正常".into(),
                None => "尚未測量".into(),
            }
        },
        color: if !dns_health.working {
            "poor".into()
        } else if dns_health.avg_ms.is_some_and(|ms| ms > 300.0) {
            "fair".into()
        } else {
            "good".into()
        },
    };

    if !segments_cfg
        .iter()
        .any(|s| s.kind == SegmentKind::International)
    {
        dns_view.status_text = "未啟用".into();
        // 沒有查詢過，就不能顯示成綠色：綠色的意思是「測過而且正常」
        dns_view.color = String::new();
    }

    let outage_text = if s.events.is_empty() {
        "沒有斷線過".to_string()
    } else {
        format!(
            "斷線 {} 次，總共 {}",
            s.events.len(),
            human_duration(total_outage_secs)
        )
    };

    // 主折線圖以國外那一段為準，最貼近「上網卡不卡」的體感
    let chart = segment_views
        .iter()
        .find(|v| v.kind == OUTAGE_SEGMENT)
        .map(|v| v.spark.clone())
        .or_else(|| segment_views.last().map(|v| v.spark.clone()))
        .unwrap_or_default();

    // 事件列表新的排前面
    let mut events = s.events.clone();
    events.reverse();

    Snapshot {
        running: s.running,
        elapsed_secs,
        elapsed_text: if elapsed_secs == 0 {
            "尚未開始".into()
        } else {
            format!("已檢測 {}", human_duration(elapsed_secs))
        },
        verdict,
        segments: segment_views,
        stability_pct,
        outage_count: s.events.len(),
        total_outage_secs,
        outage_text,
        events,
        wifi_text: s.wifi.as_ref().and_then(|w| w.plain_text()),
        wifi: s.wifi.clone(),
        dns: dns_view,
        chart,
        interface_name: topology
            .as_ref()
            .map(|t| t.interface_name.clone())
            .unwrap_or_default(),
        isp_name: topology.as_ref().and_then(|t| t.isp_name.clone()),
        notes: topology.map(|t| t.notes).unwrap_or_default(),
        log_path: s.log_path.as_ref().map(|p| p.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_written_in_plain_chinese() {
        assert_eq!(human_duration(0), "0 秒");
        assert_eq!(human_duration(47), "47 秒");
        assert_eq!(human_duration(60), "1 分鐘");
        assert_eq!(human_duration(125), "2 分 5 秒");
        assert_eq!(human_duration(3600), "1 小時");
        assert_eq!(human_duration(3900), "1 小時 5 分");
    }

    #[test]
    fn fresh_state_reports_not_running_and_full_stability() {
        let s = State::new();
        let snap = build_snapshot(&s);
        assert!(!snap.running);
        assert_eq!(snap.stability_pct, 100.0);
        assert_eq!(snap.outage_count, 0);
        assert_eq!(snap.outage_text, "沒有斷線過");
        assert_eq!(snap.elapsed_text, "尚未開始");
    }

    #[test]
    fn dns_card_is_neutral_when_dns_is_not_being_tested() {
        // 預設只測閘道、不做 DNS 查詢：卡片寫「未啟用」，而且不能是代表正常的綠色
        let snap = build_snapshot(&State::new());
        assert_eq!(snap.dns.status_text, "未啟用");
        assert_eq!(snap.dns.color, "");
    }

    #[test]
    fn dns_card_is_not_disabled_once_external_tests_are_on() {
        let mut s = State::new();
        s.topology = Some(Topology {
            segments: vec![Segment {
                kind: SegmentKind::International,
                addr: std::net::Ipv4Addr::new(192, 0, 2, 1),
                label: "國外網路".into(),
                detail: String::new(),
            }],
            interface_name: String::new(),
            is_wifi: false,
            local_ip: None,
            gateway: None,
            isp_name: None,
            notes: Vec::new(),
        });
        let snap = build_snapshot(&s);
        assert_ne!(snap.dns.status_text, "未啟用");
        assert_eq!(snap.dns.color, "good");
    }

    #[test]
    fn closing_an_outage_records_its_duration() {
        let mut s = State::new();
        let start = Local::now();
        s.events.push(OutageEvent {
            started_at: start,
            ended_at: None,
            duration_secs: None,
            culprit: Some(SegmentKind::Isp),
            text: "斷線了".into(),
            started_text: start.format("%H:%M:%S").to_string(),
            trace: None,
        });
        s.open_outage = Some(0);

        close_open_outage(&mut s, start + chrono::Duration::seconds(90));

        let e = &s.events[0];
        assert_eq!(e.duration_secs, Some(90));
        assert!(e.ended_at.is_some());
        assert!(e.text.contains("1 分 30 秒"), "{}", e.text);
        assert!(s.open_outage.is_none());
    }

    #[test]
    fn closing_without_an_open_outage_is_a_no_op() {
        let mut s = State::new();
        close_open_outage(&mut s, Local::now());
        assert!(s.events.is_empty());
    }

    #[test]
    fn dns_average_ignores_failed_probes() {
        let mut s = State::new();
        s.dns_samples.push_back(Some(20.0));
        s.dns_samples.push_back(None);
        s.dns_samples.push_back(Some(40.0));
        assert_eq!(s.dns_health().avg_ms, Some(30.0));
    }

    #[test]
    fn nearest_broken_prefers_the_closest_segment() {
        let mut s = State::new();
        // 家裡那段還好，電信商壞了，國外也壞了 —— 斷點是電信商
        let mut router = Window::new(SegmentKind::Router, 10);
        let mut isp = Window::new(SegmentKind::Isp, 10);
        let mut intl = Window::new(SegmentKind::International, 10);
        for _ in 0..5 {
            router.push(Sample {
                at: Local::now(),
                rtt_ms: Some(3.0),
            });
            isp.push(Sample {
                at: Local::now(),
                rtt_ms: None,
            });
            intl.push(Sample {
                at: Local::now(),
                rtt_ms: None,
            });
        }
        s.windows.insert(SegmentKind::Router, router);
        s.windows.insert(SegmentKind::Isp, isp);
        s.windows.insert(SegmentKind::International, intl);

        assert_eq!(nearest_broken(&s), Some(SegmentKind::Isp));
    }
}

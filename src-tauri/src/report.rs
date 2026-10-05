//! 產生供機關資訊人員判讀的去識別 HTML 報告。
//! 匯出前移除網路名稱、位址、路徑與可能間接包含識別資訊的文字。

use std::fmt::Write as _;

use chrono::Local;

use crate::monitor::{human_duration, Snapshot};

/// 把字串放進 HTML 之前先跳脫。
///
/// Wi-Fi 名稱、反查到的主機名稱都是外部來源，直接塞進 HTML 會壞掉。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 把毫秒數格式化成好讀的字串。
fn ms(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{v:.0} 毫秒"),
        None => "—".to_string(),
    }
}

/// 產生完整的 HTML 報告。
pub fn render(source: &Snapshot) -> String {
    let mut safe = source.clone();
    safe.interface_name = "（已遮蔽）".into();
    safe.isp_name = None;
    safe.wifi_text = safe
        .wifi
        .as_ref()
        .and_then(|w| w.signal_pct)
        .map(|p| format!("Wi-Fi 訊號 {p}%"));
    if let Some(w) = safe.wifi.as_mut() {
        w.ssid = None;
    }
    safe.log_path = None;
    safe.notes = vec![
        "本報告已省略網路名稱、IP、主機名稱與檔案路徑。測試未回應也可能是機關網路政策限制。".into(),
    ];
    for seg in &mut safe.segments {
        seg.label = seg.kind.default_label().into();
        seg.detail.clear();
        seg.target = "（已遮蔽）".into();
    }
    for event in &mut safe.events {
        event.trace = None;
        event.text = "測試目標未回應紀錄".into();
    }
    safe.verdict.explanation =
        "結果僅代表本次啟用的探測目標，請由機關資訊人員配合網路政策判讀。".into();
    if safe.verdict.suggestion.is_some() {
        safe.verdict.suggestion = Some("請交由機關資訊人員確認。".into());
    }
    let snap = &safe;
    let now = Local::now();
    let mut html = String::with_capacity(16 * 1024);

    let grade_class = snap.verdict.grade.color();

    html.push_str(
        r#"<!doctype html>
<html lang="zh-Hant">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>網路健檢報告</title>
<style>
  :root {
    --good: #16a34a; --fair: #d97706; --poor: #dc2626; --down: #991b1b;
    --ink: #1f2937; --muted: #6b7280; --line: #e5e7eb; --bg: #f9fafb;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; padding: 32px 20px;
    font-family: "Microsoft JhengHei", "PingFang TC", system-ui, sans-serif;
    color: var(--ink); background: var(--bg); line-height: 1.7;
  }
  .sheet { max-width: 820px; margin: 0 auto; background: #fff;
           border: 1px solid var(--line); border-radius: 14px; padding: 36px; }
  h1 { font-size: 26px; margin: 0 0 4px; }
  h2 { font-size: 18px; margin: 34px 0 12px; padding-bottom: 8px;
       border-bottom: 2px solid var(--line); }
  .meta { color: var(--muted); font-size: 14px; margin-bottom: 26px; }
  .verdict { border-radius: 12px; padding: 20px 22px; margin-bottom: 8px;
             border-left: 6px solid var(--muted); background: var(--bg); }
  .verdict.good { border-color: var(--good); }
  .verdict.fair { border-color: var(--fair); }
  .verdict.poor, .verdict.down { border-color: var(--poor); }
  .verdict h3 { margin: 0 0 8px; font-size: 21px; }
  .verdict p { margin: 0 0 6px; }
  .advice { margin-top: 12px; padding: 12px 14px; background: #fff;
            border: 1px dashed var(--line); border-radius: 8px; font-size: 15px; }
  .cards { display: grid; grid-template-columns: repeat(2, 1fr); gap: 12px; }
  .card { border: 1px solid var(--line); border-radius: 10px; padding: 14px 16px; }
  .card .name { font-weight: 700; margin-bottom: 2px; }
  .card .desc { color: var(--muted); font-size: 13px; margin-bottom: 10px; }
  .pill { display: inline-block; padding: 2px 10px; border-radius: 999px;
          font-size: 13px; font-weight: 700; color: #fff; }
  .pill.good { background: var(--good); } .pill.fair { background: var(--fair); }
  .pill.poor { background: var(--poor); } .pill.down { background: var(--down); }
  table { width: 100%; border-collapse: collapse; font-size: 14px; }
  th, td { text-align: left; padding: 9px 10px; border-bottom: 1px solid var(--line); }
  th { background: var(--bg); font-weight: 700; }
  td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .empty { color: var(--muted); font-style: italic; }
  .tech { font-size: 13px; color: var(--muted); }
  .tech table { font-family: Consolas, "Courier New", monospace; font-size: 12.5px; }
  footer { margin-top: 34px; padding-top: 14px; border-top: 1px solid var(--line);
           color: var(--muted); font-size: 12.5px; }
  @media print { body { background: #fff; padding: 0; }
                 .sheet { border: none; padding: 0; } }
</style>
</head>
<body>
<div class="sheet">
"#,
    );

    // --- 標題 ---
    let _ = write!(
        html,
        r#"<h1>網路健檢報告</h1>
<div class="meta">產生時間：{}　|　檢測時長：{}　|　使用的網路：{}</div>
"#,
        now.format("%Y 年 %-m 月 %-d 日 %H:%M"),
        esc(&snap.elapsed_text.replace("已檢測 ", "")),
        esc(&snap.interface_name),
    );

    // --- 結論 ---
    let _ = write!(
        html,
        r#"<div class="verdict {}">
  <h3>{}</h3>
  <p>{}</p>
"#,
        grade_class,
        esc(&snap.verdict.headline),
        esc(&snap.verdict.explanation),
    );
    if let Some(s) = &snap.verdict.suggestion {
        let _ = write!(html, r#"  <div class="advice">建議：{}</div>"#, esc(s));
    }
    html.push_str("</div>\n");

    // --- 摘要數字 ---
    let _ = write!(
        html,
        r#"<h2>整體狀況</h2>
<table>
  <tr><th>連線穩定度</th><td>{:.1}%</td></tr>
  <tr><th>斷線情形</th><td>{}</td></tr>
  <tr><th>查詢網址速度</th><td>{}（{}）</td></tr>
"#,
        snap.stability_pct,
        esc(&snap.outage_text),
        esc(&snap.dns.status_text),
        ms(snap.dns.avg_ms),
    );
    if let Some(w) = &snap.wifi_text {
        let _ = write!(html, "  <tr><th>無線訊號</th><td>{}</td></tr>\n", esc(w));
    }
    if let Some(isp) = &snap.isp_name {
        let _ = write!(html, "  <tr><th>網路業者</th><td>{}</td></tr>\n", esc(isp));
    }
    html.push_str("</table>\n");

    // --- 四段狀態 ---
    html.push_str("<h2>各段連線狀況</h2>\n<div class=\"cards\">\n");
    for seg in &snap.segments {
        let _ = write!(
            html,
            r#"  <div class="card">
    <div class="name">{}</div>
    <div class="desc">{}</div>
    <span class="pill {}">{}</span>
    <div style="margin-top:8px;font-size:13.5px">平均反應 {}</div>
  </div>
"#,
            esc(&seg.label),
            esc(&seg.description),
            seg.color,
            esc(&seg.status_text),
            ms(seg.avg_ms),
        );
    }
    html.push_str("</div>\n");

    // --- 附近的無線環境（使用者有勾選掃描才會有） ---
    if let Some(env) = &snap.wifi_env {
        html.push_str("<h2>附近的無線環境</h2>\n");
        if env.color.is_empty() {
            let _ = write!(html, "<p><strong>{}</strong></p>\n", esc(&env.status_text));
        } else {
            let _ = write!(
                html,
                "<p><span class=\"pill {}\">{}</span></p>\n",
                esc(&env.color),
                esc(&env.status_text),
            );
        }
        let _ = write!(html, "<p>{}</p>\n", esc(&env.detail));
        if let Some(note) = &env.note {
            let _ = write!(html, "<p>{}</p>\n", esc(note));
        }
        if let Some(suggestion) = &env.suggestion {
            let _ = write!(
                html,
                "<div class=\"advice\">建議：{}</div>\n",
                esc(suggestion)
            );
        }
        html.push_str(
            "<p class=\"tech\">僅含頻道與訊號源數量的統計，不含任何網路名稱或位址。掃描結果是系統某個時間點的快照；頻道重疊不代表一定造成不穩。</p>\n",
        );
    }

    // --- 斷線紀錄 ---
    html.push_str("<h2>斷線紀錄</h2>\n");
    if snap.events.is_empty() {
        html.push_str("<p class=\"empty\">整段檢測期間都沒有斷線。</p>\n");
    } else {
        html.push_str("<table>\n<tr><th>發生時間</th><th>持續多久</th><th>說明</th></tr>\n");
        for e in &snap.events {
            let _ = write!(
                html,
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
                esc(&e.started_text),
                match e.duration_secs {
                    Some(s) => esc(&human_duration(s)),
                    None => "仍在斷線中".to_string(),
                },
                esc(&e.text),
            );
        }
        html.push_str("</table>\n");
    }

    // --- 技術附錄 ---
    html.push_str("<h2>技術數據</h2>\n<p class=\"tech\">以下供機關資訊人員判讀。</p>\n");
    html.push_str(
        "<div class=\"tech\">\n<table>\n<tr><th>段落</th><th>探測目標</th><th>平均</th><th>最高</th><th>抖動</th><th>封包遺失</th></tr>\n",
    );
    for seg in &snap.segments {
        let _ = write!(
            html,
            "<tr><td>{}</td><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{:.1}%</td></tr>\n",
            esc(&seg.label),
            esc(&seg.target),
            ms(seg.avg_ms),
            ms(seg.max_ms),
            ms(seg.jitter_ms),
            seg.loss_pct,
        );
    }
    html.push_str("</table>\n");

    // 斷線當下的路徑追蹤，最有診斷價值的部分
    let traces: Vec<_> = snap
        .events
        .iter()
        .filter_map(|e| e.trace.as_ref().map(|t| (e, t)))
        .collect();
    if !traces.is_empty() {
        html.push_str("<h3 style=\"font-size:15px;margin-top:22px\">斷線當下的連線路徑</h3>\n");
        for (event, hops) in traces.iter().take(5) {
            let _ = write!(
                html,
                "<p style=\"margin:14px 0 6px\"><strong>{}</strong> 的路徑追蹤：</p>\n<table>\n<tr><th>#</th><th>位址</th><th>主機名稱</th><th>反應時間</th><th>回應</th></tr>\n",
                esc(&event.started_text)
            );
            for hop in hops.iter() {
                let _ = write!(
                    html,
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}/{}</td></tr>\n",
                    hop.ttl,
                    hop.addr.map(|a| a.to_string()).unwrap_or_else(|| "*".into()),
                    esc(hop.hostname.as_deref().unwrap_or("—")),
                    ms(hop.rtt_ms),
                    hop.received,
                    hop.sent,
                );
            }
            html.push_str("</table>\n");
        }
    }
    html.push_str("</div>\n");

    if !snap.notes.is_empty() {
        html.push_str("<h2>檢測備註</h2>\n<ul class=\"tech\">\n");
        for note in &snap.notes {
            let _ = write!(html, "<li>{}</li>\n", esc(note));
        }
        html.push_str("</ul>\n");
    }

    html.push_str(
        r#"<footer>本報告由「網路健檢」自動產生，已省略敏感網路識別資訊。</footer>
</div>
</body>
</html>
"#,
    );

    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_does_not_leak_identifiers_through_summary_or_notes() {
        let mut snap = crate::monitor::Monitor::new().snapshot();
        let secret = "SENSITIVE_NETWORK_77";
        snap.interface_name = secret.into();
        snap.isp_name = Some(secret.into());
        snap.wifi = Some(crate::wifi::WifiStatus {
            ssid: Some(secret.into()),
            signal_pct: Some(42),
            rx_mbps: Some(72),
        });
        snap.wifi_text = Some(format!("Wi-Fi「{secret}」"));
        snap.log_path = Some(format!("C:\\Users\\{secret}\\report.csv"));
        snap.notes.push(secret.into());
        snap.verdict.explanation = secret.into();
        snap.verdict.suggestion = Some(secret.into());
        snap.segments.push(crate::monitor::SegmentView {
            kind: crate::targets::SegmentKind::Router,
            label: secret.into(),
            description: "預設閘道".into(),
            detail: secret.into(),
            target: "10.23.45.67".into(),
            status_text: "正常".into(),
            color: "good".into(),
            last_ms: Some(3.0),
            avg_ms: Some(3.0),
            max_ms: Some(3.0),
            loss_pct: 0.0,
            jitter_ms: Some(0.0),
            uptime_pct: 100.0,
            spark: vec![Some(3.0)],
        });
        snap.events.push(crate::monitor::OutageEvent {
            started_at: Local::now(),
            ended_at: None,
            duration_secs: None,
            culprit: None,
            text: secret.into(),
            started_text: "12:00:00".into(),
            trace: Some(vec![crate::traceroute::Hop {
                ttl: 1,
                addr: Some("10.23.45.67".parse().unwrap()),
                rtt_ms: Some(3.0),
                hostname: Some(secret.into()),
                sent: 1,
                received: 1,
            }]),
        });
        let html = render(&snap);
        assert!(!html.contains(secret));
        assert!(!html.contains("10.23.45.67"));
        assert!(html.contains("42%"));
        assert!(html.contains("3 毫秒"));
        assert!(html.contains("Content-Security-Policy"));
        // Export must not destroy live diagnostic data.
        assert_eq!(snap.interface_name, secret);
        assert_eq!(
            snap.events[0].trace.as_ref().unwrap()[0]
                .hostname
                .as_deref(),
            Some(secret)
        );
    }

    #[test]
    fn report_includes_wifi_environment_only_when_it_was_enabled() {
        let mut snap = crate::monitor::Monitor::new().snapshot();
        assert!(
            !render(&snap).contains("附近的無線環境"),
            "沒啟用就不該出現"
        );

        snap.wifi_env = Some(crate::wifi_env::WifiEnvView {
            status_text: "擁擠".into(),
            color: "poor".into(),
            detail: "目前連在 2.4 GHz 的 6 號頻道。<script>".into(),
            note: Some("對照說明".into()),
            suggestion: Some("可請機關資訊人員評估".into()),
        });
        let html = render(&snap);
        assert!(html.contains("附近的無線環境"));
        assert!(html.contains("pill poor"));
        assert!(html.contains("6 號頻道"));
        assert!(html.contains("對照說明"));
        assert!(html.contains("資訊人員評估"));
        assert!(html.contains("不含任何網路名稱或位址"));
        // 文字一律跳脫，不能把標籤原樣帶進報告
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn escapes_html_special_characters() {
        assert_eq!(esc("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(esc("Tom & Jerry"), "Tom &amp; Jerry");
        assert_eq!(esc(r#""quoted""#), "&quot;quoted&quot;");
        // 中文不受影響
        assert_eq!(esc("中華電信"), "中華電信");
    }

    #[test]
    fn malicious_ssid_cannot_inject_script() {
        let evil = "<script>alert(1)</script>";
        let escaped = esc(evil);
        assert!(!escaped.contains("<script>"));
        assert!(escaped.contains("&lt;script&gt;"));
    }

    #[test]
    fn formats_missing_values_as_dash() {
        assert_eq!(ms(None), "—");
        assert_eq!(ms(Some(23.4)), "23 毫秒");
    }
}

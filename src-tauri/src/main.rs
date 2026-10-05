// 沒有這一行的話，程式啟動時會多跳一個黑色主控台視窗。
// 對不懂電腦的使用者來說那看起來就像中毒了。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod dns;
mod icmp;
mod monitor;
mod report;
mod stats;
mod storage;
mod targets;
mod traceroute;
mod verdict;
mod wifi;

use std::path::PathBuf;

use chrono::Local;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;

use monitor::{Monitor, Snapshot};

/// 報告和紀錄都放在「文件」底下的同一個資料夾，使用者找得到。
const OUTPUT_DIR_NAME: &str = "網路健檢報告";

/// 取得輸出資料夾，必要時建立它。
fn output_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .document_dir()
        // 有些系統把「文件」資料夾搬走或關掉，退回使用者家目錄
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("找不到可以存檔的位置：{e}"))?;

    let dir = base.join(OUTPUT_DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|e| format!("無法建立資料夾：{e}"))?;
    Ok(dir)
}

#[tauri::command]
async fn start_monitoring(
    app: AppHandle,
    monitor: State<'_, Monitor>,
    external: Option<bool>,
) -> Result<(), String> {
    let dir = output_dir(&app)?;
    monitor
        .start(app.clone(), dir, external.unwrap_or(false))
        .await
        .map_err(|e| format!("無法開始檢測：{e}"))
}

#[tauri::command]
fn stop_monitoring(monitor: State<'_, Monitor>) -> Snapshot {
    monitor.stop();
    monitor.snapshot()
}

#[tauri::command]
fn get_snapshot(monitor: State<'_, Monitor>) -> Snapshot {
    monitor.snapshot()
}

/// 產生 HTML 報告、存檔，然後直接用瀏覽器打開。
///
/// 刻意不跳存檔對話框 —— 要一個不懂電腦的人在檔案總管裡選資料夾，
/// 比直接告訴他「已經存到文件裡了」困難得多。
#[tauri::command]
fn save_report(app: AppHandle, monitor: State<'_, Monitor>) -> Result<String, String> {
    let snapshot = monitor.snapshot();
    let html = report::render(&snapshot);

    let dir = output_dir(&app)?;
    let name = format!("報告_{}.html", Local::now().format("%Y-%m-%d_%H%M%S"));
    let path = dir.join(&name);

    std::fs::write(&path, html).map_err(|e| format!("無法儲存報告：{e}"))?;

    // 存完直接開起來給使用者看，不要只留一句「已儲存」讓他自己去找
    let _ = app.opener().open_path(path.to_string_lossy(), None::<&str>);

    Ok(path.to_string_lossy().to_string())
}

/// 打開存放報告與紀錄的資料夾。
#[tauri::command]
fn open_output_folder(app: AppHandle) -> Result<(), String> {
    let dir = output_dir(&app)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("無法開啟資料夾：{e}"))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Monitor::new())
        .invoke_handler(tauri::generate_handler![
            start_monitoring,
            stop_monitoring,
            get_snapshot,
            save_report,
            open_output_folder,
        ])
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "顯示視窗", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "結束程式", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            let mut tray = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .tooltip("網路健檢")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => reveal_window(app),
                    "quit" => {
                        // 結束前先停下監測，把 CSV 的最後幾列寫進磁碟
                        if let Some(m) = app.try_state::<Monitor>() {
                            m.stop();
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
                    // 左鍵單擊直接叫出視窗，這是使用者最直覺的期待
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        reveal_window(tray.app_handle());
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let monitoring = app
                    .try_state::<Monitor>()
                    .map(|m| m.is_running())
                    .unwrap_or(false);

                // 檢測進行中就縮到系統匣，不要讓長時間監測被一個誤觸的 X 中斷。
                // 沒在檢測的話 X 就是關掉程式，符合一般人的預期。
                if monitoring {
                    api.prevent_close();
                    let _ = window.hide();
                    notify_still_running(app);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("無法啟動應用程式");
}

/// 把主視窗叫出來並取得焦點。
fn reveal_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 告訴使用者程式還在背景跑，不然他會以為關掉了。
fn notify_still_running(app: &AppHandle) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("網路健檢仍在背景檢測")
        .body("程式縮到工作列右下角了，點一下圖示就能叫回來。")
        .show();
}

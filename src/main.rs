extern crate hidapi;

use std::{process::Command, thread::sleep, time::Duration};

use active_win_pos_rs::get_active_window;
use chrono::{Datelike, Timelike, Utc};
use hidapi::{DeviceInfo, HidApi, HidDevice, HidError};

const KEY_BALL_VENDOR_ID: u16 = 0x5957;
const KEY_BALL_PRODUCT_ID: u16 = 0x0200;
const KEY_BALL_USAGE_ID: u16 = 0x61;

enum KeyballEvent {
    ApplicationName,
    DatetimeUpdate,
}

impl KeyballEvent {
    fn value(&self) -> u8 {
        match self {
            KeyballEvent::ApplicationName => 0x01,
            KeyballEvent::DatetimeUpdate => 0x02,
        }
    }
}

const REPORT_LENGTH: usize = 32;

// macOSでosascriptを使用してアクティブアプリ名を取得する関数
// より確実な方法として、NSWorkspace APIを経由する方法を使用
#[cfg(target_os = "macos")]
fn get_active_app_name_via_osascript() -> Option<String> {
    // より高速で確実な方法を試す
    let script = "tell application \"System Events\"\n\
                  set frontApp to first application process whose frontmost is true\n\
                  return name of frontApp\n\
                  end tell";
    
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();
    
    match output {
        Ok(output) => {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !name.is_empty() && name != "missing value" {
                    return Some(name);
                }
            }
        }
        Err(_) => {}
    }
    
    // フォールバック: 別の方法を試す
    let output2 = Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to get name of first application process whose frontmost is true")
        .output();
    
    match output2 {
        Ok(output) => {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !name.is_empty() && name != "missing value" {
                    return Some(name);
                }
            }
        }
        Err(_) => {}
    }
    
    None
}

// アプリ名を取得する関数（複数の方法を試す）
// macOSではosascriptを優先的に使用（より確実）
fn get_app_name_with_fallback() -> Option<String> {
    // macOSではosascriptを優先的に使用
    #[cfg(target_os = "macos")]
    {
        if let Some(name) = get_active_app_name_via_osascript() {
            let trimmed = name.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    
    // フォールバック: active-win-pos-rsを試す
    match get_active_window() {
        Ok(active_window) => {
            let name = active_window.app_name.trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
        Err(()) => {
            // エラーは無視
        }
    }
    
    None
}

fn check_device(info: &DeviceInfo) -> bool {
    info.vendor_id() == KEY_BALL_VENDOR_ID
        && info.product_id() == KEY_BALL_PRODUCT_ID
        && info.usage() == KEY_BALL_USAGE_ID
}

#[allow(dead_code)]
fn get_device_list() {
    match HidApi::new() {
        Ok(api) => {
            let mut devs: Vec<_> = api.device_list().collect();
            devs.sort_by_key(|d| d.product_id());
            devs.sort_by_key(|d| d.vendor_id());
            for device in devs {
                println!(
                    "PID:{:04X}_VID:{:04X}&UP:{:04X}_U:{:04X}",
                    device.vendor_id(),
                    device.product_id(),
                    device.usage_page(),
                    device.usage()
                );
                if let Ok(hid) = device.open_device(&api) {
                    if let Ok(man) = hid.get_manufacturer_string() {
                        println!("  manufacturer: {}", man.unwrap());
                    } else {
                        println!("  failed to get manufacturer");
                    }
                    if let Ok(prd) = hid.get_product_string() {
                        println!("  product name: {}", prd.unwrap());
                    } else {
                        println!("  failed to get product name");
                    }
                    // try `let...else...` statement
                    let Ok(sn) = hid.get_serial_number_string() else {
                        println!("  failed to get serial number");
                        continue;
                    };
                    println!("  serial number: {}", sn.unwrap());
                } else {
                    println!("  it cannot be opened");
                    continue;
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
        }
    }
}

// 現在時刻を取得して、フォーマットされたバイト列として返す関数
fn get_current_time_bytes(padding_byte: usize) -> Vec<u8> {
    let original = Utc::now(); // 現在のUTC時間を取得
    let now = original + chrono::Duration::hours(9);

    let time_string = format!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    ); // YYYY:MM:DD hh:mm:ss 形式の文字列

    let time_bytes = time_string.as_bytes();

    // 必要な長さだけ切り取る
    let mut time_data = vec![0x00; REPORT_LENGTH];
    let write_length = time_bytes.len().min(REPORT_LENGTH - padding_byte);
    time_data[padding_byte - 1] = KeyballEvent::DatetimeUpdate.value();
    time_data[padding_byte..write_length + padding_byte]
        .copy_from_slice(&time_bytes[..write_length]);

    time_data
}

// 再接続が必要なエラーかどうかを判定する関数
fn is_reconnection_needed(error: &HidError) -> bool {
    let error_str = format!("{:?}", error).to_lowercase();
    // 再接続が必要なエラーパターン
    error_str.contains("device not found") ||
    error_str.contains("no such device") ||
    error_str.contains("device disconnected") ||
    error_str.contains("broken pipe") ||
    error_str.contains("connection") ||
    error_str.contains("permission denied") ||
    error_str.contains("access denied") ||
    error_str.contains("io error")
}

// デバイスにバイト列を書き込む関数
fn write_to_device(hid: &HidDevice, data: &[u8]) -> Result<(), HidError> {
    match hid.write(data) {
        Ok(_sz) => {
            Ok(())
        }
        Err(e) => {
            eprintln!("Error writing to device: {:?}", e);
            Err(e)
        }
    }
}

fn start(hid: &HidDevice) -> Result<(), HidError> {
    let mut temp_app_name: Option<String> = None; // Option型に変更して、初期状態を明確にする
    let mut read_buf = vec![0x00; REPORT_LENGTH]; // 読み取り用バッファ
    let padding_byte = if cfg!(target_os = "windows") { 2 } else { 1 };

    // 非ブロッキングモードに設定
    match hid.set_blocking_mode(false) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("Error setting blocking mode: {:?}", e);
            if is_reconnection_needed(&e) {
                return Err(e);
            }
            // 再接続が不要なエラーの場合は続行
        }
    }

    // 接続直後に少し待ってから初期アプリ名を取得・送信する
    sleep(Duration::from_millis(200));
    if let Some(initial_app_name) = get_app_name_with_fallback() {
        let app_name_bytes = initial_app_name.as_bytes();
        let write_length = app_name_bytes.len().min(REPORT_LENGTH - padding_byte);
        let mut data = vec![0x00; REPORT_LENGTH];
        data[padding_byte - 1] = KeyballEvent::ApplicationName.value();
        data[padding_byte..write_length + padding_byte]
            .copy_from_slice(&app_name_bytes[..write_length]);
        
        match write_to_device(hid, &data) {
            Ok(_) => {
                temp_app_name = Some(initial_app_name);
                eprintln!("Initial app name sent: {}", temp_app_name.as_ref().unwrap());
            }
            Err(e) => {
                if is_reconnection_needed(&e) {
                    return Err(e);
                }
                eprintln!("Failed to send initial app name (non-critical): {:?}", e);
            }
        }
    }

    loop {
        // まずはキーボードからの通信を確認する
        match hid.read(&mut read_buf) {
            Ok(bytes_read) => {
                if bytes_read > 0 {
                    // 先頭バイトが時刻取得だった場合、現在時刻を返す
                    if read_buf[0] == KeyballEvent::DatetimeUpdate.value() {
                        let time_data = get_current_time_bytes(padding_byte);
                        write_to_device(hid, &time_data)?;
                        continue; // 時刻返答が完了したので、次のループへ
                    }
                }
            }
            Err(e) => {
                // 非ブロッキングモードでは、データがない場合のエラーは正常な動作
                // WouldBlockエラーは無視して続行
                let error_str = format!("{:?}", e).to_lowercase();
                if error_str.contains("wouldblock") || error_str.contains("would block") {
                    // 正常な動作なので続行
                } else {
                    // 再接続が必要なエラーの場合は、エラーを返して再接続を試みる
                    if is_reconnection_needed(&e) {
                        eprintln!("Device disconnected or error occurred, reconnecting...: {:?}", e);
                        return Err(e);
                    } else {
                        eprintln!("Error reading from device (non-critical): {:?}", e);
                        sleep(Duration::from_millis(100));
                        continue;
                    }
                }
            }
        }

        // アクティブウィンドウの情報を取得して処理する
        // macOSでは、システム設定 > セキュリティとプライバシー > プライバシー > アクセシビリティ
        // でこのアプリ（またはターミナル）に権限を付与する必要があります
        
        // アプリ名を取得（より頻繁にチェックするため、リトライは最小限に）
        let app_name = {
            let mut retries = 2; // リトライ回数を減らして、より頻繁にチェック
            let mut result = None;
            while retries > 0 && result.is_none() {
                result = get_app_name_with_fallback();
                if result.is_none() && retries > 1 {
                    sleep(Duration::from_millis(10)); // リトライ間隔を短く
                    retries -= 1;
                } else {
                    break;
                }
            }
            // デバッグ: アプリ名が取得できない場合のみログ出力（頻繁すぎるログを避ける）
            if result.is_none() && temp_app_name.is_none() {
                eprintln!("Warning: Failed to get app name");
            }
            result
        };

        let app_name = match app_name {
            Some(name) => name,
            None => {
                // アプリ名が取得できない場合
                if temp_app_name.is_some() {
                    // 前回のアプリ名がある場合は、それを維持して続行（短い間隔で再チェック）
                    sleep(Duration::from_millis(30));
                    continue;
                } else {
                    // 初回でアプリ名が取得できない場合は、少し待機
                    sleep(Duration::from_millis(100));
                    continue;
                }
            }
        };

        // アプリ名が変わっていない場合は短い間隔で再チェック
        if let Some(ref current_name) = temp_app_name {
            if current_name == &app_name {
                sleep(Duration::from_millis(30)); // チェック間隔を短く（30ms）
                continue;
            }
        }
        
        // アプリ名が変わった場合、または初回送信の場合
        temp_app_name = Some(app_name.clone());
        eprintln!("App name changed to: {}", app_name);

        let app_name_bytes = app_name.as_bytes();
        let write_length = app_name_bytes.len().min(REPORT_LENGTH - padding_byte);

        let mut data = vec![0x00; REPORT_LENGTH];
        data[padding_byte - 1] = KeyballEvent::ApplicationName.value();
        data[padding_byte..write_length + padding_byte]
            .copy_from_slice(&app_name_bytes[..write_length]);

        match write_to_device(hid, &data) {
            Ok(_) => {
                eprintln!("Successfully sent app name: {}", app_name);
            }
            Err(e) => {
                // 再接続が必要なエラーの場合は、エラーを返して再接続を試みる
                if is_reconnection_needed(&e) {
                    eprintln!("Device disconnected or error occurred during write, reconnecting...: {:?}", e);
                    return Err(e);
                } else {
                    eprintln!("Error writing to device (non-critical): {:?}", e);
                    sleep(Duration::from_millis(100));
                    continue;
                }
            }
        }

        // アプリ名を送信した後は短い間隔で次のチェックに進む
        sleep(Duration::from_millis(50));
    }
}

fn reconnect() -> Result<HidDevice, String> {
    loop {
        // デバイスリストを最新の状態で取得
        match HidApi::new() {
            Ok(api) => {
                for info in api.device_list() {
                    if check_device(info) {
                        match info.open_device(&api) {
                            Ok(hid) => return Ok(hid),
                            Err(_e) => {
                                // エラーは無視して次のデバイスを試す
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Error initializing HID API: {:?}", e);
                return Err(format!("Error initializing HID API: {:?}", e));
            }
        }
        sleep(Duration::from_secs(3)); // 再接続を3秒ごとに試みる
    }
}

// アクセシビリティ権限が付与されているかチェックする関数
#[cfg(target_os = "macos")]
fn check_accessibility_permission() -> bool {
    // アクセシビリティAPIを試行して権限をチェック
    match get_active_window() {
        Ok(_) => true,
        Err(()) => false,
    }
}

#[cfg(not(target_os = "macos"))]
fn check_accessibility_permission() -> bool {
    true
}

fn main() {
    // アクセシビリティ権限をチェック
    if !check_accessibility_permission() {
        eprintln!("アクセシビリティ権限が付与されていません。システム設定から権限を付与してください。");
        eprintln!("  システム設定 > プライバシーとセキュリティ > アクセシビリティ");
        eprintln!("  RawHID Rust を追加してチェックを入れてください");
        
        // 権限が付与されるまで待機（10秒ごとに再チェック）
        loop {
            sleep(Duration::from_secs(10));
            if check_accessibility_permission() {
                break;
            }
        }
    }
    
    match reconnect() {
        Ok(mut device) => {
            loop {
                match start(&device) {
                    Ok(_) => {
                        // 正常終了（通常は発生しない）
                        eprintln!("Device connection closed normally, reconnecting...");
                    }
                    Err(e) => {
                        eprintln!("Device error occurred: {:?}, attempting to reconnect...", e);
                        match reconnect() {
                            Ok(hid) => {
                                eprintln!("Reconnected successfully!");
                                device = hid;
                            }
                            Err(e) => {
                                eprintln!("Failed to reconnect: {}", e);
                                // 再接続に失敗しても、再接続を続ける
                                sleep(Duration::from_secs(1));
                            }
                        };
                    }
                }
            }
        },
        Err(e) => {
            eprintln!("Failed to connect to keyball: {}", e);
        }
    }
}

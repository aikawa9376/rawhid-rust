extern crate hidapi;

use std::{thread::sleep, time::Duration};

use active_win_pos_rs::get_active_window;
use chrono::{Datelike, Timelike, Utc};
use hidapi::{DeviceInfo, HidApi, HidDevice};

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
    let now = original + chrono::Duration::hours(7);

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

// デバイスにバイト列を書き込む関数
fn write_to_device(hid: &HidDevice, data: &[u8]) -> Result<(), String> {
    match hid.write(data) {
        Ok(sz) => {
            println!("Write ({} bytes): {:?}", sz, &data[..sz]);
            Ok(())
        }
        Err(e) => {
            eprintln!("Error writing to device: {:?}", e);
            Err(format!("Error writing to device: {:?}", e))
        }
    }
}

fn start(hid: &HidDevice) -> Result<(), String> {
    let mut temp_app_name: String = String::new();
    let mut read_buf = vec![0x00; REPORT_LENGTH]; // 読み取り用バッファ
    let padding_byte = if cfg!(target_os = "windows") { 2 } else { 1 };

    hid.set_blocking_mode(false).unwrap();

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
                eprintln!("Error reading from device: {:?}", e);
                return Err(format!("Error reading from device: {:?}", e));
            }
        }

        // アクティブウィンドウの情報を取得して処理する
        // let app_name = get_active_window().unwrap().app_name;
        let app_name = match get_active_window() {
            Ok(active_window) => active_window.app_name,
            Err(()) => {
                sleep(Duration::from_millis(700));
                continue;
            }
        };

        if temp_app_name == app_name {
            sleep(Duration::from_millis(700));
            continue;
        }
        temp_app_name = app_name.clone();

        let app_name_bytes = app_name.as_bytes();
        let write_length = app_name_bytes.len().min(REPORT_LENGTH - padding_byte);

        let mut data = vec![0x00; REPORT_LENGTH];
        data[padding_byte - 1] = KeyballEvent::ApplicationName.value();
        data[padding_byte..write_length + padding_byte]
            .copy_from_slice(&app_name_bytes[..write_length]);

        write_to_device(hid, &data)?; // 書き込み処理を関数化

        sleep(Duration::from_millis(700));
    }
}

fn reconnect() -> Result<HidDevice, String> {
    loop {
        // デバイスリストを最新の状態で取得
        match HidApi::new() {
            Ok(api) => {
                for info in api.device_list() {
                    if check_device(info) {
                        println!("Reconnecting to keyball...");
                        match info.open_device(&api) {
                            Ok(hid) => return Ok(hid),
                            Err(e) => {
                                println!("Error opening keyball: {:?}", e);
                            }
                        }
                    }
                }
                println!("Keyball not found. Retrying in 3 seconds...");
            }
            Err(e) => {
                eprintln!("Error initializing HID API: {:?}", e);
                return Err(format!("Error initializing HID API: {:?}", e));
            }
        }
        sleep(Duration::from_secs(3)); // 再接続を3秒ごとに試みる
    }
}

fn main() {
    match reconnect() {
        Ok(mut device) => loop {
            match start(&device) {
                Ok(_) => {}
                Err(_) => {
                    println!("Attempting to reconnect...");
                    match reconnect() {
                        Ok(hid) => device = hid,
                        Err(e) => {
                            eprintln!("Failed to reconnect: {}", e);
                            return;
                        }
                    };
                }
            }
        },
        Err(e) => {
            eprintln!("Failed to connect to keyball: {}", e);
        }
    }
}

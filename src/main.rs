extern crate hidapi;

use std::{sync::mpsc::Receiver, thread::sleep, time::Duration};

use chrono::{Datelike, Timelike, Utc};
use hidapi::{DeviceInfo, HidApi, HidDevice};

mod active_window;

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
const HID_READ_TIMEOUT: i32 = 50;

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

fn report_padding_byte() -> usize {
    if cfg!(target_os = "windows") {
        2
    } else {
        1
    }
}

fn build_report(event: KeyballEvent, payload: &[u8], padding_byte: usize) -> [u8; REPORT_LENGTH] {
    let mut report = [0x00; REPORT_LENGTH];
    let write_length = payload.len().min(REPORT_LENGTH - padding_byte);

    report[padding_byte - 1] = event.value();
    report[padding_byte..write_length + padding_byte].copy_from_slice(&payload[..write_length]);

    report
}

// 現在時刻を取得して、フォーマットされたバイト列として返す関数
fn get_current_time_bytes(padding_byte: usize) -> [u8; REPORT_LENGTH] {
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

    build_report(
        KeyballEvent::DatetimeUpdate,
        time_string.as_bytes(),
        padding_byte,
    )
}

// デバイスにバイト列を書き込む関数
fn write_to_device(hid: &HidDevice, data: &[u8]) -> Result<(), String> {
    match hid.write(data) {
        Ok(sz) => {
            let sz = sz.saturating_sub(if cfg!(target_os = "windows") { 1 } else { 0 });
            println!("Write ({} bytes): {:?}", sz, &data[..sz]);
            Ok(())
        }
        Err(e) => {
            eprintln!("Error writing to device: {:?}", e);
            Err(format!("Error writing to device: {:?}", e))
        }
    }
}

fn handle_keyboard_report(
    hid: &HidDevice,
    read_buf: &[u8],
    padding_byte: usize,
) -> Result<(), String> {
    // 先頭バイトが時刻取得だった場合、現在時刻を返す
    if read_buf.first().copied() == Some(KeyballEvent::DatetimeUpdate.value()) {
        let time_data = get_current_time_bytes(padding_byte);
        write_to_device(hid, &time_data)?;
    }

    Ok(())
}

fn update_active_application_name(
    hid: &HidDevice,
    app_name: String,
    temp_app_name: &mut String,
    padding_byte: usize,
) -> Result<(), String> {
    if *temp_app_name == app_name {
        return Ok(());
    }

    let data = build_report(
        KeyballEvent::ApplicationName,
        app_name.as_bytes(),
        padding_byte,
    );
    write_to_device(hid, &data)?;

    *temp_app_name = app_name;
    Ok(())
}

fn start(hid: &HidDevice) -> Result<(), String> {
    let mut temp_app_name: String = String::new();
    let mut read_buf = [0x00; REPORT_LENGTH]; // 読み取り用バッファ
    let padding_byte = report_padding_byte();
    let app_name_rx = active_window::spawn_app_name_watcher();

    loop {
        // キーボードからの通信は hidapi 側の poll/read_timeout に任せる
        match hid.read_timeout(&mut read_buf, HID_READ_TIMEOUT) {
            Ok(bytes_read) if bytes_read > 0 => {
                handle_keyboard_report(hid, &read_buf[..bytes_read], padding_byte)?;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error reading from device: {:?}", e);
                return Err(format!("Error reading from device: {:?}", e));
            }
        }

        drain_active_application_names(hid, &app_name_rx, &mut temp_app_name, padding_byte)?;
    }
}

fn drain_active_application_names(
    hid: &HidDevice,
    app_name_rx: &Receiver<String>,
    temp_app_name: &mut String,
    padding_byte: usize,
) -> Result<(), String> {
    while let Ok(app_name) = app_name_rx.try_recv() {
        update_active_application_name(hid, app_name, temp_app_name, padding_byte)?;
    }

    Ok(())
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
    if std::env::args().nth(1).as_deref() == Some("--watch-app-names") {
        for app_name in active_window::spawn_app_name_watcher() {
            println!("{app_name}");
        }
        return;
    }

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

extern crate hidapi;

use std::{thread::sleep, time::Duration};

use active_win_pos_rs::get_active_window;
use hidapi::{DeviceInfo, HidApi, HidDevice};

const KEY_BALL_VENDOR_ID: u16 = 0x5957;
const KEY_BALL_PRODUCT_ID: u16 = 0x0200;
const KEY_BALL_USAGE_ID: u16 = 0x61;

enum KeyballEvent {
    ApplicationName,
    // DatetimeUpdate,
}

impl KeyballEvent {
    fn value(&self) -> u8 {
        match self {
            KeyballEvent::ApplicationName => 0x01,
            // KeyballEvent::DatetimeUpdate => 0x02,
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

fn start(hid: &HidDevice) -> Result<(), String> {
    let mut temp_app_name: String = String::new();

    loop {
        let app_name = get_active_window().unwrap().app_name;

        if temp_app_name == app_name {
            sleep(Duration::from_millis(700));
            continue;
        }
        temp_app_name = app_name.clone();

        let app_name_bytes = app_name.as_bytes();
        let write_length = app_name_bytes.len().min(REPORT_LENGTH - 1);

        let mut data = vec![0x00; REPORT_LENGTH];
        data[0] = KeyballEvent::ApplicationName.value();
        data[1..write_length + 1].copy_from_slice(&app_name_bytes[..write_length]);

        match hid.write(&data) {
            Ok(sz) => {
                println!("Write ({} bytes): {:?}", sz, &data[..sz]);
            }
            Err(e) => {
                eprintln!("Error writing to device: {:?}", e);
                return Err(format!("Error writing to device: {:?}", e));
            }
        }

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

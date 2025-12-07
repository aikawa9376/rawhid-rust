use chrono::{Local, Datelike, Timelike};
use hidapi::{DeviceInfo, HidApi, HidDevice, HidError};
use std::{thread::sleep, time::{Duration, Instant}, os::raw::c_void};

// CoreGraphics imports
use core_graphics::display::{
    kCGWindowListOptionOnScreenOnly,
    kCGWindowListExcludeDesktopElements,
    CGWindowListCopyWindowInfo,
    kCGNullWindowID
};

// CoreFoundation types
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::array::CFArrayRef;
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::number::CFNumberRef;
use core_foundation::base::TCFType;

// macOS frameworks linking
#[link(name = "AppKit", kind = "framework")]
extern "C" {}
#[link(name = "Foundation", kind = "framework")]
extern "C" {}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(theArray: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(theArray: CFArrayRef, idx: isize) -> *const c_void;
    fn CFDictionaryGetValue(theDict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: CFNumberRef, theType: i64, valuePtr: *mut c_void) -> bool;
    fn CFRelease(cf: *const c_void);
}

// kCFNumberIntType = 9
const K_CFNUMBER_INT_TYPE: i64 = 9;

const KEY_BALL_VENDOR_ID: u16 = 0x5957;
const KEY_BALL_PRODUCT_ID: u16 = 0x0200;
const KEY_BALL_USAGE_ID: u16 = 0x61;

const REPORT_LENGTH: usize = 32;

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

// CFStringRef から String への変換ヘルパー
unsafe fn cfstring_to_string(cf_string: CFStringRef) -> String {
    let cf_str = CFString::wrap_under_get_rule(cf_string);
    cf_str.to_string()
}

fn get_frontmost_app_name() -> Option<String> {
    unsafe {
        // オプション: 画面上のウィンドウのみ、デスクトップ要素（壁紙など）を除外
        let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
        let window_list_info = CGWindowListCopyWindowInfo(options, kCGNullWindowID);

        if window_list_info.is_null() {
            return None;
        }

        let array_ref = window_list_info as CFArrayRef;
        let count = CFArrayGetCount(array_ref);

        let key_layer = CFString::new("kCGWindowLayer");
        let key_owner = CFString::new("kCGWindowOwnerName");

        let key_layer_ref = key_layer.as_concrete_TypeRef();
        let key_owner_ref = key_owner.as_concrete_TypeRef();

        for i in 0..count {
            let dict_ref = CFArrayGetValueAtIndex(array_ref, i) as CFDictionaryRef;

            // kCGWindowLayerを取得
            let layer_val_ptr = CFDictionaryGetValue(dict_ref, key_layer_ref as *const c_void);

            if !layer_val_ptr.is_null() {
                let layer_num_ref = layer_val_ptr as CFNumberRef;
                let mut layer: i32 = 0;
                let success = CFNumberGetValue(layer_num_ref, K_CFNUMBER_INT_TYPE, &mut layer as *mut i32 as *mut c_void);

                if success {
                    // レイヤー0が通常のアプリケーションウィンドウ
                    if layer == 0 {
                        // アプリ名 (kCGWindowOwnerName) を取得
                        let owner_val_ptr = CFDictionaryGetValue(dict_ref, key_owner_ref as *const c_void);

                        if !owner_val_ptr.is_null() {
                            let app_name = cfstring_to_string(owner_val_ptr as CFStringRef);
                            // DockやWindow Serverなどは除外
                            if app_name != "Dock" && app_name != "Window Server" && !app_name.is_empty() {
                                CFRelease(window_list_info as *const c_void);
                                return Some(app_name);
                            }
                        }
                    }
                }
            }
        }

        CFRelease(window_list_info as *const c_void);
        None
    }
}

fn check_device(info: &DeviceInfo) -> bool {
    info.vendor_id() == KEY_BALL_VENDOR_ID
        && info.product_id() == KEY_BALL_PRODUCT_ID
        && info.usage() == KEY_BALL_USAGE_ID
}

fn get_current_time_bytes() -> Vec<u8> {
    let now = Local::now();
    let time_string = format!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
        now.year(), now.month(), now.day(),
        now.hour(), now.minute(), now.second()
    );
    let time_bytes = time_string.as_bytes();
    let mut time_data = vec![0x00; REPORT_LENGTH];
    time_data[0] = KeyballEvent::DatetimeUpdate.value();
    let write_length = time_bytes.len().min(REPORT_LENGTH - 1);
    time_data[1..write_length + 1].copy_from_slice(&time_bytes[..write_length]);
    time_data
}

fn time_bytes_slice(bytes: &[u8], len: usize) -> &[u8] {
    if bytes.len() >= len {
        &bytes[..len]
    } else {
        bytes
    }
}

fn write_to_device(hid: &HidDevice, data: &[u8]) -> Result<(), HidError> {
    hid.write(data).map(|_| ())
}

fn start_loop(hid: &HidDevice) -> Result<(), HidError> {
    let mut current_app_name: Option<String> = None;
    let mut read_buf = vec![0x00; REPORT_LENGTH];
    let mut last_log_time = Instant::now();
    let mut last_check_time = Instant::now();

    // ノンブロッキングモードに設定
    hid.set_blocking_mode(false)?;

    eprintln!("Sending initial app name...");
    if let Some(app_name) = get_frontmost_app_name() {
        eprintln!("Initial app name: {}", app_name);
        send_app_name(hid, &app_name)?;
        current_app_name = Some(app_name);
    } else {
        eprintln!("Failed to get initial app name.");
        eprintln!("Note: If you are running on macOS 10.15+, please ensure this terminal has Screen Recording permission.");
        eprintln!("      System Settings > Privacy & Security > Screen Recording");
    }

    loop {
        // 1. デバイスからの読み込み
        match hid.read(&mut read_buf) {
            Ok(bytes_read) if bytes_read > 0 => {
                if read_buf[0] == KeyballEvent::DatetimeUpdate.value() {
                    let time_data = get_current_time_bytes();
                    if let Err(e) = write_to_device(hid, &time_data) {
                        eprintln!("Failed to send time data: {:?}", e);
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                 let error_str = format!("{:?}", e).to_lowercase();
                 if !error_str.contains("wouldblock") && !error_str.contains("temporarily unavailable") {
                     return Err(e);
                 }
            }
        }

        // 2. アプリ名の変更検知
        if last_check_time.elapsed() >= Duration::from_millis(100) {
            if let Some(new_app_name) = get_frontmost_app_name() {
                // デバッグログ
                if last_log_time.elapsed() >= Duration::from_secs(5) {
                    eprintln!("Heartbeat (CG): checking app (current detected: {})", new_app_name);
                    last_log_time = Instant::now();
                }

                let changed = match &current_app_name {
                    Some(old_name) => old_name != &new_app_name,
                    None => true,
                };

                if changed {
                    eprintln!("App name changed detected: {} -> {}",
                        current_app_name.as_deref().unwrap_or("None"),
                        new_app_name
                    );

                    if let Err(e) = send_app_name(hid, &new_app_name) {
                        eprintln!("Failed to send app name: {:?}", e);
                        return Err(e);
                    } else {
                        eprintln!("Sent app name packet successfully.");
                    }
                    current_app_name = Some(new_app_name);
                }
            }
            last_check_time = Instant::now();
        }

        sleep(Duration::from_millis(10));
    }
}

fn send_app_name(hid: &HidDevice, app_name: &str) -> Result<(), HidError> {
    let mut data = vec![0x00; REPORT_LENGTH];
    let app_name_bytes = app_name.as_bytes();

    data[0] = KeyballEvent::ApplicationName.value();

    let write_length = app_name_bytes.len().min(REPORT_LENGTH - 1);
    data[1..write_length + 1].copy_from_slice(&time_bytes_slice(app_name_bytes, write_length));

    write_to_device(hid, &data)
}

fn main() {
    eprintln!("Starting RawHID Rust for macOS (v0.1.4 - CoreGraphics)...");

    loop {
        let api = match HidApi::new() {
            Ok(api) => api,
            Err(e) => {
                eprintln!("Error initializing HID API: {}", e);
                sleep(Duration::from_secs(3));
                continue;
            }
        };

        let mut device_found = false;

        for info in api.device_list() {
            if check_device(info) {
                eprintln!("Found Keyball device: VID={:04x}, PID={:04x}", info.vendor_id(), info.product_id());
                match info.open_device(&api) {
                    Ok(device) => {
                        device_found = true;
                        eprintln!("Connected to Keyball.");

                        match start_loop(&device) {
                            Ok(_) => eprintln!("Disconnected normally."),
                            Err(e) => eprintln!("Device disconnected with error: {:?}", e),
                        }
                        break;
                    }
                    Err(e) => {
                        eprintln!("Failed to open device: {}", e);
                    }
                }
            }
        }

        if !device_found {
            sleep(Duration::from_secs(3));
        } else {
            sleep(Duration::from_secs(1));
        }
    }
}

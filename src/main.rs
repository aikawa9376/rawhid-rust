use std::{
    sync::mpsc::{Receiver, SyncSender},
    thread::sleep,
    time::{Duration, Instant},
};

use chrono::{Datelike, Timelike, Utc};
use hidapi::{DeviceInfo, HidApi, HidDevice};

mod active_window;
mod clipboard;
mod protocol;
use protocol::*;

const APP_REFRESH: Duration = Duration::from_secs(1);
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(3);
const HID_READ_TIMEOUT: i32 = 20;

fn firmware_app_name(app_name: &str) -> &str {
    match app_name {
        "com.github.hluk.copyq" => "copyq",
        _ => app_name,
    }
}

fn check_device(info: &DeviceInfo) -> bool {
    info.vendor_id() == 0x5957
        && info.product_id() == 0x0200
        && info.usage_page() == 0xff60
        && info.usage() == 0x61
}

#[derive(Default)]
struct AppSync {
    latest: Option<String>,
    sent: Option<String>,
    last_sent: Option<Instant>,
}

impl AppSync {
    fn drain(&mut self, receiver: &Receiver<String>) {
        // Only the latest focus matters; do not replay the disconnected PC's history.
        while let Ok(name) = receiver.try_recv() {
            self.latest = Some(firmware_app_name(&name).to_owned());
        }
    }

    fn connected(&mut self) {
        self.sent = None;
        self.last_sent = None;
    }

    fn pending(&self, now: Instant) -> Option<&str> {
        let name = self.latest.as_deref()?;
        if self.sent.as_deref() != Some(name)
            || self
                .last_sent
                .is_none_or(|sent| now.duration_since(sent) >= APP_REFRESH)
        {
            Some(name)
        } else {
            None
        }
    }

    fn mark_sent(&mut self, now: Instant) {
        self.sent.clone_from(&self.latest);
        self.last_sent = Some(now);
    }
}

fn write_to_device(hid: &HidDevice, report: &Report) -> Result<(), String> {
    let output = output_report(report);
    let written = hid
        .write(&output)
        .map_err(|e| format!("HID write failed: {e}"))?;
    if written != output.len() {
        return Err(format!("short HID write: {written}/{}", output.len()));
    }
    Ok(())
}

fn get_current_time_report() -> Report {
    let now = Utc::now() + chrono::Duration::hours(9);
    text_report(
        DATETIME_UPDATE,
        &format!(
            "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        ),
    )
}

#[derive(Default)]
struct ClipboardSession {
    transfer: TextTransfer,
    started: Option<Instant>,
    job: Option<(u8, bool)>,
}

impl ClipboardSession {
    fn accepts(&self, completion: &clipboard::Completion, generation: u64) -> bool {
        completion.generation == generation
            && self.job == Some((completion.token, completion.writing))
    }

    fn receive(&mut self, packet: &Report, generation: u64, jobs: &SyncSender<clipboard::Job>) {
        let token = packet[1];
        let result = match packet[0] {
            STORE_REQUEST => {
                self.transfer = TextTransfer::default();
                self.started = Some(Instant::now());
                self.job = Some((token, false));
                jobs.try_send(clipboard::Job {
                    generation,
                    token,
                    text: None,
                })
                .map_err(|_| "clipboard helper is busy".to_string())
            }
            PASTE_BEGIN | PASTE_DATA | PASTE_COMMIT => {
                if packet[0] == PASTE_BEGIN {
                    self.started = Some(Instant::now());
                    self.job = None;
                }
                match self.transfer.receive(packet) {
                    Ok(Some(text)) => {
                        self.job = Some((token, true));
                        jobs.try_send(clipboard::Job {
                            generation,
                            token,
                            text: Some(text),
                        })
                        .map_err(|_| "clipboard helper is busy".to_string())
                    }
                    Ok(None) => Ok(()),
                    Err(error) => Err(error),
                }
            }
            STORE_STATUS => {
                if self.job == Some((token, false)) {
                    eprintln!(
                        "Keyboard clipboard store: {}",
                        if packet[2] == 0 { "saved" } else { "rejected" }
                    );
                    *self = Self::default();
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            eprintln!("Clipboard: {error}");
            *self = Self::default();
        }
    }

    fn finish(
        &mut self,
        hid: &HidDevice,
        completion: clipboard::Completion,
        generation: u64,
    ) -> Result<(), String> {
        if !self.accepts(&completion, generation) {
            return Ok(());
        }
        let token = completion.token;
        match completion.result {
            Ok(_) if completion.writing => {
                write_to_device(hid, &report(PASTE_READY, &[token, 0]))?;
                *self = Self::default();
            }
            Ok(text) => match clipboard_text(&text) {
                Ok(text) => {
                    for packet in store_reports(token, &text) {
                        write_to_device(hid, &packet)?;
                    }
                    eprintln!(
                        "Keyboard clipboard: sent {} code points; awaiting confirmation",
                        text.chars().count()
                    );
                }
                Err(error) => {
                    eprintln!("Clipboard: {error}");
                    *self = Self::default();
                }
            },
            Err(error) => {
                eprintln!("Clipboard: {error}");
                if completion.writing {
                    write_to_device(hid, &report(PASTE_READY, &[token, 1]))?;
                }
                *self = Self::default();
            }
        }
        Ok(())
    }

    fn expire(&mut self) {
        if self
            .started
            .is_some_and(|started| started.elapsed() >= TRANSFER_TIMEOUT)
        {
            eprintln!("Clipboard transfer timed out; previous stored text is retained");
            *self = Self::default();
        }
    }
}

fn start(
    hid: &HidDevice,
    names: &Receiver<String>,
    apps: &mut AppSync,
    jobs: &SyncSender<clipboard::Job>,
    completions: &Receiver<clipboard::Completion>,
    generation: u64,
) -> Result<(), String> {
    let mut clipboard = ClipboardSession::default();
    let mut read_buf = [0; REPORT_LENGTH];
    apps.connected();
    loop {
        apps.drain(names);
        let now = Instant::now();
        if let Some(name) = apps.pending(now) {
            write_to_device(hid, &text_report(APPLICATION_NAME, name))?;
            if apps.sent.as_deref() != Some(name) {
                eprintln!("Active application: {name}");
            }
            apps.mark_sent(now);
        }
        clipboard.expire();
        while let Ok(completion) = completions.try_recv() {
            clipboard.finish(hid, completion, generation)?;
        }
        let size = hid
            .read_timeout(&mut read_buf, HID_READ_TIMEOUT)
            .map_err(|e| format!("HID read failed: {e}"))?;
        if size != REPORT_LENGTH {
            continue;
        }
        if read_buf[0] == DATETIME_UPDATE {
            write_to_device(hid, &get_current_time_report())?;
        } else {
            clipboard.receive(&read_buf, generation, jobs);
        }
    }
}

fn reconnect(names: &Receiver<String>, apps: &mut AppSync) -> HidDevice {
    let mut attempts = 0;
    loop {
        apps.drain(names);
        match HidApi::new() {
            Ok(api) => {
                for info in api.device_list().filter(|info| check_device(info)) {
                    match info.open_device(&api) {
                        Ok(hid) => {
                            eprintln!("Connected to Keyball");
                            return hid;
                        }
                        Err(error) if attempts % 10 == 0 => {
                            eprintln!("Cannot open Keyball: {error}")
                        }
                        Err(_) => {}
                    }
                }
            }
            Err(error) if attempts % 10 == 0 => eprintln!("Cannot initialize HID API: {error}"),
            Err(_) => {}
        }
        if attempts % 10 == 0 {
            eprintln!("Waiting for Keyball (retry every 500 ms)");
        }
        attempts += 1;
        sleep(RECONNECT_INTERVAL);
    }
}

#[cfg(target_os = "linux")]
fn single_instance() -> std::io::Result<std::os::unix::net::UnixListener> {
    use std::os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixListener},
    };
    UnixListener::bind_addr(&SocketAddr::from_abstract_name(
        b"rawhid-rust-keyball-5957-0200",
    )?)
}

fn main() {
    let names = active_window::spawn_app_name_watcher();
    if std::env::args().nth(1).as_deref() == Some("--watch-app-names") {
        for name in names {
            println!("{}", firmware_app_name(&name));
        }
        return;
    }
    #[cfg(target_os = "linux")]
    let _instance = match single_instance() {
        Ok(instance) => instance,
        Err(error) => {
            eprintln!("Cannot start Keyball daemon (another instance may be running): {error}");
            std::process::exit(1);
        }
    };
    let (jobs, completions) = clipboard::worker();
    let mut apps = AppSync::default();
    let mut generation = 0;
    loop {
        let device = reconnect(&names, &mut apps);
        generation += 1;
        if let Err(error) = start(&device, &names, &mut apps, &jobs, &completions, generation) {
            eprintln!("{error}; reconnecting");
        }
        drop(device);
        sleep(RECONNECT_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kvm_return_resends_unchanged_application_and_coalesces_focus_history() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut apps = AppSync::default();
        let now = Instant::now();
        tx.send("kitty".into()).unwrap();
        tx.send("com.github.hluk.copyq".into()).unwrap();
        apps.drain(&rx);
        assert_eq!(apps.pending(now), Some("copyq"));
        apps.mark_sent(now);
        assert_eq!(apps.pending(now + Duration::from_millis(900)), None);
        assert_eq!(apps.pending(now + APP_REFRESH), Some("copyq"));
        apps.connected();
        assert_eq!(apps.pending(now), Some("copyq"));
    }

    #[test]
    fn ignores_clipboard_completions_from_a_previous_usb_connection_or_request() {
        let session = ClipboardSession {
            job: Some((9, true)),
            ..Default::default()
        };
        let mut completion = clipboard::Completion {
            generation: 1,
            token: 9,
            writing: true,
            result: Ok(String::new()),
        };
        assert!(!session.accepts(&completion, 2));
        completion.generation = 2;
        assert!(session.accepts(&completion, 2));
        completion.token = 8;
        assert!(!session.accepts(&completion, 2));
        completion.token = 9;
        completion.writing = false;
        assert!(!session.accepts(&completion, 2));
    }

    #[test]
    fn failed_write_is_still_pending() {
        let apps = AppSync {
            latest: Some("Rofi".into()),
            ..Default::default()
        };
        assert_eq!(apps.pending(Instant::now()), Some("Rofi"));
        assert_eq!(apps.pending(Instant::now()), Some("Rofi"));
        assert_eq!(firmware_app_name("kitty"), "kitty");
    }
}

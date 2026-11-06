//! Clipboard commands run off the HID loop, with a deadline and without logging text.
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

pub struct Job {
    pub generation: u64,
    pub token: u8,
    pub text: Option<String>, // None: read; Some: write
}

pub struct Completion {
    pub generation: u64,
    pub token: u8,
    pub writing: bool,
    pub result: Result<String, String>,
}

pub fn worker() -> (SyncSender<Job>, Receiver<Completion>) {
    let (jobs, rx) = mpsc::sync_channel::<Job>(1);
    let (tx, completions) = mpsc::channel();
    thread::spawn(move || {
        for job in rx {
            let writing = job.text.is_some();
            let result = run(job.text.as_deref());
            if tx
                .send(Completion {
                    generation: job.generation,
                    token: job.token,
                    writing,
                    result,
                })
                .is_err()
            {
                break;
            }
        }
    });
    (jobs, completions)
}

fn run(text: Option<&str>) -> Result<String, String> {
    run_command(command(text.is_some())?, text)
}

fn run_command(mut command: Command, text: Option<&str>) -> Result<String, String> {
    command.stdin(if text.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(if text.is_some() {
        Stdio::null()
    } else {
        Stdio::piped()
    });
    command.stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start clipboard helper: {e}"))?;
    // Clipboard text is bounded to 400 bytes before a write is submitted.
    if let Some(text) = text {
        if let Err(error) = child.stdin.take().unwrap().write_all(text.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("clipboard helper stdin failed: {error}"));
        }
    }
    let reader = child.stdout.take().map(|stdout| {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(1_048_577)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send(result);
        });
        rx
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                thread::sleep(Duration::from_millis(10))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => format!("clipboard helper failed: {error}"),
                    _ => "clipboard helper timed out".into(),
                });
            }
        }
    };
    if !status.success() {
        return Err(
            "clipboard helper failed (check desktop session and clipboard permissions)".into(),
        );
    }
    let bytes = match reader {
        Some(reader) => reader
            .recv_timeout(Duration::from_secs(2).saturating_sub(started.elapsed()))
            .map_err(|_| "clipboard helper output timed out")?
            .map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    if bytes.len() > 1_048_576 {
        return Err("clipboard exceeds 1 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "clipboard is not UTF-8 text".into())
}

#[cfg(target_os = "linux")]
fn command(writing: bool) -> Result<Command, String> {
    use std::{fs, os::unix::fs::FileTypeExt, path::PathBuf};
    let sudo_uid = std::env::var("SUDO_UID").ok().filter(|uid| uid != "0");
    let runtime = sudo_uid
        .as_ref()
        .map(|uid| PathBuf::from("/run/user").join(uid))
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from));
    let wayland = std::env::var_os("WAYLAND_DISPLAY").or_else(|| {
        fs::read_dir(runtime.as_ref()?)
            .ok()?
            .filter_map(Result::ok)
            .find(|entry| {
                entry.file_name().to_string_lossy().starts_with("wayland-")
                    && entry.file_type().is_ok_and(|ty| ty.is_socket())
            })
            .map(|entry| entry.file_name())
    });
    let executable = match (wayland.is_some(), writing) {
        (true, true) => "wl-copy",
        (true, false) => "wl-paste",
        (false, _) => "xclip",
    };
    let mut environment = Vec::new();
    if let Some(runtime) = &runtime {
        environment.push(("XDG_RUNTIME_DIR", runtime.as_os_str().to_owned()));
    }
    if let Some(display) = &wayland {
        environment.push(("WAYLAND_DISPLAY", display.clone()));
    }
    for name in ["DISPLAY", "XAUTHORITY"] {
        if let Some(value) = std::env::var_os(name) {
            environment.push((name, value));
        }
    }
    let mut command = if let Some(uid) = sudo_uid {
        uid.parse::<u32>().map_err(|_| "invalid SUDO_UID")?;
        let mut command = Command::new("sudo");
        command.args(["-n", "-u", &format!("#{uid}"), "--", "env"]);
        for (name, value) in &environment {
            let mut argument = std::ffi::OsString::from(format!("{name}="));
            argument.push(value);
            command.arg(argument);
        }
        command.arg(executable);
        command
    } else {
        Command::new(executable)
    };
    for (name, value) in environment {
        command.env(name, value);
    }
    if wayland.is_some() {
        if writing {
            command.args(["--type", "text/plain;charset=utf-8"]);
        } else {
            command.args(["--no-newline", "--type", "text"]);
        }
    } else {
        command.args(["-selection", "clipboard", "-target", "UTF8_STRING"]);
        if !writing {
            command.arg("-out");
        }
    }
    Ok(command)
}

#[cfg(target_os = "macos")]
fn command(writing: bool) -> Result<Command, String> {
    let mut command = Command::new(if writing { "pbcopy" } else { "pbpaste" });
    command.env("LC_CTYPE", "en_US.UTF-8");
    Ok(command)
}

#[cfg(target_os = "windows")]
fn command(writing: bool) -> Result<Command, String> {
    let mut command = Command::new("powershell.exe");
    let action = if writing {
        "Set-Clipboard -Value ([Console]::In.ReadToEnd())"
    } else {
        "[Console]::Write([string](Get-Clipboard -Raw))"
    };
    command.args(["-NoProfile", "-NonInteractive", "-Command", &format!(
        "[Console]::InputEncoding=[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); {action}"
    )]);
    Ok(command)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn command(_writing: bool) -> Result<Command, String> {
    Err("clipboard backend unavailable on this OS".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn helper_preserves_utf8_and_trailing_newlines() {
        let mut command = Command::new("printf");
        command.arg("日本語😀\n");
        assert_eq!(run_command(command, None).unwrap(), "日本語😀\n");
    }

    #[test]
    fn helper_failure_does_not_return_success_or_log_clipboard_content() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf secret >&2; exit 1"]);
        let error = run_command(command, None).unwrap_err();
        assert!(!error.contains("secret"));
    }

    #[test]
    fn helper_deadline_includes_output_pipes_inherited_by_a_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 3 &"]);
        let started = Instant::now();
        assert!(run_command(command, None)
            .unwrap_err()
            .contains("timed out"));
        assert!(started.elapsed() < Duration::from_millis(2800));
    }

    #[test]
    fn helper_deadline_prevents_an_unresponsive_clipboard_from_stalling_requests() {
        let mut command = Command::new("sleep");
        command.arg("10");
        let started = Instant::now();
        assert!(run_command(command, None)
            .unwrap_err()
            .contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}

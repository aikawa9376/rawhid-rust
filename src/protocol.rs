//! Fixed 32-byte QMK reports; hidapi's leading report ID is added only at write time.
pub const REPORT_LENGTH: usize = 32;
pub const MAX_CHARS: usize = 100;
pub const MAX_BYTES: usize = MAX_CHARS * 4;
pub const CHUNK_BYTES: usize = REPORT_LENGTH - 5;
pub const APPLICATION_NAME: u8 = 0x01;
pub const DATETIME_UPDATE: u8 = 0x02;
pub const STORE_REQUEST: u8 = 0x10;
pub const STORE_BEGIN: u8 = 0x11;
pub const STORE_DATA: u8 = 0x12;
pub const STORE_COMMIT: u8 = 0x13;
pub const STORE_STATUS: u8 = 0x14;
pub const PASTE_BEGIN: u8 = 0x15;
pub const PASTE_DATA: u8 = 0x16;
pub const PASTE_COMMIT: u8 = 0x17;
pub const PASTE_READY: u8 = 0x18;
pub type Report = [u8; REPORT_LENGTH];

pub fn report(event: u8, payload: &[u8]) -> Report {
    let mut report = [0; REPORT_LENGTH];
    report[0] = event;
    let length = payload.len().min(REPORT_LENGTH - 1);
    report[1..1 + length].copy_from_slice(&payload[..length]);
    report
}

pub fn text_report(event: u8, text: &str) -> Report {
    let mut end = text.len().min(REPORT_LENGTH - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    report(event, &text.as_bytes()[..end])
}

pub fn output_report(report: &Report) -> [u8; REPORT_LENGTH + 1] {
    let mut output = [0; REPORT_LENGTH + 1];
    output[1..].copy_from_slice(report);
    output
}

pub fn clipboard_text(text: &str) -> Result<String, String> {
    if text.contains('\0') {
        return Err("clipboard text contains NUL".into());
    }
    Ok(text.chars().take(MAX_CHARS).collect())
}

pub fn store_reports(token: u8, text: &str) -> Vec<Report> {
    let length = text.len() as u16;
    let mut packets = vec![report(
        STORE_BEGIN,
        &[token, length as u8, (length >> 8) as u8],
    )];
    for (index, chunk) in text.as_bytes().chunks(CHUNK_BYTES).enumerate() {
        let offset = (index * CHUNK_BYTES) as u16;
        let mut packet = report(
            STORE_DATA,
            &[token, offset as u8, (offset >> 8) as u8, chunk.len() as u8],
        );
        packet[5..5 + chunk.len()].copy_from_slice(chunk);
        packets.push(packet);
    }
    packets.push(report(STORE_COMMIT, &[token]));
    packets
}

#[derive(Default)]
pub struct TextTransfer {
    token: u8,
    total: Option<usize>,
    bytes: Vec<u8>,
}

impl TextTransfer {
    pub fn receive(&mut self, packet: &Report) -> Result<Option<String>, String> {
        match packet[0] {
            PASTE_BEGIN => {
                *self = Self::default();
                self.token = packet[1];
                let length = u16::from_le_bytes([packet[2], packet[3]]) as usize;
                if length > MAX_BYTES {
                    return Err("clipboard transfer too large".into());
                }
                self.total = Some(length);
            }
            PASTE_DATA | PASTE_COMMIT => {
                let total = self.total.ok_or("clipboard transfer has no begin")?;
                if packet[1] != self.token {
                    return Err("clipboard transfer token mismatch".into());
                }
                if packet[0] == PASTE_DATA {
                    let offset = u16::from_le_bytes([packet[2], packet[3]]) as usize;
                    let count = packet[4] as usize;
                    if count == 0
                        || count > CHUNK_BYTES
                        || offset != self.bytes.len()
                        || offset + count > total
                    {
                        self.total = None;
                        return Err("clipboard transfer invalid offset or length".into());
                    }
                    self.bytes.extend_from_slice(&packet[5..5 + count]);
                } else {
                    self.total = None;
                    if self.bytes.len() != total {
                        return Err("incomplete clipboard transfer".into());
                    }
                    let text = String::from_utf8(std::mem::take(&mut self.bytes))
                        .map_err(|_| "clipboard transfer invalid UTF-8")?;
                    if text.contains('\0') || text.chars().count() > MAX_CHARS {
                        return Err("clipboard transfer invalid text".into());
                    }
                    return Ok(Some(text));
                }
            }
            _ => return Err("unexpected clipboard packet".into()),
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) -> Result<Option<String>, String> {
        let mut rx = TextTransfer::default();
        let mut result = None;
        for mut packet in store_reports(7, text) {
            packet[0] = match packet[0] {
                STORE_BEGIN => PASTE_BEGIN,
                STORE_DATA => PASTE_DATA,
                _ => PASTE_COMMIT,
            };
            result = rx.receive(&packet)?;
        }
        Ok(result)
    }

    #[test]
    fn round_trips_utf8_across_chunk_boundaries_and_empty_text() {
        for text in ["", "日本語\nwith emoji😀", &"😀".repeat(100)] {
            assert_eq!(round_trip(text).unwrap().as_deref(), Some(text));
        }
    }

    #[test]
    fn limits_code_points_without_cutting_utf8() {
        assert_eq!(clipboard_text(&"😀".repeat(101)).unwrap(), "😀".repeat(100));
        assert!(clipboard_text("a\0b").is_err());
        assert!(round_trip(&"a".repeat(101)).is_err());
    }

    #[test]
    fn rejects_missing_reordered_and_mismatched_chunks() {
        let mut rx = TextTransfer::default();
        assert!(rx.receive(&report(PASTE_COMMIT, &[1])).is_err());
        rx.receive(&report(PASTE_BEGIN, &[1, 2, 0])).unwrap();
        assert!(rx
            .receive(&report(PASTE_DATA, &[2, 0, 0, 1, b'a']))
            .is_err());
        assert!(rx
            .receive(&report(PASTE_DATA, &[1, 1, 0, 1, b'a']))
            .is_err());
        rx.receive(&report(PASTE_BEGIN, &[1, 2, 0])).unwrap();
        assert!(rx.receive(&report(PASTE_COMMIT, &[1])).is_err());
    }

    #[test]
    fn rejects_invalid_utf8_and_oversize_transfers() {
        let mut rx = TextTransfer::default();
        assert!(rx.receive(&report(PASTE_BEGIN, &[1, 145, 1])).is_err());
        rx.receive(&report(PASTE_BEGIN, &[1, 1, 0])).unwrap();
        rx.receive(&report(PASTE_DATA, &[1, 0, 0, 1, 0xff]))
            .unwrap();
        assert!(rx.receive(&report(PASTE_COMMIT, &[1])).is_err());
    }

    #[test]
    fn uses_one_zero_report_id_and_all_32_payload_bytes_on_every_os() {
        let report = report(APPLICATION_NAME, &[b'a'; 31]);
        let output = output_report(&report);
        assert_eq!(output.len(), 33);
        assert_eq!(&output[..2], &[0, APPLICATION_NAME]);
        assert_eq!(&output[2..], &[b'a'; 31]);
        assert_eq!(text_report(APPLICATION_NAME, &"あ".repeat(11))[31], 0);
    }
}

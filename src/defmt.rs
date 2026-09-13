//! defmt support: decode raw defmt frames captured from the QEMU semihosting
//! console when a test fails.
//!
//! Firmware that logs with defmt through a semihosting transport (e.g. the
//! `defmt-semihosting` global logger) writes *encoded frames* to the console,
//! which QEMU forwards to our stdout/stderr. [`DefmtInfo`] parses the `.defmt`
//! section of the test ELF with `defmt-decoder` and renders those frames as
//! human-readable log lines, so a failing test prints real log output instead
//! of binary garbage. Bytes that are not defmt frames (plain `semihosting::println!`
//! output, a frame truncated by a reset) pass through as lossy text.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use defmt_decoder::{DecodeError, Frame, Locations, Table};

use crate::cli::Cli;

/// defmt metadata extracted from a test ELF, ready to decode guest output.
///
/// `Table` is behind `Arc` because it is not `Clone` in newer defmt-decoder
/// releases (it became an interner-owning type); sharing keeps `DefmtInfo`
/// cheaply cloneable for `Cli`.
#[derive(Clone)]
pub struct DefmtInfo {
    table: std::sync::Arc<Table>,
    /// DWARF locations of the log statements, when the ELF carries debug info.
    locations: Option<Locations>,
}

impl DefmtInfo {
    /// Parse the `.defmt` section (and, best-effort, DWARF locations) of the
    /// given test ELF. Returns `Ok(None)` when the firmware does not use defmt.
    pub fn from_elf(path: &Path) -> Result<Option<Self>> {
        let elf = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        Self::from_elf_bytes(&elf)
            .with_context(|| format!("failed to parse .defmt section of {}", path.display()))
    }

    /// Same as [`Self::from_elf`] but takes the ELF image as bytes.
    pub fn from_elf_bytes(elf: &[u8]) -> Result<Option<Self>> {
        let Some(table) = Table::parse(elf)? else {
            return Ok(None);
        };
        let locations = table.get_locations(elf).ok().filter(|l| !l.is_empty());
        Ok(Some(Self {
            table: std::sync::Arc::new(table),
            locations,
        }))
    }

    /// Decode captured guest output into printable lines.
    ///
    /// The stream may mix defmt frames with plain text (e.g. embedded-test's
    /// panic report) and can end in a truncated frame when the guest reset.
    /// Decoding resynchronizes after malformed bytes by skipping one byte at a
    /// time; skipped bytes are flushed as lossy UTF-8 text so nothing the
    /// firmware printed is lost.
    pub fn decode_output(&self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut rest = bytes;
        let mut pos = 0usize;
        // Start of the current run of non-frame bytes, pending flush.
        let mut garbage_start: Option<usize> = None;

        while !rest.is_empty() {
            match self.table.decode(rest) {
                Ok((frame, consumed)) => {
                    flush_garbage(&mut lines, bytes, garbage_start.take(), pos);
                    lines.push(self.render_frame(&frame));
                    let advance = consumed.max(1);
                    rest = &rest[advance..];
                    pos += advance;
                }
                Err(DecodeError::UnexpectedEof) => {
                    // Frame cut short by a reset/crash: show the rest as text.
                    flush_garbage(&mut lines, bytes, garbage_start.take(), bytes.len());
                    break;
                }
                Err(DecodeError::Malformed) => {
                    // Not a frame at this offset: skip one byte and keep looking.
                    if garbage_start.is_none() {
                        garbage_start = Some(pos);
                    }
                    rest = &rest[1..];
                    pos += 1;
                }
            }
        }
        flush_garbage(&mut lines, bytes, garbage_start, pos);
        lines
    }

    fn render_frame(&self, frame: &Frame) -> String {
        let mut line = frame.display(false).to_string();
        if let Some(location) = self
            .locations
            .as_ref()
            .and_then(|locs| locs.get(&frame.index()))
        {
            line.push_str(&format!(
                "\n  └─ {}:{}",
                location.file.display(),
                location.line
            ));
        }
        line
    }
}

/// Decide how to handle defmt output for one test ELF: forced on/off via the
/// CLI flags, otherwise auto-detected from the presence of a `.defmt` section.
pub fn resolve(args: &Cli, elf_path: &Path) -> Result<Option<DefmtInfo>> {
    if args.no_defmt {
        return Ok(None);
    }
    let info = DefmtInfo::from_elf(elf_path)?;
    if args.defmt && info.is_none() {
        eprintln!(
            "qtest: warning: --defmt given but `{}` has no .defmt section; showing raw output",
            elf_path.display()
        );
    }
    Ok(info)
}

fn flush_garbage(lines: &mut Vec<String>, bytes: &[u8], start: Option<usize>, end: usize) {
    if let Some(start) = start {
        if end > start {
            let text = String::from_utf8_lossy(&bytes[start..end]);
            lines.extend(
                text.lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(str::to_owned),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static CAPTURED: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();

    fn captured() -> &'static Mutex<Vec<u8>> {
        CAPTURED.get_or_init(|| Mutex::new(Vec::new()))
    }

    #[defmt::global_logger]
    struct CaptureLogger;

    static ENCODER: std::sync::Mutex<defmt::Encoder> = std::sync::Mutex::new(defmt::Encoder::new());

    fn sink(bytes: &[u8]) {
        captured().lock().unwrap().extend_from_slice(bytes);
    }

    defmt::timestamp!("{=u64}", 0);

    // Safety: this is a host-side test double; frames are encoded with the
    // real `defmt::Encoder` (raw encoding) and all sink writes go to a
    // mutex-guarded buffer, so the acquire/write/release ordering the trait
    // requires is upheld.
    unsafe impl defmt::Logger for CaptureLogger {
        fn acquire() {
            ENCODER.lock().unwrap().start_frame(sink);
        }

        unsafe fn release() {
            ENCODER.lock().unwrap().end_frame(sink);
        }

        unsafe fn write(bytes: &[u8]) {
            ENCODER.lock().unwrap().write(bytes, sink);
        }

        unsafe fn flush() {}
    }

    /// Host-side roundtrip helper: parse the `.defmt` table of the running
    /// test binary. Returns `None` (skipping the test) when the host linker
    /// garbage-collected the `.defmt` section -- interned strings are
    /// unreferenced on host targets unless `--no-gc-sections` is forced, which
    /// breaks other link jobs. The QEMU end-to-end tests cover decoding on
    /// real firmware ELFs either way.
    fn own_table() -> Option<DefmtInfo> {
        match DefmtInfo::from_elf(&std::env::current_exe().unwrap()) {
            Ok(Some(info)) => Some(info),
            Ok(None) => {
                eprintln!("skipping: test binary has no .defmt section");
                None
            }
            Err(e) => {
                eprintln!("skipping: cannot parse own .defmt section: {e:#}");
                None
            }
        }
    }

    #[test]
    fn decodes_frames_and_passes_text_through() {
        let Some(info) = own_table() else {
            return;
        };
        let buf = captured();
        buf.lock().unwrap().clear();

        defmt::info!("qtest canary {=u8}", 7);
        defmt::warn!("qtest second {=u16}", 300);
        // Plain semihosting text interleaved with the frames, then another frame.
        buf.lock().unwrap().extend_from_slice(b"plain panic line\n");
        defmt::error!("qtest third {=bool}", true);

        let bytes = buf.lock().unwrap().clone();
        let lines = info.decode_output(&bytes);
        let joined = lines.join("\n");

        assert!(
            joined.contains("qtest canary 7"),
            "decoded output:\n{joined}"
        );
        assert!(
            joined.contains("qtest second 300"),
            "decoded output:\n{joined}"
        );
        assert!(
            joined.contains("plain panic line"),
            "decoded output:\n{joined}"
        );
        assert!(
            joined.contains("qtest third true"),
            "decoded output:\n{joined}"
        );
        // The frames must appear in order around the text line.
        let i1 = joined.find("qtest canary 7").unwrap();
        let i2 = joined.find("qtest second 300").unwrap();
        let i3 = joined.find("plain panic line").unwrap();
        let i4 = joined.find("qtest third true").unwrap();
        assert!(i1 < i2 && i2 < i3 && i3 < i4, "decoded output:\n{joined}");
    }

    #[test]
    fn undecodable_bytes_become_text() {
        let Some(info) = own_table() else {
            return;
        };
        let lines = info.decode_output(b"not defmt at all\n");
        assert_eq!(lines, vec!["not defmt at all".to_string()]);
    }
}

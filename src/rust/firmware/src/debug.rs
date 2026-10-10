//! Firmware diagnostics.
//!
//! A BIOS that does not boot says nothing, which makes a silent failure
//! the hardest kind to debug: the host can see the machine halted but not
//! how far it got. Two channels are provided here, and both are cheap
//! enough to leave enabled.
//!
//! * A **trace log**: services append a tag and a line to a ring buffer
//!   owned by the firmware, which the host can read at any time. This is
//!   the "what did the firmware do" channel.
//! * A **POST banner**: services can write to the screen through the
//!   video BIOS, which is where a user looks first. This is the "what
//!   should the user see" channel.

/// Tags, so the log says which subsystem spoke.
pub mod tag {
    pub const POST: u8 = b'P';
    pub const VIDEO: u8 = b'V';
    pub const DISK: u8 = b'D';
    pub const SYSTEM: u8 = b'S';
    pub const KEYBOARD: u8 = b'K';
    pub const TRAP: u8 = b'T';
    pub const BOOT: u8 = b'B';
}

const CAPACITY: usize = 4096;
const MAX_LINE: usize = 96;

/// The trace ring. Kept outside the firmware state so a test can inspect
/// it without constructing a machine.
pub struct Trace {
    buf: Vec<u8>,
    /// Number of entries dropped because the ring wrapped.
    pub dropped: u32,
    enabled: bool,
}

impl Trace {
    pub const fn new() -> Trace {
        Trace {
            buf: Vec::new(),
            dropped: 0,
            enabled: false,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Append one entry. Wraps rather than growing without bound: a guest
    /// that calls INT 10h in a loop must not exhaust memory.
    pub fn record(&mut self, tag: u8, line: &str) {
        if !self.enabled {
            return;
        }

        let start = self.buf.len();

        // 'T' tag, 8-hex-digit sequence, ' ' line '\n'
        let seq = self.count();
        let mut header = [0u8; 10];
        header[0] = tag;
        let seq_hex = format!("{:08X}", seq);
        header[1..9].copy_from_slice(seq_hex.as_bytes());
        header[9] = b' ';

        let mut payload: Vec<u8> = Vec::with_capacity(MAX_LINE + 2);
        for b in line.as_bytes().iter().take(MAX_LINE) {
            payload.push(*b);
        }
        payload.push(b'\n');

        if start + header.len() + payload.len() > CAPACITY {
            let keep = header.len() + payload.len();
            if keep >= CAPACITY {
                self.dropped += 1;
                return;
            }
            let overflow = start + header.len() + payload.len() - CAPACITY;
            self.buf.drain(..overflow);
            self.dropped += 1;
        }

        self.buf.extend_from_slice(&header);
        self.buf.extend_from_slice(&payload);
    }

    fn count(&self) -> u32 {
        self.entries() as u32
    }

    /// The raw log bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Number of complete entries recorded.
    pub fn entries(&self) -> usize {
        self.buf.iter().filter(|b| **b == b'\n').count()
    }

    /// Render the log as text, one line per entry.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}

impl Default for Trace {
    fn default() -> Trace {
        Trace::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_tagged_lines() {
        let mut t = Trace::new();
        t.set_enabled(true);
        t.record(tag::POST, "hello");
        let text = t.text();
        assert!(text.ends_with(" hello\n"), "got {:?}", text);
        assert!(text.starts_with('P'), "got {:?}", text);
        assert_eq!(t.entries(), 1);
    }

    #[test]
    fn stays_quiet_when_disabled() {
        let mut t = Trace::new();
        t.record(tag::POST, "should not appear");
        assert_eq!(t.entries(), 0);
    }

    #[test]
    fn wraps_instead_of_growing() {
        let mut t = Trace::new();
        t.set_enabled(true);
        for i in 0..2000
        {
            t.record(tag::VIDEO, &format!("line {}", i));
        }
        assert!(t.buf.len() <= CAPACITY, "ring exceeded its capacity");
        assert!(t.dropped > 0, "wrapping should be counted");
    }
}

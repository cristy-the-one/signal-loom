//! Read-only SocketCAN capture.
//!
//! This opens a raw CAN socket, binds it, and reads. It never sends a frame.
//! There is no `write`, `send`, or `sendto` on this socket.

use crate::error::{Error, Result};
use crate::scan::hex_payload;
use crate::IndexControl;
use std::time::Duration;

/// Frames kept from one capture. Past this the window is cut short and says so.
const MAX_FRAMES: usize = 200_000;

/// Raw frames read in the window, and whether the window was cut short at `MAX_FRAMES`.
type Window = (Vec<(u64, [u8; 16])>, bool);

/// Turn one classic `can_frame` (16 bytes, little-endian) into a SLOGv1 line.
/// A DLC 0 frame has no payload token, which the SLOG reader reads as DLC 0.
pub fn frame_line(t_us: u64, frame: &[u8]) -> Result<String> {
    if frame.len() < 16 {
        return Err(Error::msg("CAN frame is shorter than 16 bytes"));
    }
    let id = u32::from_le_bytes(frame[0..4].try_into().unwrap()) & 0x1FFF_FFFF;
    let dlc = frame[4].min(8);
    let hex = hex_payload(&frame[8..], dlc);
    if hex.is_empty() {
        return Ok(format!("F {t_us} {id:X}"));
    }
    Ok(format!("F {t_us} {id:X} {hex}"))
}

/// Read `iface` for `duration_ms` and return a SLOGv1 log. Read only.
/// `control` shows the elapsed milliseconds and the frames so far, and stops
/// the capture early when a cancel is requested.
pub fn capture_slog(
    iface: &str,
    duration_ms: u64,
    control: Option<&IndexControl>,
) -> Result<String> {
    if !(1..=30_000).contains(&duration_ms) {
        return Err(Error::msg("capture length must be between 1 ms and 30 s"));
    }
    if !valid_iface(iface) {
        return Err(Error::msg(
            "interface name must be a short Linux device name, such as can0 or vcan0",
        ));
    }
    if let Some(control) = control {
        control.set_total(duration_ms);
    }
    let (frames, capped) = read_only(iface, Duration::from_millis(duration_ms), control)?;
    if frames.is_empty() {
        return Err(Error::msg(format!(
            "no CAN frames on {iface} in {duration_ms} ms. The socket was read-only."
        )));
    }
    let mut out =
        String::from("SLOGv1\n# Read-only SocketCAN capture. Signal Loom did not transmit.\n");
    for (t_us, frame) in &frames {
        out.push_str(&frame_line(*t_us, frame)?);
        out.push('\n');
    }
    if capped {
        let t_us = frames.last().map_or(0, |(t_us, _)| *t_us);
        out.push_str(&format!(
            "E {t_us} Capture stopped at {MAX_FRAMES} frames; the rest of the {duration_ms} ms window was not recorded\n"
        ));
    }
    Ok(out)
}

fn valid_iface(iface: &str) -> bool {
    let mut chars = iface.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() || iface.len() > 15 {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(target_os = "linux")]
fn read_only(iface: &str, duration: Duration, control: Option<&IndexControl>) -> Result<Window> {
    // Safety: the socket is created, bound, and read. It is closed before return.
    // No bytes are written to the CAN controller.
    unsafe { read_only_fd(iface, duration, control) }
}

#[cfg(not(target_os = "linux"))]
fn read_only(_iface: &str, _duration: Duration, _control: Option<&IndexControl>) -> Result<Window> {
    Err(Error::msg("SocketCAN capture is only available on Linux"))
}

#[cfg(target_os = "linux")]
unsafe fn read_only_fd(
    iface: &str,
    duration: Duration,
    control: Option<&IndexControl>,
) -> Result<Window> {
    let fd = unsafe { libc::socket(libc::AF_CAN, libc::SOCK_RAW, libc::CAN_RAW) };
    if fd < 0 {
        return Err(Error::msg(format!(
            "could not open a read-only CAN socket: {}",
            std::io::Error::last_os_error()
        )));
    }
    let _guard = Close(fd);
    let index = if_index(fd, iface)?;
    let mut addr: libc::sockaddr_can = unsafe { std::mem::zeroed() };
    addr.can_family = libc::AF_CAN as u16;
    addr.can_ifindex = index;
    let bound = unsafe {
        libc::bind(
            fd,
            &addr as *const libc::sockaddr_can as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_can>() as libc::socklen_t,
        )
    };
    if bound != 0 {
        return Err(Error::msg(format!(
            "could not bind {iface} read-only: {}",
            std::io::Error::last_os_error()
        )));
    }
    let timeout = libc::timeval {
        tv_sec: 0,
        tv_usec: 200_000,
    };
    let timed = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if timed != 0 {
        return Err(Error::msg(format!(
            "could not set a read timeout on {iface}: {}",
            std::io::Error::last_os_error()
        )));
    }
    let start = std::time::Instant::now();
    let mut frames = Vec::new();
    let mut capped = false;
    while start.elapsed() < duration {
        if let Some(control) = control {
            control
                .observe(start.elapsed().as_millis() as u64, frames.len() as u64, 0)
                .map_err(|_| Error::msg("capture cancelled"))?;
        }
        if frames.len() >= MAX_FRAMES {
            capped = true;
            break;
        }
        let mut buf = [0u8; 16];
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock
                || err.raw_os_error() == Some(libc::EAGAIN)
            {
                continue;
            }
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(Error::msg(format!("CAN read failed: {err}")));
        }
        if n >= 16 {
            let t_us = start.elapsed().as_micros() as u64;
            frames.push((t_us, buf));
        }
    }
    Ok((frames, capped))
}

#[cfg(target_os = "linux")]
fn if_index(fd: i32, iface: &str) -> Result<i32> {
    let mut req: libc::ifreq = unsafe { std::mem::zeroed() };
    let bytes = iface.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        req.ifr_name[i] = *byte as libc::c_char;
    }
    let rc = unsafe { libc::ioctl(fd, libc::SIOCGIFINDEX, &mut req) };
    if rc < 0 {
        return Err(Error::msg(format!(
            "interface {iface} is not available: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(unsafe { req.ifr_ifru.ifru_ifindex })
}

#[cfg(target_os = "linux")]
struct Close(i32);

#[cfg(target_os = "linux")]
impl Drop for Close {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{LogFormat, RecKind, Scanner};

    #[test]
    fn decodes_a_classic_frame_without_touching_a_socket() {
        let mut frame = [0u8; 16];
        frame[0] = 0xA0;
        frame[1] = 0x01;
        frame[4] = 4;
        frame[8] = 0x11;
        frame[9] = 0x22;
        frame[10] = 0x33;
        frame[11] = 0x44;
        let line = frame_line(1500, &frame).unwrap();
        assert_eq!(line, "F 1500 1A0 11223344");
    }

    #[test]
    fn zero_length_frame_round_trips_as_dlc_zero() {
        let mut frame = [0u8; 16];
        frame[0] = 0xA0;
        frame[1] = 0x01;
        frame[4] = 0;
        let line = frame_line(1500, &frame).unwrap();
        assert_eq!(line, "F 1500 1A0");
        let text = format!("SLOGv1\n{line}\n");
        let mut cursor = std::io::Cursor::new(text.into_bytes());
        let mut scanner = Scanner::open(&mut cursor, LogFormat::Slog).unwrap();
        let rec = scanner.next_rec().unwrap().unwrap();
        assert_eq!(rec.t_us, 1500);
        let RecKind::Frame { id, dlc, .. } = rec.kind else {
            panic!("expected a frame");
        };
        assert_eq!((id, dlc), (0x1A0, 0));
    }

    #[test]
    fn rejects_a_strange_interface_name() {
        let err = capture_slog("can0;reboot", 10, None).unwrap_err();
        assert!(err.to_string().contains("interface"), "{err}");
    }
}

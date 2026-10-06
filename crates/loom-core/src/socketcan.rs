//! Read-only SocketCAN capture.
//!
//! This opens a raw CAN socket, binds it, and reads. It never sends a frame.
//! There is no `write`, `send`, or `sendto` on this socket.

use crate::error::{Error, Result};
use std::time::Duration;

/// Turn one classic `can_frame` (16 bytes, little-endian) into a SLOGv1 line.
pub fn frame_line(t_us: u64, frame: &[u8]) -> Result<String> {
    if frame.len() < 16 {
        return Err(Error::msg("CAN frame is shorter than 16 bytes"));
    }
    let id = u32::from_le_bytes(frame[0..4].try_into().unwrap()) & 0x1FFF_FFFF;
    let dlc = frame[4].min(8) as usize;
    let data = &frame[8..8 + dlc];
    let mut hex = String::new();
    for byte in data {
        hex.push_str(&format!("{byte:02X}"));
    }
    if hex.is_empty() {
        hex.push_str("00");
    }
    Ok(format!("F {t_us} {id:X} {hex}"))
}

/// Read `iface` for `duration_ms` and return a SLOGv1 log. Read only.
pub fn capture_slog(iface: &str, duration_ms: u64) -> Result<String> {
    if !(1..=30_000).contains(&duration_ms) {
        return Err(Error::msg("capture length must be between 1 ms and 30 s"));
    }
    if !valid_iface(iface) {
        return Err(Error::msg(
            "interface name must be a short Linux device name, such as can0 or vcan0",
        ));
    }
    let frames = read_only(iface, Duration::from_millis(duration_ms))?;
    if frames.is_empty() {
        return Err(Error::msg(format!(
            "no CAN frames on {iface} in {duration_ms} ms. The socket was read-only."
        )));
    }
    let mut out =
        String::from("SLOGv1\n# Read-only SocketCAN capture. Signal Loom did not transmit.\n");
    for (t_us, frame) in frames {
        out.push_str(&frame_line(t_us, &frame)?);
        out.push('\n');
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
fn read_only(iface: &str, duration: Duration) -> Result<Vec<(u64, [u8; 16])>> {
    // Safety: the socket is created, bound, and read. It is closed before return.
    // No bytes are written to the CAN controller.
    unsafe { read_only_fd(iface, duration) }
}

#[cfg(not(target_os = "linux"))]
fn read_only(_iface: &str, _duration: Duration) -> Result<Vec<(u64, [u8; 16])>> {
    Err(Error::msg("SocketCAN capture is only available on Linux"))
}

#[cfg(target_os = "linux")]
unsafe fn read_only_fd(iface: &str, duration: Duration) -> Result<Vec<(u64, [u8; 16])>> {
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
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
    let start = std::time::Instant::now();
    let mut frames = Vec::new();
    while start.elapsed() < duration && frames.len() < 200_000 {
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
    Ok(frames)
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
    fn rejects_a_strange_interface_name() {
        let err = capture_slog("can0;reboot", 10).unwrap_err();
        assert!(err.to_string().contains("interface"), "{err}");
    }
}

//! Reading and writing a Unix socket together with the files passed along
//! it (SCM_RIGHTS): graphics buffers and shared memory travel that way
//! (DRI3, MIT-SHM's file descriptors), both to the display and back.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// The most files taken with one read.
const MOST_FDS: usize = 32;

fn cmsg_space(n: usize) -> usize {
    // SAFETY: CMSG_SPACE only computes a size.
    unsafe { libc::CMSG_SPACE((n * std::mem::size_of::<RawFd>()) as u32) as usize }
}

/// Reads what's there into `buf`; 0 bytes: the other end closed.
pub fn recv(s: &UnixStream, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
    let mut control = vec![0u8; cmsg_space(MOST_FDS)];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    // SAFETY: msghdr is plain data; every pointer in it is to a live buffer of the given length.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len() as _;
    let n = loop {
        // SAFETY: as above; MSG_CMSG_CLOEXEC so no program the gate starts inherits them.
        let n = unsafe { libc::recvmsg(s.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
        if n >= 0 {
            break n as usize;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    };
    // SAFETY: walking the control messages the kernel just wrote, within msg_controllen.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c).cast::<RawFd>();
                let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    fds.push(OwnedFd::from_raw_fd(data.add(i).read_unaligned()));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    Ok(n)
}

/// Writes all of `data`, the files going with its first byte (then closed here).
pub fn send(s: &UnixStream, mut data: &[u8], fds: &mut Vec<OwnedFd>) -> io::Result<()> {
    while !data.is_empty() || !fds.is_empty() {
        let raw: Vec<RawFd> = fds.iter().take(MOST_FDS).map(AsRawFd::as_raw_fd).collect();
        let mut control = vec![0u8; cmsg_space(raw.len().max(1))];
        // Files can't travel without a byte; the display reads none extra
        // since there's always a request with them.
        let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut _, iov_len: data.len() };
        // SAFETY: as in recv.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if !raw.is_empty() {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = cmsg_space(raw.len()) as _;
            // SAFETY: the header and data fit in `control` (sized by CMSG_SPACE for this many).
            unsafe {
                let c = libc::CMSG_FIRSTHDR(&msg);
                (*c).cmsg_level = libc::SOL_SOCKET;
                (*c).cmsg_type = libc::SCM_RIGHTS;
                (*c).cmsg_len = libc::CMSG_LEN((raw.len() * std::mem::size_of::<RawFd>()) as u32) as _;
                std::ptr::copy_nonoverlapping(raw.as_ptr(), libc::CMSG_DATA(c).cast::<RawFd>(), raw.len());
            }
        }
        if data.is_empty() {
            // Nothing to carry them: they go with the next write.
            return Ok(());
        }
        // SAFETY: as in recv. MSG_NOSIGNAL: a closed end is an error, not SIGPIPE.
        let n = unsafe { libc::sendmsg(s.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        fds.drain(..raw.len());
        data = &data[n as usize..];
    }
    Ok(())
}

/// Who's at the other end: process, user, group.
pub fn peer(s: &UnixStream) -> Option<(u32, u32, u32)> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred is a ucred and len its size.
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (r == 0 && cred.pid > 0).then_some((cred.pid as u32, cred.uid, cred.gid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn files_travel_with_the_bytes() {
        let (a, b) = UnixStream::pair().unwrap();
        let (r, w) = UnixStream::pair().unwrap();
        let mut fds = vec![OwnedFd::from(w.try_clone().unwrap())];
        send(&a, b"hello", &mut fds).unwrap();
        assert!(fds.is_empty());
        let (mut buf, mut got) = ([0u8; 16], Vec::new());
        assert_eq!(recv(&b, &mut buf, &mut got).unwrap(), 5);
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(got.len(), 1);
        // The file that came is the one sent: writing to it reaches `r`.
        std::io::Write::write_all(&mut UnixStream::from(got.pop().unwrap()), b"x").unwrap();
        drop(w);
        let mut one = [0u8; 1];
        (&r).read_exact(&mut one).unwrap();
        assert_eq!(&one, b"x");
        assert_eq!(peer(&a).map(|p| p.0), Some(std::process::id()));
    }
}

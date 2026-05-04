#![allow(dead_code, unused_imports, unused_variables)]
#![allow(clippy::all)]

use std::collections::HashSet;
use std::env;
use std::ffi::CString;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::str::FromStr;

use libc::{
    c_int, c_void, iovec, pid_t, process_vm_readv, sockaddr_in, sockaddr_in6, sockaddr_un, AF_INET,
    AF_INET6, AF_UNIX,
};

// Seccomp constants
const SECCOMP_SET_MODE_FILTER: u32 = 1;
const SECCOMP_FILTER_FLAG_NEW_LISTENER: u32 = 8;
const SECCOMP_RET_ALLOW: u32 = 0x7FFF0000;
const SECCOMP_RET_USER_NOTIF: u32 = 0x7FC00000;
const SECCOMP_RET_ERRNO: u32 = 0x00050000;
const EPERM: u32 = 1;
const SYS_CONNECT: u32 = 42;

// BPF constants
const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;
const BPF_RET: u16 = 0x06;

const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
const AUDIT_ARCH_X86_64: u32 = 0xC000003E;

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

fn bpf_stmt(code: u16, k: u32) -> SockFilter {
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn bpf_jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

#[repr(C)]
struct SeccompData {
    nr: i32,
    arch: u32,
    instruction_pointer: u64,
    args: [u64; 6],
}

#[repr(C)]
struct SeccompNotif {
    id: u64,
    pid: u32,
    flags: u32,
    data: SeccompData,
}

#[repr(C)]
struct SeccompNotifResp {
    id: u64,
    val: i64,
    error: i32,
    flags: u32,
}

const SECCOMP_USER_NOTIF_FLAG_CONTINUE: u32 = 1;

// IOCTL calculation macros in rust
const _IOC_NRBITS: u32 = 8;
const _IOC_TYPEBITS: u32 = 8;
const _IOC_SIZEBITS: u32 = 14;
const _IOC_DIRBITS: u32 = 2;

const _IOC_NRSHIFT: u32 = 0;
const _IOC_TYPESHIFT: u32 = _IOC_NRSHIFT + _IOC_NRBITS;
const _IOC_SIZESHIFT: u32 = _IOC_TYPESHIFT + _IOC_TYPEBITS;
const _IOC_DIRSHIFT: u32 = _IOC_SIZESHIFT + _IOC_SIZEBITS;

const _IOC_NONE: u32 = 0;
const _IOC_WRITE: u32 = 1;
const _IOC_READ: u32 = 2;

macro_rules! _IOC {
    ($dir:expr, $type:expr, $nr:expr, $size:expr) => {
        (($dir as u32) << _IOC_DIRSHIFT)
            | (($type as u32) << _IOC_TYPESHIFT)
            | (($nr as u32) << _IOC_NRSHIFT)
            | (($size as u32) << _IOC_SIZESHIFT)
    };
}

macro_rules! _IOWR {
    ($type:expr, $nr:expr, $size:expr) => {
        _IOC!(_IOC_READ | _IOC_WRITE, $type, $nr, $size)
    };
}

const SECCOMP_IOC_MAGIC: u8 = b'!';
const SECCOMP_IOCTL_NOTIF_RECV: u64 =
    _IOWR!(SECCOMP_IOC_MAGIC, 0, std::mem::size_of::<SeccompNotif>()) as u64;
const SECCOMP_IOCTL_NOTIF_SEND: u64 = _IOWR!(
    SECCOMP_IOC_MAGIC,
    1,
    std::mem::size_of::<SeccompNotifResp>()
) as u64;

fn resolve_domains(domains: &[&str]) -> (HashSet<IpAddr>, Vec<String>) {
    let mut ips = HashSet::new();
    let mut wildcards = Vec::new();
    // Always allow loopback
    ips.insert(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    ips.insert(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)));

    // Parse /etc/resolv.conf for DNS servers
    if let Ok(contents) = std::fs::read_to_string("/etc/resolv.conf") {
        for line in contents.lines() {
            if line.starts_with("nameserver ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(ip) = IpAddr::from_str(parts[1]) {
                        ips.insert(ip);
                    }
                }
            }
        }
    }

    for domain in domains {
        let domain = domain.trim();
        if domain.is_empty() {
            continue;
        }
        if domain.starts_with("*.") {
            // Wildcard: store pattern for runtime reverse-DNS matching
            wildcards.push(domain.to_string());
            // Pre-resolve the base domain and common subdomain prefixes
            let base = &domain[2..];
            let prefixes = [
                "", "www.", "api.", "cdn.", "raw.", "assets.", "static.", "docs.", "app.", "m.",
                "mail.", "ns1.", "ns2.",
            ];
            for prefix in &prefixes {
                let fqdn = format!("{}{}:80", prefix, base);
                if let Ok(addrs) = fqdn.to_socket_addrs() {
                    for addr in addrs {
                        ips.insert(addr.ip());
                    }
                }
            }
        } else {
            let addr_str = format!("{}:80", domain);
            if let Ok(addrs) = addr_str.to_socket_addrs() {
                for addr in addrs {
                    ips.insert(addr.ip());
                }
            }
        }
    }
    (ips, wildcards)
}

/// Reverse DNS lookup: IP → hostname via libc::getnameinfo
fn reverse_dns(ip: IpAddr) -> Option<String> {
    unsafe {
        let mut host = [0u8; 256];
        let (sa_ptr, sa_len): (*const libc::sockaddr, libc::socklen_t) = match ip {
            IpAddr::V4(v4) => {
                let mut sa: libc::sockaddr_in = std::mem::zeroed();
                sa.sin_family = libc::AF_INET as libc::sa_family_t;
                sa.sin_addr.s_addr = u32::from_ne_bytes(v4.octets());
                let boxed = Box::new(sa);
                let ptr = Box::into_raw(boxed);
                (
                    ptr as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                )
            }
            IpAddr::V6(v6) => {
                let mut sa: libc::sockaddr_in6 = std::mem::zeroed();
                sa.sin6_family = libc::AF_INET6 as libc::sa_family_t;
                sa.sin6_addr.s6_addr = v6.octets();
                let boxed = Box::new(sa);
                let ptr = Box::into_raw(boxed);
                (
                    ptr as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
                )
            }
        };

        let ret = libc::getnameinfo(
            sa_ptr,
            sa_len,
            host.as_mut_ptr() as *mut libc::c_char,
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            0, // NI_NAMEREQD would fail if no name; 0 returns IP as fallback
        );

        // Clean up the boxed sockaddr
        match ip {
            IpAddr::V4(_) => {
                let _ = Box::from_raw(sa_ptr as *mut libc::sockaddr_in);
            }
            IpAddr::V6(_) => {
                let _ = Box::from_raw(sa_ptr as *mut libc::sockaddr_in6);
            }
        }

        if ret == 0 {
            let hostname = std::ffi::CStr::from_ptr(host.as_ptr() as *const libc::c_char)
                .to_str()
                .ok()?
                .to_string();
            // getnameinfo may return the IP as string if no PTR record; skip that
            if hostname.parse::<IpAddr>().is_ok() {
                None
            } else {
                Some(hostname)
            }
        } else {
            None
        }
    }
}

/// Check if an IP matches any wildcard pattern via reverse DNS
fn matches_wildcard(ip: IpAddr, wildcards: &[String]) -> bool {
    if wildcards.is_empty() {
        return false;
    }
    if let Some(hostname) = reverse_dns(ip) {
        for pattern in wildcards {
            // pattern = "*.github.com" → suffix = ".github.com"
            if let Some(suffix) = pattern.strip_prefix('*') {
                if hostname.ends_with(suffix) || hostname == &suffix[1..] {
                    return true;
                }
            }
        }
    }
    false
}

fn send_fd(sock: RawFd, fd: RawFd) -> std::io::Result<()> {
    unsafe {
        let mut iov_buf: u8 = 0;
        let mut iov = libc::iovec {
            iov_base: &mut iov_buf as *mut _ as *mut c_void,
            iov_len: 1,
        };

        // Use a properly-sized and aligned control buffer
        let cmsg_space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
        let mut cmsg_buf = vec![0u8; cmsg_space];

        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut c_void;
        msg.msg_controllen = cmsg_space as _;

        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::copy_nonoverlapping(
            &fd as *const RawFd as *const u8,
            libc::CMSG_DATA(cmsg),
            std::mem::size_of::<RawFd>(),
        );

        if libc::sendmsg(sock, &msg, 0) < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn recv_fd(sock: RawFd) -> std::io::Result<RawFd> {
    unsafe {
        let mut iov_buf: u8 = 0;
        let mut iov = libc::iovec {
            iov_base: &mut iov_buf as *mut _ as *mut c_void,
            iov_len: 1,
        };

        let cmsg_space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
        let mut cmsg_buf = vec![0u8; cmsg_space];

        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut c_void;
        msg.msg_controllen = cmsg_space as _;

        if libc::recvmsg(sock, &mut msg, 0) < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no cmsg received",
            ));
        }

        let mut fd: RawFd = -1;
        std::ptr::copy_nonoverlapping(
            libc::CMSG_DATA(cmsg),
            &mut fd as *mut RawFd as *mut u8,
            std::mem::size_of::<RawFd>(),
        );
        Ok(fd)
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!("Usage: rune-net-guard --allow-domains <domains> -- <command> [args...]");
        std::process::exit(1);
    }

    let mut allowed_domains = String::new();
    let mut cmd_idx = 0;

    for i in 1..args.len() {
        if args[i] == "--allow-domains" && i + 1 < args.len() {
            allowed_domains = args[i + 1].clone();
        } else if args[i] == "--" {
            cmd_idx = i + 1;
            break;
        }
    }

    if cmd_idx == 0 || cmd_idx >= args.len() {
        eprintln!("Missing command to execute");
        std::process::exit(1);
    }

    let domains: Vec<&str> = allowed_domains.split(',').collect();
    let (allowed_ips, wildcard_patterns) = resolve_domains(&domains);

    let mut sv = [0; 2];
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) } < 0 {
        eprintln!("socketpair failed");
        std::process::exit(1);
    }

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        eprintln!("fork failed");
        std::process::exit(1);
    }

    if pid == 0 {
        // Child
        unsafe {
            libc::close(sv[0]);
        }
        let sock = sv[1];

        // Install seccomp filter
        let mut filter: Vec<SockFilter> = Vec::new();
        filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARCH));
        filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 0, 3));
        filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_NR));
        filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, SYS_CONNECT, 0, 1));
        filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_USER_NOTIF));
        filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));

        let prog = SockFprog {
            len: filter.len() as u16,
            filter: filter.as_ptr(),
        };

        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                eprintln!("prctl(NO_NEW_PRIVS) failed");
                std::process::exit(1);
            }

            let notif_fd = libc::syscall(
                libc::SYS_seccomp,
                SECCOMP_SET_MODE_FILTER as libc::c_ulong,
                SECCOMP_FILTER_FLAG_NEW_LISTENER as libc::c_ulong,
                &prog as *const SockFprog,
            ) as RawFd;

            if notif_fd < 0 {
                let e = std::io::Error::last_os_error();
                eprintln!("seccomp failed: {} (notif_fd raw={})", e, notif_fd);
                std::process::exit(1);
            }

            if let Err(e) = send_fd(sock, notif_fd) {
                eprintln!("Failed to send notif_fd: {}", e);
                std::process::exit(1);
            }

            libc::close(notif_fd);
            libc::close(sock);
        }

        let err = Command::new(&args[cmd_idx])
            .args(&args[cmd_idx + 1..])
            .exec();
        eprintln!("exec failed: {}", err);
        std::process::exit(1);
    } else {
        // Parent
        unsafe {
            libc::close(sv[1]);
        }
        let sock = sv[0];

        let notif_fd = match recv_fd(sock) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("Failed to receive notif_fd: {}", e);
                std::process::exit(1);
            }
        };

        unsafe {
            libc::close(sock);
        }

        loop {
            let mut req: SeccompNotif = unsafe { std::mem::zeroed() };
            let ret = unsafe { libc::ioctl(notif_fd, SECCOMP_IOCTL_NOTIF_RECV, &mut req) };
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                break; // child exited
            }

            let mut resp: SeccompNotifResp = unsafe { std::mem::zeroed() };
            resp.id = req.id;

            if req.data.nr == SYS_CONNECT as i32 {
                let sockaddr_ptr = req.data.args[1] as *const c_void;
                let addrlen = req.data.args[2] as usize;

                let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                let read_len = addrlen.min(std::mem::size_of::<libc::sockaddr_storage>());

                // Try process_vm_readv first, fallback to /proc/pid/mem
                let nread = unsafe {
                    let local_iov = iovec {
                        iov_base: &mut addr as *mut _ as *mut c_void,
                        iov_len: read_len,
                    };
                    let remote_iov = iovec {
                        iov_base: sockaddr_ptr as *mut c_void,
                        iov_len: read_len,
                    };
                    process_vm_readv(req.pid as pid_t, &local_iov, 1, &remote_iov, 1, 0)
                };
                let nread = if nread < 0 {
                    // Fallback: read from /proc/pid/mem
                    use std::io::{Read, Seek, SeekFrom};
                    let mem_path = format!("/proc/{}/mem", req.pid);
                    match std::fs::File::open(&mem_path) {
                        Ok(mut f) => {
                            let offset = sockaddr_ptr as u64;
                            if f.seek(SeekFrom::Start(offset)).is_ok() {
                                let buf = unsafe {
                                    std::slice::from_raw_parts_mut(
                                        &mut addr as *mut _ as *mut u8,
                                        read_len,
                                    )
                                };
                                match f.read(buf) {
                                    Ok(n) => n as isize,
                                    Err(_) => -1,
                                }
                            } else {
                                -1
                            }
                        }
                        Err(_) => -1,
                    }
                } else {
                    nread
                };

                let mut allowed = false;

                if nread > 0 {
                    let family = addr.ss_family as i32;
                    if family == AF_UNIX {
                        allowed = true;
                    } else if family == AF_INET {
                        let sin = unsafe { *(&addr as *const _ as *const sockaddr_in) };
                        let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                        let port = u16::from_be(sin.sin_port);
                        // Allow DNS queries (port 53) and loopback unconditionally
                        if port == 53
                            || ip.is_loopback()
                            || allowed_ips.contains(&IpAddr::V4(ip))
                            || matches_wildcard(IpAddr::V4(ip), &wildcard_patterns)
                        {
                            allowed = true;
                        }
                    } else if family == AF_INET6 {
                        let sin6 = unsafe { *(&addr as *const _ as *const sockaddr_in6) };
                        let ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                        let port = u16::from_be(sin6.sin6_port);
                        // Allow DNS queries (port 53) and loopback unconditionally
                        if port == 53
                            || ip.is_loopback()
                            || allowed_ips.contains(&IpAddr::V6(ip))
                            || matches_wildcard(IpAddr::V6(ip), &wildcard_patterns)
                        {
                            allowed = true;
                        }
                    }
                }

                if allowed {
                    resp.error = 0;
                    resp.flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;
                } else {
                    resp.error = -libc::EPERM;
                    resp.flags = 0;
                }
            } else {
                resp.error = 0;
                resp.flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;
            }

            unsafe {
                libc::ioctl(notif_fd, SECCOMP_IOCTL_NOTIF_SEND, &mut resp);
            }
        }

        let mut status = 0;
        unsafe {
            libc::waitpid(pid, &mut status, 0);
        }

        if libc::WIFEXITED(status) {
            std::process::exit(libc::WEXITSTATUS(status));
        } else {
            std::process::exit(1);
        }
    }
}

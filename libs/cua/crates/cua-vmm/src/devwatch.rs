//! Keeps a running personal Space's GPU device nodes equal to the host's cards
//! (#3282, design section 3).
//!
//! Under gVisor's GPU proxy a card reaches a program in the Space through two
//! nodes: one in the device gofer's `/dev` (the gofer opens the host's card
//! through it) and one in the Space's own `/dev` (the sentry's file that the
//! program opens). Both sets are made at start. A card that the host gains
//! later needs both nodes; a card that left needs both removed. This module
//! plans those steps from three sets of card numbers; the caller reads the
//! sets and carries the steps out (a bind mount in the gofer's namespaces, the
//! runsc refresh call for the sentry).

use std::collections::BTreeSet;
use std::path::Path;

/// One step that brings a Space's nodes to the host's cards.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeStep {
    /// Make `/dev/nvidia<minor>` in the device gofer's `/dev`.
    AddGofer(u32),
    /// Remove it from the gofer's `/dev`.
    RemoveGofer(u32),
    /// Ask the sentry to make or drop its nodes from the gofer's set (one
    /// call covers every card; runsc reads the gofer's `/dev` itself).
    RefreshSpace,
}

/// A source of card numbers (the minor of `/dev/nvidia<minor>`).
pub trait DeviceSource {
    /// The host's cards now.
    fn host(&self) -> BTreeSet<u32>;
    /// The cards the Space's device gofer has nodes for.
    fn gofer(&self) -> BTreeSet<u32>;
    /// The cards the Space's own `/dev` has nodes for.
    fn space(&self) -> BTreeSet<u32>;
}

/// The steps that make the gofer's and the Space's nodes equal to the host's
/// cards: gofer nodes first (the sentry's refresh reads them), then one refresh
/// when the Space's set differs from the host's after those steps.
pub fn plan(source: &dyn DeviceSource) -> Vec<NodeStep> {
    let host = source.host();
    let gofer = source.gofer();
    let space = source.space();
    let mut steps: Vec<NodeStep> = host
        .difference(&gofer)
        .map(|m| NodeStep::AddGofer(*m))
        .collect();
    steps.extend(gofer.difference(&host).map(|m| NodeStep::RemoveGofer(*m)));
    if space != host {
        steps.push(NodeStep::RefreshSpace);
    }
    steps
}

/// The card numbers among the names of a `/dev` directory: `nvidia<N>` only
/// (not `nvidiactl`, `nvidia-uvm`, `nvidia-caps`).
pub fn card_minors<'a>(names: impl Iterator<Item = &'a str>) -> BTreeSet<u32> {
    names
        .filter_map(|n| n.strip_prefix("nvidia"))
        .filter(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|rest| rest.parse().ok())
        .collect()
}

/// Whether a kernel device event (a uevent from the kernel's netlink group)
/// can change the host's NVIDIA cards: a PCI device bound to the NVIDIA
/// driver, or a PCI device unbound or removed (the kernel sends no driver name
/// on unbind, so every PCI unbind and remove counts; the reconcile that
/// follows reads the cards as they are and does nothing when none changed).
pub fn affects_cards(msg: &[u8]) -> bool {
    let mut action = None;
    let mut subsystem = None;
    let mut driver = None;
    for field in msg.split(|b| *b == 0) {
        let field = std::str::from_utf8(field).unwrap_or("");
        if let Some(v) = field.strip_prefix("ACTION=") {
            action = Some(v);
        } else if let Some(v) = field.strip_prefix("SUBSYSTEM=") {
            subsystem = Some(v);
        } else if let Some(v) = field.strip_prefix("DRIVER=") {
            driver = Some(v);
        }
    }
    subsystem == Some("pci")
        && match action {
            Some("bind") => driver == Some("nvidia"),
            Some("unbind") | Some("remove") => true,
            _ => false,
        }
}

/// Calls `on_change` for every kernel device event that can change the host's
/// NVIDIA cards, until the socket fails. It reads the kernel's own uevent
/// group, which needs no privilege; it never opens a card.
#[cfg(target_os = "linux")]
pub fn watch_kernel(mut on_change: impl FnMut()) -> std::io::Result<()> {
    // SAFETY: plain socket calls with checked results; the buffer outlives recv.
    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        );
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut addr: libc::sockaddr_nl = std::mem::zeroed();
        addr.nl_family = libc::AF_NETLINK as u16;
        // Group 1: the kernel's events (group 2 is udev's re-broadcast).
        addr.nl_groups = 1;
        if libc::bind(
            fd,
            &addr as *const libc::sockaddr_nl as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        ) < 0
        {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0);
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                libc::close(fd);
                return Err(e);
            }
            if affects_cards(&buf[..n as usize]) {
                on_change();
            }
        }
    }
}

/// The device gofer's pid of container `cid`, from runsc's state file in its
/// root directory (`<cid>_sand:<sandbox id>.state`, key `goferPid`).
pub fn gofer_pid(runsc_root: &Path, cid: &str) -> Option<i32> {
    let prefix = format!("{cid}_sand:");
    let entry = std::fs::read_dir(runsc_root).ok()?.flatten().find(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        name.starts_with(&prefix) && name.ends_with(".state")
    })?;
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?;
    state
        .get("goferPid")?
        .as_i64()
        .map(|p| p as i32)
        .filter(|p| *p > 0)
}

/// Carries out node steps for one running Space: the gofer-side node inside
/// the device gofer's namespaces, then the sentry's refresh through runsc.
/// The host side only makes or removes the node; it never opens a card.
#[cfg(target_os = "linux")]
pub struct GoferNodes {
    /// The device gofer's pid ([`gofer_pid`]).
    pub gofer_pid: i32,
    /// The runsc binary and its `--root` (the engine's runtime root).
    pub runsc: std::path::PathBuf,
    pub runsc_root: std::path::PathBuf,
    /// The container's id.
    pub container_id: String,
}

#[cfg(target_os = "linux")]
impl GoferNodes {
    /// Carries out `steps` in order and returns runsc's refresh output when
    /// a refresh ran.
    pub fn apply(&self, steps: &[NodeStep]) -> std::io::Result<Option<String>> {
        let mut out = None;
        for step in steps {
            match step {
                NodeStep::AddGofer(m) => self.in_gofer(*m, true)?,
                NodeStep::RemoveGofer(m) => self.in_gofer(*m, false)?,
                NodeStep::RefreshSpace => out = Some(self.refresh()?),
            }
        }
        Ok(out)
    }

    fn refresh(&self) -> std::io::Result<String> {
        let out = std::process::Command::new(&self.runsc)
            .arg("--root")
            .arg(&self.runsc_root)
            .args(["nvproxy", "refresh", &self.container_id])
            .stdin(std::process::Stdio::null())
            .output()?;
        if !out.status.success() {
            return Err(std::io::Error::other(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Makes (`add`) or removes `/dev/nvidia<minor>` in the gofer's mount
    /// namespace. A rootless engine's gofer runs in a user namespace where no
    /// node can be made, so the host's node is cloned (`open_tree`) in the
    /// host's namespace and moved onto an empty file in the gofer's
    /// (`move_mount`). This runs in a forked child: `setns` into a user
    /// namespace needs a single-threaded process.
    fn in_gofer(&self, minor: u32, add: bool) -> std::io::Result<()> {
        use std::ffi::CString;
        let host = CString::new(format!("/dev/nvidia{minor}")).unwrap();
        let target = host.clone();
        let user_ns = CString::new(format!("/proc/{}/ns/user", self.gofer_pid)).unwrap();
        let mnt_ns = CString::new(format!("/proc/{}/ns/mnt", self.gofer_pid)).unwrap();
        let own_user = std::fs::read_link("/proc/self/ns/user").ok();
        let their_user = std::fs::read_link(format!("/proc/{}/ns/user", self.gofer_pid)).ok();
        let enter_user = own_user != their_user;
        let empty = CString::new("").unwrap();
        // SAFETY: after fork the child calls only raw syscalls on values made
        // before the fork, then _exit; the parent waits for it.
        unsafe {
            let pid = libc::fork();
            if pid < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if pid == 0 {
                let code = (|| -> i32 {
                    let tree = if add {
                        let fd = libc::syscall(
                            libc::SYS_open_tree,
                            libc::AT_FDCWD,
                            host.as_ptr(),
                            1u32 /* OPEN_TREE_CLONE */ | libc::O_CLOEXEC as u32,
                        ) as i32;
                        if fd < 0 {
                            return 10;
                        }
                        fd
                    } else {
                        -1
                    };
                    if enter_user {
                        let fd = libc::open(user_ns.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
                        if fd < 0 || libc::setns(fd, libc::CLONE_NEWUSER) != 0 {
                            return 11;
                        }
                    }
                    let fd = libc::open(mnt_ns.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
                    if fd < 0 || libc::setns(fd, libc::CLONE_NEWNS) != 0 {
                        return 12;
                    }
                    if add {
                        let f = libc::open(
                            target.as_ptr(),
                            libc::O_CREAT | libc::O_WRONLY | libc::O_CLOEXEC,
                            0o666,
                        );
                        if f < 0 {
                            return 13;
                        }
                        libc::close(f);
                        if libc::syscall(
                            libc::SYS_move_mount,
                            tree,
                            empty.as_ptr(),
                            libc::AT_FDCWD,
                            target.as_ptr(),
                            4u32, /* MOVE_MOUNT_F_EMPTY_PATH */
                        ) != 0
                        {
                            return 14;
                        }
                    } else {
                        libc::umount2(target.as_ptr(), libc::MNT_DETACH);
                        if libc::unlink(target.as_ptr()) != 0
                            && *libc::__errno_location() != libc::ENOENT
                        {
                            return 15;
                        }
                    }
                    0
                })();
                libc::_exit(code);
            }
            let mut status = 0;
            if libc::waitpid(pid, &mut status, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else {
                -1
            };
            if code != 0 {
                let what = match code {
                    10 => "clone the host's node",
                    11 => "enter the gofer's user namespace",
                    12 => "enter the gofer's mount namespace",
                    13 => "create the target file",
                    14 => "move the node into place",
                    15 => "remove the node",
                    _ => "finish",
                };
                return Err(std::io::Error::other(format!(
                    "/dev/nvidia{minor} in the gofer of {}: could not {what}",
                    self.container_id
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        host: Vec<u32>,
        gofer: Vec<u32>,
        space: Vec<u32>,
    }

    impl DeviceSource for Fake {
        fn host(&self) -> BTreeSet<u32> {
            self.host.iter().copied().collect()
        }
        fn gofer(&self) -> BTreeSet<u32> {
            self.gofer.iter().copied().collect()
        }
        fn space(&self) -> BTreeSet<u32> {
            self.space.iter().copied().collect()
        }
    }

    #[test]
    fn nodes_equal_to_the_host_need_no_step() {
        assert!(
            plan(&Fake {
                host: vec![0, 1],
                gofer: vec![0, 1],
                space: vec![0, 1]
            })
            .is_empty()
        );
        assert!(
            plan(&Fake {
                host: vec![],
                gofer: vec![],
                space: vec![]
            })
            .is_empty()
        );
    }

    #[test]
    fn a_card_the_host_gained_gets_a_gofer_node_then_a_space_node() {
        let steps = plan(&Fake {
            host: vec![0, 1, 2],
            gofer: vec![0, 1],
            space: vec![0, 1],
        });
        assert_eq!(steps, vec![NodeStep::AddGofer(2), NodeStep::RefreshSpace]);
    }

    #[test]
    fn a_card_that_left_loses_both_nodes() {
        let steps = plan(&Fake {
            host: vec![0],
            gofer: vec![0, 3],
            space: vec![0, 3],
        });
        assert_eq!(
            steps,
            vec![NodeStep::RemoveGofer(3), NodeStep::RefreshSpace]
        );
    }

    #[test]
    fn a_space_behind_its_gofer_only_needs_the_refresh() {
        let steps = plan(&Fake {
            host: vec![0, 1],
            gofer: vec![0, 1],
            space: vec![0],
        });
        assert_eq!(steps, vec![NodeStep::RefreshSpace]);
    }

    fn uevent(fields: &[&str]) -> Vec<u8> {
        let mut msg = b"bind@/devices/pci0000:00/0000:41:00.0".to_vec();
        for f in fields {
            msg.push(0);
            msg.extend_from_slice(f.as_bytes());
        }
        msg
    }

    #[test]
    fn a_card_bound_to_nvidia_or_any_pci_unbind_or_remove_counts() {
        assert!(affects_cards(&uevent(&[
            "ACTION=bind",
            "SUBSYSTEM=pci",
            "DRIVER=nvidia"
        ])));
        assert!(affects_cards(&uevent(&["ACTION=unbind", "SUBSYSTEM=pci"])));
        assert!(affects_cards(&uevent(&["ACTION=remove", "SUBSYSTEM=pci"])));
        // Another driver, another subsystem, or an action that changes no binding: no.
        assert!(!affects_cards(&uevent(&[
            "ACTION=bind",
            "SUBSYSTEM=pci",
            "DRIVER=amdgpu"
        ])));
        assert!(!affects_cards(&uevent(&["ACTION=add", "SUBSYSTEM=usb"])));
        assert!(!affects_cards(&uevent(&[
            "ACTION=change",
            "SUBSYSTEM=pci",
            "DRIVER=nvidia"
        ])));
        assert!(!affects_cards(b"garbage"));
    }

    #[test]
    fn the_gofer_pid_comes_from_runscs_state_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("abc_sand:abc.state"),
            r#"{"goferPid": 4242, "id": "abc"}"#,
        )
        .unwrap();
        std::fs::write(root.path().join("other_sand:x.state"), r#"{"goferPid": 1}"#).unwrap();
        assert_eq!(gofer_pid(root.path(), "abc"), Some(4242));
        assert_eq!(gofer_pid(root.path(), "missing"), None);
        std::fs::write(root.path().join("zero_sand:z.state"), r#"{"goferPid": 0}"#).unwrap();
        assert_eq!(gofer_pid(root.path(), "zero"), None);
    }

    #[test]
    fn only_card_nodes_count() {
        let names = [
            "nvidia0",
            "nvidiactl",
            "nvidia-uvm",
            "nvidia-caps",
            "nvidia12",
            "nvidia-modeset",
            "null",
        ];
        assert_eq!(card_minors(names.into_iter()), BTreeSet::from([0, 12]));
    }
}

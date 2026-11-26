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

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

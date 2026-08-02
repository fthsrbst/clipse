//! The ordered list of addresses to try for one peer.
//!
//! Pairing records every address a peer advertises; mDNS refreshes the LAN
//! ones whenever the peer is on the same network, and `tailnet` resolves the
//! remote one. Dialling walks this list in order and takes the first that
//! connects.
//!
//! LAN comes first because it is faster and does not leave the building. The
//! tailnet address is the fallback, not a parallel path.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// How a peer address was learned, which is also its dial priority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reachability {
    /// Same network: discovered by mDNS, or recorded at pairing time.
    Lan,
    /// Reached over a Tailscale tailnet. There is no multicast on a tailnet,
    /// so this address is never discovered — only resolved by name.
    Tailnet,
}

/// How many LAN addresses to keep for one peer. A machine has a handful of
/// interfaces; anything past this is history, and history is what `dial_order`
/// puts last anyway.
const MAX_LAN_CANDIDATES: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub addr: SocketAddr,
    pub reachability: Reachability,
    /// When this address was last seen working or advertised. `None` means it
    /// came from pairing and has not been confirmed since.
    pub last_seen_ms: Option<u64>,
}

impl Candidate {
    pub fn lan(addr: SocketAddr) -> Self {
        Self {
            addr,
            reachability: Reachability::Lan,
            last_seen_ms: None,
        }
    }

    pub fn tailnet(addr: SocketAddr) -> Self {
        Self {
            addr,
            reachability: Reachability::Tailnet,
            last_seen_ms: None,
        }
    }

    pub fn seen_at(mut self, ms: u64) -> Self {
        self.last_seen_ms = Some(ms);
        self
    }
}

/// Every way we currently know of to reach one peer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateList {
    candidates: Vec<Candidate>,
}

impl CandidateList {
    pub fn new(candidates: impl IntoIterator<Item = Candidate>) -> Self {
        let mut list = Self::default();
        for candidate in candidates {
            list.upsert(candidate);
        }
        list
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Add an address, or refresh one we already have.
    ///
    /// Keyed on the socket address alone: the same address learned twice is
    /// one candidate, and a fresher `last_seen_ms` always wins so a peer that
    /// just answered on the LAN rises to the top of its group.
    pub fn upsert(&mut self, candidate: Candidate) {
        match self
            .candidates
            .iter_mut()
            .find(|c| c.addr == candidate.addr)
        {
            Some(existing) => {
                if candidate.last_seen_ms > existing.last_seen_ms {
                    existing.last_seen_ms = candidate.last_seen_ms;
                }
                // A LAN sighting of an address we had recorded as tailnet is
                // real information: the peer came home.
                existing.reachability = existing.reachability.min(candidate.reachability);
            }
            None => self.candidates.push(candidate),
        }
    }

    /// Fold what discovery just reported into the LAN entries.
    ///
    /// Additive, deliberately. This used to replace the LAN set outright, and
    /// that lost a real deployment: one browse resolved the peer with only
    /// link-local addresses it could not be dialled on, those four replaced
    /// the address that had been working, no later browse re-reported it, and
    /// the peer stayed unreachable until it was paired again.
    ///
    /// An address that has gone stale does not need deleting — [`dial_order`]
    /// sorts by when each was last seen, so it sinks to the bottom on its own,
    /// and being last costs one refused connect. Being *deleted* costs the
    /// whole peer. The cap is what stops a peer that restarts on a new
    /// ephemeral port every day from growing this list without end.
    ///
    /// [`dial_order`]: Self::dial_order
    pub fn refresh_lan(&mut self, discovered: impl IntoIterator<Item = SocketAddr>, now_ms: u64) {
        for addr in discovered {
            self.upsert(Candidate::lan(addr).seen_at(now_ms));
        }
        self.prune_lan();
    }

    /// Keep the most recently seen LAN addresses and drop the rest.
    fn prune_lan(&mut self) {
        let lan = self
            .candidates
            .iter()
            .filter(|c| c.reachability == Reachability::Lan)
            .count();
        if lan <= MAX_LAN_CANDIDATES {
            return;
        }

        let mut by_age: Vec<(usize, Option<u64>)> = self
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.reachability == Reachability::Lan)
            .map(|(index, c)| (index, c.last_seen_ms))
            .collect();
        // Never-seen last, so an address recorded at pairing time is the first
        // to go once real sightings have replaced it.
        by_age.sort_by_key(|(_, seen)| std::cmp::Reverse(*seen));

        let doomed: Vec<usize> = by_age
            .into_iter()
            .skip(MAX_LAN_CANDIDATES)
            .map(|(i, _)| i)
            .collect();
        let mut index = 0;
        self.candidates.retain(|_| {
            let keep = !doomed.contains(&index);
            index += 1;
            keep
        });
    }

    pub fn set_tailnet(&mut self, addr: Option<SocketAddr>, now_ms: u64) {
        self.candidates
            .retain(|c| c.reachability != Reachability::Tailnet);
        if let Some(addr) = addr {
            self.upsert(Candidate::tailnet(addr).seen_at(now_ms));
        }
    }

    /// The order to dial in: LAN before tailnet, and within each group the
    /// most recently seen first.
    pub fn dial_order(&self) -> Vec<&Candidate> {
        let mut ordered: Vec<&Candidate> = self.candidates.iter().collect();
        ordered.sort_by(|a, b| {
            a.reachability
                .cmp(&b.reachability)
                .then(b.last_seen_ms.cmp(&a.last_seen_ms))
        });
        ordered
    }

    pub fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    fn lan(last: u8, port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, last)), port)
    }

    fn tailnet(last: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 64, 0, last)), 7420)
    }

    #[test]
    fn lan_is_tried_before_tailnet() {
        let list = CandidateList::new([
            Candidate::tailnet(tailnet(1)),
            Candidate::lan(lan(10, 7420)),
        ]);

        let order = list.dial_order();
        assert_eq!(order[0].reachability, Reachability::Lan);
        assert_eq!(order[1].reachability, Reachability::Tailnet);
    }

    #[test]
    fn the_most_recently_seen_address_in_a_group_comes_first() {
        let list = CandidateList::new([
            Candidate::lan(lan(10, 7420)).seen_at(1_000),
            Candidate::lan(lan(11, 7420)).seen_at(9_000),
            Candidate::lan(lan(12, 7420)),
        ]);

        let order = list.dial_order();
        assert_eq!(order[0].addr, lan(11, 7420));
        assert_eq!(order[1].addr, lan(10, 7420));
        assert_eq!(order[2].addr, lan(12, 7420), "never-seen goes last");
    }

    #[test]
    fn upsert_keys_on_the_address_not_the_reachability() {
        let mut list = CandidateList::new([Candidate::lan(lan(10, 7420)).seen_at(1_000)]);
        list.upsert(Candidate::lan(lan(10, 7420)).seen_at(5_000));

        assert_eq!(list.len(), 1, "the same address must not be duplicated");
        assert_eq!(list.iter().next().unwrap().last_seen_ms, Some(5_000));
    }

    #[test]
    fn a_stale_sighting_does_not_overwrite_a_fresher_one() {
        let mut list = CandidateList::new([Candidate::lan(lan(10, 7420)).seen_at(9_000)]);
        list.upsert(Candidate::lan(lan(10, 7420)).seen_at(1_000));

        assert_eq!(list.iter().next().unwrap().last_seen_ms, Some(9_000));
    }

    #[test]
    fn seeing_a_tailnet_address_on_the_lan_promotes_it() {
        // The peer came home: the same address is now reachable directly, and
        // it should be dialled first from here on.
        let mut list = CandidateList::new([Candidate::tailnet(tailnet(1))]);
        list.upsert(Candidate::lan(tailnet(1)).seen_at(1_000));

        assert_eq!(list.len(), 1);
        assert_eq!(list.dial_order()[0].reachability, Reachability::Lan);
    }

    #[test]
    fn refreshing_the_lan_does_not_lose_the_way_home() {
        let mut list = CandidateList::new([
            Candidate::lan(lan(10, 7420)).seen_at(1_000),
            Candidate::tailnet(tailnet(1)).seen_at(1_000),
        ]);

        // The peer left the network: discovery reports nothing.
        list.refresh_lan([], 2_000);

        assert!(
            list.dial_order()
                .iter()
                .any(|c| c.reachability == Reachability::Tailnet),
            "the tailnet route must survive a browse that found nothing"
        );
    }

    #[test]
    fn a_new_lease_is_tried_before_the_old_one() {
        let mut list = CandidateList::new([Candidate::lan(lan(10, 7420)).seen_at(1_000)]);

        // Same peer, new DHCP lease.
        list.refresh_lan([lan(55, 7420)], 2_000);

        assert_eq!(list.dial_order()[0].addr, lan(55, 7420));
    }

    /// The deployment failure this list exists to survive: a browse resolves
    /// the peer with addresses that cannot be dialled, and the one that works
    /// must still be there afterwards.
    #[test]
    fn a_browse_that_reports_nothing_usable_does_not_evict_the_working_address() {
        let working = lan(9, 58_091);
        let mut list = CandidateList::new([Candidate::lan(working).seen_at(1_000)]);

        // Four sightings on a different interface, none of them reachable.
        list.refresh_lan(
            [
                lan(200, 65_026),
                lan(201, 65_026),
                lan(202, 65_026),
                lan(203, 65_026),
            ],
            2_000,
        );

        assert!(
            list.dial_order().iter().any(|c| c.addr == working),
            "the address that has been working was thrown away"
        );
    }

    #[test]
    fn a_peer_that_keeps_restarting_does_not_grow_the_list_forever() {
        let mut list = CandidateList::new([Candidate::lan(lan(9, 1)).seen_at(1)]);
        for port in 2..40u16 {
            list.refresh_lan([lan(9, port)], u64::from(port) * 1_000);
        }

        assert!(
            list.len() <= MAX_LAN_CANDIDATES,
            "candidate list grew to {}",
            list.len()
        );
        assert_eq!(
            list.dial_order()[0].addr,
            lan(9, 39),
            "the newest sighting must still be tried first"
        );
    }

    #[test]
    fn losing_tailscale_leaves_a_working_lan_only_list() {
        let mut list = CandidateList::new([
            Candidate::lan(lan(10, 7420)).seen_at(1_000),
            Candidate::tailnet(tailnet(1)).seen_at(1_000),
        ]);

        list.set_tailnet(None, 2_000);

        assert_eq!(list.len(), 1);
        assert_eq!(list.dial_order()[0].reachability, Reachability::Lan);
    }

    #[test]
    fn ipv6_addresses_are_ordinary_candidates() {
        let v6 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0, 1)),
            7420,
        );
        let list = CandidateList::new([Candidate::tailnet(v6), Candidate::lan(lan(10, 7420))]);

        assert_eq!(list.len(), 2);
        assert_eq!(list.dial_order()[1].addr, v6);
    }

    #[test]
    fn an_empty_list_is_dialable_without_panicking() {
        let list = CandidateList::default();
        assert!(list.is_empty());
        assert!(list.dial_order().is_empty());
    }

    #[test]
    fn the_list_survives_a_round_trip_through_serde() {
        let list = CandidateList::new([
            Candidate::lan(lan(10, 7420)).seen_at(1_000),
            Candidate::tailnet(tailnet(1)),
        ]);
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(serde_json::from_str::<CandidateList>(&json).unwrap(), list);
    }
}

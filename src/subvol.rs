//! Sub-volume identity and snapshot-family aware inline dedup.
//!
//! Btrfs inode numbers are only unique *within* a subvolume, and every
//! subvolume numbers its inodes from 256 upwards.  Two unrelated
//! subvolumes therefore collide routinely, while a snapshot deliberately
//! reproduces the inode numbers of its source (and shares the same inline
//! data through copy-on-write).  Deduplicating inline extents by inode
//! number alone cannot tell those two cases apart.
//!
//! We group subvolumes into "families" using the snapshot lineage exposed
//! by `BTRFS_IOC_GET_SUBVOL_INFO` (`uuid` / `parent_uuid`) and dedup by
//! `(family, ino)` instead: hardlinks and snapshots collapse, unrelated
//! subvolumes do not.
//!
//! Inline dedup is the only consumer of this module.  Regular extents
//! carry a `disk_bytenr`, which is already unique per filesystem, so they
//! keep deduplicating globally.

use std::{
    collections::HashMap,
    os::fd::BorrowedFd,
    sync::{LazyLock, Mutex, MutexGuard},
};

use nohash::IntSet;
use rustix::{
    fs::fstat,
    io::{Errno, Result},
    ioctl::{Getter, ioctl},
};

use crate::btrfs::ioctl::{BTRFS_IOC_GET_SUBVOL_INFO, GetSubvolInfoArgs};

type Uuid = [u8; 16];
const ZERO_UUID: Uuid = [0; 16];

/// Stand-in identity for a subvolume whose real UUID could not be read, so
/// that it still gets a family of its own instead of sharing one with every
/// other failed lookup.
fn synthetic_uuid(dev: u64) -> Uuid {
    let mut uuid = ZERO_UUID;
    uuid[..8].copy_from_slice(&dev.to_le_bytes());
    uuid
}

fn get_subvol_info(fd: BorrowedFd<'_>) -> Result<(Uuid, Uuid)> {
    // SAFETY: `BTRFS_IOC_GET_SUBVOL_INFO` is a read-only `_IOR` whose size
    // matches `GetSubvolInfoArgs` exactly (see the const asserts in
    // `btrfs::ioctl`), and `Getter` hands the kernel a properly sized,
    // properly aligned buffer to write into.
    let info = unsafe {
        ioctl(
            fd,
            Getter::<BTRFS_IOC_GET_SUBVOL_INFO, GetSubvolInfoArgs>::new(),
        )
    }?;
    Ok((info.uuid, info.parent_uuid))
}

/// Union-find over subvolume UUIDs plus the per-family inline inode sets.
struct Registry {
    /// uuid -> node index
    nodes: HashMap<Uuid, u32>,
    /// union-find parent array, indexed by node
    parent: Vec<u32>,
    /// device number -> (node, whether it came from a real UUID)
    by_dev: HashMap<u64, (u32, bool)>,
    /// devices we already looked up and whether `SEARCH_V2` can work there
    attempted: HashMap<u64, Support>,
    /// family root -> inode numbers already counted as unique
    inline: HashMap<u32, IntSet<u64>>,
}

impl Registry {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            parent: Vec::new(),
            by_dev: HashMap::new(),
            attempted: HashMap::new(),
            inline: HashMap::new(),
        }
    }

    fn node(&mut self, uuid: Uuid) -> u32 {
        if let Some(&node) = self.nodes.get(&uuid) {
            return node;
        }
        let node = self.parent.len() as u32;
        self.nodes.insert(uuid, node);
        self.parent.push(node);
        node
    }

    fn find(&mut self, mut node: u32) -> u32 {
        let mut root = node;
        while self.parent[root as usize] != root {
            root = self.parent[root as usize];
        }
        // Path compression.
        while self.parent[node as usize] != root {
            let next = self.parent[node as usize];
            self.parent[node as usize] = root;
            node = next;
        }
        root
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        // Keep the smallest index as representative so the choice does not
        // depend on discovery order beyond "first wins".
        let (keep, drop) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[drop as usize] = keep;
        // A snapshot can be discovered after some of its extents were
        // already counted, which merges two existing families late.  Merge
        // the inode sets as well, or a shared inode would later be counted
        // unique a second time.
        if let Some(set) = self.inline.remove(&drop) {
            self.inline.entry(keep).or_default().extend(set);
        }
    }

    fn register(&mut self, dev: u64, uuid: Uuid, parent_uuid: Uuid) -> u32 {
        let node = self.node(uuid);
        if parent_uuid != ZERO_UUID {
            let parent = self.node(parent_uuid);
            self.union(node, parent);
        }
        // Fold in a synthetic entry created by an extent that arrived
        // before its subvolume was registered.
        if let Some((old, false)) = self.by_dev.insert(dev, (node, true)) {
            self.union(old, node);
        }
        self.find(node)
    }

    /// Give `dev` a family of its own when its real UUID is unavailable.
    fn synthetic(&mut self, dev: u64) -> u32 {
        let node = self.node(synthetic_uuid(dev));
        self.by_dev.entry(dev).or_insert((node, false));
        node
    }

    fn inline_seen(&mut self, dev: u64, ino: u64) -> bool {
        let node = match self.by_dev.get(&dev) {
            Some(&(node, _)) => node,
            None => self.synthetic(dev),
        };
        let root = self.find(node);
        self.inline.entry(root).or_default().insert(ino)
    }
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::new()));

fn lock() -> MutexGuard<'static, Registry> {
    // A poisoned lock would only mean a previous caller panicked while
    // updating plain bookkeeping; the data is still consistent.
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `SEARCH_V2` can be used on an fd.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Support {
    Btrfs,
    /// No btrfs ioctl here.  Seen for non-btrfs bind mounts and for the
    /// stub directories a snapshot leaves where a nested subvolume was
    /// (they report the btrfs magic from `statfs` but reject every ioctl).
    Unsupported,
}

/// Register the subvolume containing `fd`, returning its device number and
/// whether it is searchable.
///
/// Cheap to call for every directory chunk / top-level argument: each
/// distinct device number is looked up at most once, and failures are
/// cached silently.  A `BTRFS_IOC_GET_SUBVOL_INFO` failure is not fatal:
/// the subvolume simply gets a synthetic family of its own (no snapshot
/// dedup), unless the ioctl says there is no btrfs here at all, in which
/// case callers should not descend into it.
///
/// A real subvolume root is inode 256; keep it even if the ioctl is
/// missing from the kernel, so older kernels still measure nested
/// subvolumes (just without cross-snapshot dedup).
pub fn register_fd(fd: BorrowedFd<'_>) -> Result<(u64, Support)> {
    let st = fstat(fd)?;
    let dev = st.st_dev;
    let mut registry = lock();
    if let Some(&support) = registry.attempted.get(&dev) {
        return Ok((dev, support));
    }
    let info = get_subvol_info(fd);
    let unsupported = info.as_ref().err().copied() == Some(Errno::NOTTY) && st.st_ino != 256;
    match info {
        Ok((uuid, parent_uuid)) => {
            registry.register(dev, uuid, parent_uuid);
        }
        Err(_) => {
            registry.synthetic(dev);
        }
    }
    let support = if unsupported {
        Support::Unsupported
    } else {
        Support::Btrfs
    };
    registry.attempted.insert(dev, support);
    Ok((dev, support))
}

/// Record `(subvolume family, ino)` and report whether this inode's inline
/// extent was seen for the first time.
pub fn inline_seen(dev: u64, ino: u64) -> bool {
    lock().inline_seen(dev, ino)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(n: u8) -> Uuid {
        let mut u = ZERO_UUID;
        u[0] = n;
        u
    }

    /// Two brand-new subvolumes with no shared lineage must not collapse
    /// just because they reuse the same inode number.
    #[test]
    fn unrelated_subvolumes_are_distinct() {
        let mut r = Registry::new();
        r.register(1, uuid(1), ZERO_UUID);
        r.register(2, uuid(2), ZERO_UUID);
        assert!(r.inline_seen(1, 300));
        assert!(r.inline_seen(2, 300), "unrelated ino must not be deduped");
    }

    /// A hardlink / single path revisited is deduped within one subvolume.
    #[test]
    fn hardlinks_dedup() {
        let mut r = Registry::new();
        r.register(1, uuid(1), ZERO_UUID);
        assert!(r.inline_seen(1, 300));
        assert!(!r.inline_seen(1, 300));
    }

    /// A snapshot shares its source's inode numbers and inline data, even
    /// through a chain of snapshots.
    #[test]
    fn snapshots_dedup_transitively() {
        let mut r = Registry::new();
        r.register(1, uuid(1), ZERO_UUID); // A
        r.register(2, uuid(2), uuid(1)); // B = snap(A)
        r.register(3, uuid(3), uuid(2)); // C = snap(B)
        assert!(r.inline_seen(1, 300));
        assert!(!r.inline_seen(2, 300));
        assert!(!r.inline_seen(3, 300));
    }

    /// Discovering a middle subvolume after its descendants can merge two
    /// already-populated families; the inode sets must merge with it.
    #[test]
    fn late_union_merges_inode_sets() {
        let mut r = Registry::new();
        r.register(1, uuid(0xA1), uuid(0xA0)); // Y = snap(X)
        r.register(2, uuid(0xB1), uuid(0xB0)); // W = snap(Z)
        assert!(r.inline_seen(1, 10));
        assert!(r.inline_seen(2, 20));

        // X is itself a snapshot of Z: this connects {X, Y} with {Z, W}.
        r.register(3, uuid(0xA0), uuid(0xB0));

        assert!(!r.inline_seen(1, 10));
        assert!(!r.inline_seen(2, 20));
        assert!(!r.inline_seen(1, 20), "set 2 must have merged into set 1");
        assert!(!r.inline_seen(2, 10), "set 1 must have merged into set 2");
        assert!(r.inline_seen(3, 30));
    }

    /// An extent seen before its subvolume is registered must still end up
    /// in the real family once the registration arrives.
    #[test]
    fn synthetic_entry_upgrades_to_real_family() {
        let mut r = Registry::new();
        assert!(r.inline_seen(7, 42)); // no registration yet
        r.register(1, uuid(1), ZERO_UUID);
        r.register(7, uuid(1), ZERO_UUID); // same family as dev 1
        assert!(!r.inline_seen(1, 42), "synthetic set must be adopted");
        assert!(r.inline_seen(1, 43));
    }
}

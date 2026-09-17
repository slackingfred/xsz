use rustix::ioctl::{
    Opcode,
    opcode::{read, read_write},
};

pub const BTRFS_IOCTL_MAGIC: u8 = 0x94;
pub const BTRFS_IOCTL_SEARCH_V2: Opcode = read_write::<Sv2Args>(BTRFS_IOCTL_MAGIC, 17);
pub const BTRFS_IOC_GET_SUBVOL_INFO: Opcode = read::<GetSubvolInfoArgs>(BTRFS_IOCTL_MAGIC, 60);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct IoctlSearchKey {
    pub tree_id: u64,
    pub min_objectid: u64,
    pub max_objectid: u64,
    pub min_offset: u64,
    pub max_offset: u64,
    pub min_transid: u64,
    pub max_transid: u64,
    pub min_type: u32,
    pub max_type: u32,
    pub nr_items: u32,
    unused: u32,
    unused1: u64,
    unused2: u64,
    unused3: u64,
    unused4: u64,
}

impl IoctlSearchKey {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tree_id: u64,
        min_objectid: u64,
        max_objectid: u64,
        min_offset: u64,
        max_offset: u64,
        min_transid: u64,
        max_transid: u64,
        min_type: u8,
        max_type: u8,
    ) -> Self {
        Self {
            tree_id,
            min_objectid,
            max_objectid,
            min_offset,
            max_offset,
            min_transid,
            max_transid,
            min_type: min_type as _,
            max_type: max_type as _,
            nr_items: u32::MAX,
            unused: 0,
            unused1: 0,
            unused2: 0,
            unused3: 0,
            unused4: 0,
        }
    }
}

// should be reused for different files
#[derive(Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Sv2Args {
    pub key: IoctlSearchKey,
    buf_size: u64,
    buf: [u8; 16384],
}

impl Sv2Args {
    #[inline]
    pub fn from_sk(sk: IoctlSearchKey) -> Self {
        Self {
            key: sk,
            buf_size: 16384,
            buf: [0; 16384],
        }
    }

    #[inline]
    pub fn buf(&self) -> &[u8; 16384] {
        &self.buf
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct SearchHeader {
    pub transid: u64,
    pub objectid: u64,
    pub offset: u64,
    pub r#type: u32,
    pub len: u32,
}
impl SearchHeader {
    /// # Safety
    /// `buf` must contain at least `size_of::<SearchHeader>()` (32) bytes.
    /// The caller must ensure the buffer is valid for reads.
    #[inline]
    pub unsafe fn from_raw(buf: &[u8]) -> Self {
        unsafe { buf.as_ptr().cast::<Self>().read_unaligned() }
    }
}

#[repr(C)]
struct Timespec {
    sec: u64,
    nsec: u32,
}

/// Mirrors the kernel's `struct btrfs_ioctl_get_subvol_info_args`.
///
/// The exact size and field offsets matter: [`BTRFS_IOC_GET_SUBVOL_INFO`]
/// encodes `size_of::<Self>()` in `_IOC_SIZE`, and the kernel dispatches on
/// the whole command word (unlike `SEARCH_V2`, there is no modulus trick
/// here).  The asserts below pin the layout to the `linux/btrfs.h`
/// definition, so a future edit that breaks it fails at compile time
/// instead of with `ENOTTY` at runtime.
#[repr(C)]
pub struct GetSubvolInfoArgs {
    treeid: u64,
    name: [u8; 256], // BTRFS_VOL_NAME_MAX + 1
    parent_id: u64,
    dirid: u64,
    generation: u64,
    flags: u64,
    pub(crate) uuid: [u8; 16],
    pub(crate) parent_uuid: [u8; 16],
    received_uuid: [u8; 16],
    ctransid: u64,
    otransid: u64,
    stransid: u64,
    rtransid: u64,
    ctime: Timespec,
    otime: Timespec,
    stime: Timespec,
    rtime: Timespec,
    reserved: [u64; 8],
}

const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<Timespec>() == 16);
    assert!(size_of::<GetSubvolInfoArgs>() == 504);
    assert!(offset_of!(GetSubvolInfoArgs, uuid) == 296);
    assert!(offset_of!(GetSubvolInfoArgs, parent_uuid) == 312);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin both command words against `include/uapi/linux/btrfs.h`.
    ///
    /// `SEARCH_V2` is the subtle one: `Sv2Args` inlines a 16 KiB buffer, so
    /// `size_of::<Sv2Args>()` is 16496 and only collapses back to the
    /// kernel's expected 112 because `_IOC_SIZE` is 14 bits wide.  Changing
    /// the buffer to a non-multiple of 16384 would silently send a different
    /// command and fail with `ENOTTY`; this test catches that.
    #[test]
    fn ioctl_commands_match_kernel() {
        assert_eq!(BTRFS_IOC_GET_SUBVOL_INFO, 0x81f8_943c);
        assert_eq!(BTRFS_IOCTL_SEARCH_V2, 0xc070_9411);
    }
}

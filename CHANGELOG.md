# Changelog

All notable changes to `xsz` are documented here, newest first.
For the intentional differences from `compsize`, see
[Behavior differences from `compsize`](README.md#behavior-differences-from-compsize).

### 0.5.3 — 2026-09-18

- **Inline dedup across subvolumes**: inline extents are now deduplicated
  by `(snapshot family, inode)` instead of inode alone, so unrelated
  subvolumes no longer collide. Snapshot families come from
  `BTRFS_IOC_GET_SUBVOL_INFO`.
- **Inline extents with a payload are accepted again**: `EXTENT_DATA`
  validation demanded an inline item be exactly the 21-byte header, which
  rejected every non-empty inline extent and aborted the run.
- **Non-btrfs mounts are skipped**: a directory whose
  `BTRFS_IOC_GET_SUBVOL_INFO` reports no btrfs subvolume (a bind mount of
  another filesystem, or a snapshot's stub for a nested subvolume) is no
  longer descended into. Previously that aborted the whole run.
- **Non-UTF8 paths**: directory entries and positional arguments no longer
  have to be valid UTF-8. A non-UTF8 entry used to panic the walker (exit
  101) and a non-UTF8 argument was rejected at startup.
- **Relative paths with `-t`**: `xsz -t .` walked up through an empty path
  and silently reported "No Files." with exit 0. Arguments are now made
  absolute first, and a failed subvolume-root lookup is reported instead
  of being swallowed.
- **Malformed extent data**: a malformed kernel reply used to trip a
  length assertion and abort the process; it is now reported as a scan
  error.
- **Report formatting**: sizes of 2^62 bytes or more no longer overflow
  the unit shift, an empty fragment distribution no longer prints
  sentinel Min/Max/Avg values, and a zero total uncompressed size no
  longer divides by zero.
- **Executor**: worker stacks are raised from 4 KiB (below
  `PTHREAD_STACK_MIN` on glibc) to 128 KiB, and an executor channel error
  no longer makes `block_on` drain in a loop forever.
- **`compsize` parity**: directories are opened with `NOCTTY | NONBLOCK`
  and one that disappears mid-walk (`ENOENT`) is skipped instead of
  aborting the run.
- **Packaging**: a Nix flake was added.
- **Regular extent `refs` count**: the `Y` in `X regular extents (Y refs)`
  is now exact. v0.5.2 computed `all items − unique inline inodes`, so every
  duplicate inline reference (a hardlinked or snapshotted symlink or small
  file) was counted as a regular ref. `Y` is smaller than in v0.5.2 by the
  number of duplicate inline references.
- **Output**: the summary line now also reports the inline reference count:
  `Processed N files, X regular extents (Y refs), A inline (B refs).`
  (The `Processed ... inline.` lines in the README benchmarks predate this
  field, and the `compsize` lines predate its own `fragments` field. See
  [Behavior differences from `compsize`](README.md#behavior-differences-from-compsize).)

### 0.5.2 — 2026-07-17

- **Failed `stat()`**: `get_dev()` no longer unwraps, so a path that
  cannot be `stat`ed (or whose device id is 0) is reported with its error
  and skipped instead of aborting the run.

### 0.5.1 — 2026-07-01

- **Multiple paths with `-t`**: `xsz -t a b` used to scan only `a` and
  silently ignore the rest. Every argument is now resolved to its
  enclosing subvolume root and all roots are scanned; arguments that share
  a subvolume are deduplicated. Tree-scan still covers the whole
  subvolume, not just the directory named on the command line. (The
  default walkdir mode already accepted multiple paths.)
- **HDD performance**: tree-scan scans subvolume roots through a bounded
  worker pool instead of spawning one task per root.
- **Memory**: the per-worker `SEARCH_V2` ioctl buffer is reduced from
  64 KiB to 16 KiB.
- **Docs**: the inline-dedup limitation is documented.

### 0.5.0 — 2026-06-29

- **`-t` / `--tree-scan`**: new flag to scan btrfs tree directly instead of
  walking directory hierarchy. Up to 4× faster on large subvolumes.
- **Inline dedup**: xsz now deduplicates inline extents by inode
- **Symlink support**: symlinks (stored as inline extents in btrfs) are now
  counted.

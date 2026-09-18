# xsz - A Multi-threaded Btrfs Compression Analysis Tool

This is a rewrite of `compsize`, but faster, and use less memory.

## Introduction

`xsz` is an open-source tool designed to measure the used compression types
and effective compression ratios of files on a Btrfs filesystem,
producing a comprehensive report. It is inspired by [compsize](https://github.com/kilobyte/compsize)
and maintains compatibility with its command-line interface.
However, xsz goes a step further by incorporating a variety of optimization techniques
to significantly enhance performance,
particularly on NVMe SSDs or with hot metadata cache, while also delivering substantial
speed improvements across a wider range of hardware configurations.
Under cache-hot conditions, it achieves over 50% higher performance even in single-threaded mode,
and scales further to deliver markedly faster speeds when running with multiple threads.

## Installation

With Nix, run xsz directly from the flake:

```sh
nix run github:SaltyKitkat/xsz -- /path/to/dir
```

Or install it into your profile:

```sh
nix profile add github:SaltyKitkat/xsz
```

The flake also provides a Rust development shell through `nix develop`.

## Usage

`xsz` follows the same command-line syntax as `compsize`. To get started, simply run:

```sh

xsz /path/to/dir
```

For more detailed usage instructions and options, refer to the help message:

```console
xsz --help
Usage: xsz [options] file-or-dir1 [file-or-dir2 ...]

xsz displays total space used by set of files, taking into account
compression, reflinks, partially overwritten extents.

Options:
    -h, --help              print this help message and exit
    -b, --bytes             display raw bytes instead of human-readable sizes
    -x, --one-file-system   don't cross filesystem boundaries
    -j N, --jobs=N          allow N jobs at once
    -t, --tree-scan         scan btrfs tree instead of walking directory (faster on subvolumes)
```

## Important Notes

This project has not undergone rigorous testing. Use it in production environments at your own risk.

**`-t` / `--tree-scan` mode** scans the btrfs tree per-inode instead of
walking the directory hierarchy. Disk Usage and Uncompressed numbers
are consistent between modes. The "Referenced" column will differ when
hardlinks exist: walkdir counts each path's reference separately, while
tree-scan counts each extent once. Its `Processed N files` value is not
comparable to `compsize` either; see
[Behavior differences from `compsize`](#behavior-differences-from-compsize).

**Inline extent dedup** groups subvolumes into snapshot families (using the
`uuid`/`parent_uuid` lineage reported by `BTRFS_IOC_GET_SUBVOL_INFO`) and
deduplicates inline extents by `(family, inode)`, for Disk Usage and
Uncompressed only — `Referenced` still counts every path. This collapses
hardlinks and snapshots (same inode = same inline data) while keeping unrelated
subvolumes separate — the latter matters because every btrfs subvolume
numbers its inodes from 256, so an inode-only key collides across them
routinely. Inode number reuse *within* a snapshot family can still cause a
small under-count. Because `compsize` performs no such dedup, this is a source
of difference from it; see
[Behavior differences from `compsize`](#behavior-differences-from-compsize).

**Summary line** reports both unique extents and references, for regular and
inline data alike:

```
Processed N files, X regular extents (Y refs), A inline (B refs).
```

The parenthesized inline value is `compsize`'s own `inline` count; the leading
one counts distinct inodes — see
[Behavior differences from `compsize`](#behavior-differences-from-compsize).

## Behavior differences from `compsize`

`xsz` keeps `compsize`'s command line and its per-extent accounting, but it
deliberately reports a few things differently. Most of the table changes the
numbers, which is why `Processed N files` can legitimately disagree with
`compsize` on the same directory.

| Aspect | `compsize` | `xsz` |
| --- | --- | --- |
| Symlinks | filtered out of the directory walk (`DT_LNK`) and never counted | counted as files; their inline targets count towards Disk Usage / Uncompressed / Referenced ([details](#file-count)) |
| Inline extents | added every time the extent is reached, so hardlinks and snapshots are counted repeatedly | deduplicated by `(snapshot family, inode)`: Disk Usage / Uncompressed count the data once, Referenced still counts every path ([details](#inline-extents)) |
| File count | bumped after a successful `open()`, so unreadable or already-deleted files are skipped | bumped at `readdir` time; `xsz` never opens the file itself ([details](#file-count)) |
| Unknown compression id / extent type | bucketed and printed as a `?N` row | treated as malformed and aborts the run ([details](#error-handling)) |
| Non-btrfs directory (bind mount, subvolume stub) | walked into, then aborts on the first file with `Not btrfs` | detected with `GET_SUBVOL_INFO` and skipped ([details](#error-handling)) |
| Summary line | `... A inline, F fragments.` | `... A inline (B refs).`; no fragment count unless `-F` is used ([details](#output)) |
| Human-readable sizes | tail dropped (truncated) | rounded ([FAQ](#faq)) |

The rest is meant to match: regular extents are deduplicated by 4K page on
both sides, holes are skipped on both sides, and `Referenced` counts one
reference per path for regular and inline data alike.

### File count

`xsz` counts a file as soon as `readdir` returns it (`is_file()` or
`is_symlink()`), while `compsize` only counts regular files it managed to
`open()`. Two visible consequences:

- **Symlinks are files to `xsz`.** They are counted in `Processed N files`,
  and their inline targets (btrfs stores symlink targets as inline
  `EXTENT_DATA`) contribute to the byte totals. `compsize` ignores them
  completely: on a symlink-only tree it prints `No files.` where `xsz`
  reports the number of symlinks.
- **Unreadable files still count** for `xsz`, because it searches extents by
  inode through the directory fd instead of opening each file.

In `-t` / `--tree-scan` mode the count has yet another meaning: it is the
number of distinct inodes with a non-hole `EXTENT_DATA`. Empty or hole-only
files are therefore not counted, and hardlinks are counted once (see the `-t`
note in [Important Notes](#important-notes)).

### Inline extents

Both tools agree until the point of aggregation. `compsize` adds an inline
item's sizes every time it reaches it, so a hardlink, or a snapshot of a file
with inline data, is counted again for every path. `xsz` deduplicates inline
extents by `(snapshot family, inode)`: the first encounter adds Disk Usage and
Uncompressed, later ones only add `Referenced`. On snapshot-heavy trees `xsz`
therefore reports smaller Disk Usage and Uncompressed than `compsize`, by
exactly the repeatedly-reached inline data. Regular extents are deduplicated
by 4K page by both tools, so this only shows up for inline data (small files
and symlink targets). See the inline-dedup note in
[Important Notes](#important-notes) for the family logic and its limitations.

### Error handling

`compsize` is deliberately forward-compatible: any compression id is bucketed
and printed as a `?N` row, so a new algorithm still yields a report. `xsz`
knows only `none`/`zlib`/`lzo`/`zstd` and treats an unknown id or extent type
as malformed, aborting the run. Likewise, a directory that is not a searchable
btrfs subvolume — a bind mount of another filesystem, or the stub a snapshot
leaves for a nested subvolume — is skipped by `xsz`, whereas `compsize`
descends and aborts with `Not btrfs (or SEARCH_V2 unsupported)`.

### Output

The summary line is not a drop-in replacement:

```text
compsize: Processed N files, X regular extents (Y refs), A inline, F fragments.
xsz:      Processed N files, X regular extents (Y refs), A inline (B refs).
```

`compsize`'s `A` is the total number of inline items — that is `xsz`'s `B`
(the parenthesized reference count). `xsz`'s `A` is the number of distinct
inodes, which `compsize` does not report. Newer `compsize` versions also print
`F fragments`, a metric `xsz` only exposes through `-F` / `--frag`, as a full
distribution. Human-readable sizes round in `xsz` and truncate in `compsize`;
`-b` prints exact bytes in both.

## Changelog

### Unreleased

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
- **Output**: the summary line now also reports the inline reference count:
  `Processed N files, X regular extents (Y refs), A inline (B refs).`
  (The `Processed ... inline.` lines in the benchmarks below predate this
  field, and the `compsize` lines predate its own `fragments` field. See
  [Behavior differences from `compsize`](#behavior-differences-from-compsize).)

### 0.5.0 — 2026-06-29

- **`-t` / `--tree-scan`**: new flag to scan btrfs tree directly instead of
  walking directory hierarchy. Up to 4× faster on large subvolumes.
- **Inline dedup**: xsz now deduplicates inline extents by inode
- **Symlink support**: symlinks (stored as inline extents in btrfs) are now
  counted.

If you encounter any issues or have suggestions for improvement, please feel free to open an issue or join the discussion.

## Contribution

We warmly welcome contributions from the community! Areas where your help would be particularly valuable include:

  - Improving error handling.

  - Adding benchmarks.

  - Enhancing performance.

  - Improving code readability.

We're looking forward to your contribution and feedback.

## FAQ

Q: Why you write `xsz`?

A: Because `compsize` is so useful for me, but sometimes too slow. I had a try to speed it up, and `xsz` is the result.
   I'm glad that I can get a more than 4x speed up by using `xsz`.

Q: Why rust?

A: Because rust makes me, who is too stupid to write correct multi-thread programs in C,
   able to write a working multi-thread program. You know, multi-threading is important when you want to make
   some cpu bounded tasks faster.

Q: But is `compsize` really a cpu bounded task? Does multi-threading really work in this case?

A: Yes, and no. The task is actually at somewhere between cpu bounded and io bounded.
   If you run `compsize` on a fast enough disk, let's say, NVMe SSD, then you'll
   find that the cpu usage can be more than 50%, and reaching above 90%, while the load of SSD is quite low.
   So, yes, it is kind of a cpu bounded task. But if you run `compsize` on a HDD,
   you'll find that most time is spent on waiting the disk. So it's more like a io bounded task.

Q: So does `xsz` also run faster on HDD?

A: Yes! I think it's because we do a lot less open and close syscalls.
   So even running in single thread, `xsz` is a lot faster than `compsize`.
   But multi-threading hardly improve the performance in this case.

Q: Why `xsz` is faster?

A: TL;DR: Because we use multi-threading, and do less syscalls.

   Longer version:
   
   - we reduce a lot of unnecessary open and close syscalls;
   
   - we use multi-threading to call ioctl, which costs most time;
   
   - we use multi-threading to walkdir because when cache is hot, a single-thread walkdir can be the bottleneck.

   - `-t` mode skips directory walks entirely by scanning the btrfs tree directly.
     This avoids opening each file individually and is especially faster on large subvolumes.
     Both modes now count symlinks (stored as inline extents in btrfs) and handle
     hardlinks consistently for Disk Usage and Uncompressed numbers.

   Full version: I'm too lazy to finish this part now...

Q: Oh, yes, `xsz` seems good. Can I use it as a dropin replacement for `compsize`?

A: Yes, and no. We try to have the same cli-arguments with `compsize`, but we added a `-j`
   so you can set the number of worker threads, and `-t` to enable tree scan mode.
   And, `xsz` is not widely used and tested like `compsize`. So welcome to have a try.
   And if you find the result is different from `compsize`,
   it's expected because we do real rounding instead of just drop the tail like `compsize`
   since v0.4.1 release(commit 9549fa5).
   See [Behavior differences from `compsize`](#behavior-differences-from-compsize)
   for the full list of intentional differences (symlinks, inline dedup, input
   errors, ...).

Q: How many worker threads should I set on my machine?

A: I don't know. It depends on your cpu and disk. You can try to increase worker threads until
   either your cpu or your disk is under 100% load.

## Benchmark

On a SATA SSD device, with some snapshots for backing up, and some large media files.

Cache is cleared before each run.

```console
$ sudo time ./xsz -j1 /mnt/1
Processed 5663444 files, 1097233 regular extents (3859780 refs), 3459956 inline.
Type       Perc     Disk Usage   Uncompressed Referenced
TOTAL       95%      827G         865G         1.3T
none       100%      811G         811G         1.1T
zstd        29%       15G          54G         192G
4.27user 37.46system 1:14.13elapsed 56%CPU (0avgtext+0avgdata 31420maxresident)k
5060832inputs+0outputs (0major+46434minor)pagefaults 0swaps

$ sudo time ./xsz -j4 /mnt/1
Processed 5663444 files, 1097233 regular extents (3859780 refs), 3459956 inline.
Type       Perc     Disk Usage   Uncompressed Referenced
TOTAL       95%      827G         865G         1.3T
none       100%      811G         811G         1.1T
zstd        29%       15G          54G         192G
4.89user 37.88system 0:20.42elapsed 209%CPU (0avgtext+0avgdata 35820maxresident)k
5060832inputs+0outputs (0major+79430minor)pagefaults 0swaps

$ sudo time compsize /mnt/1
Processed 5663444 files, 1097233 regular extents (3859780 refs), 3459956 inline.
Type       Perc     Disk Usage   Uncompressed Referenced
TOTAL       95%      827G         865G         1.3T
none       100%      811G         811G         1.1T
zstd        29%       15G          54G         192G
3.72user 72.98system 1:50.07elapsed 69%CPU (0avgtext+0avgdata 80132maxresident)k
5254008inputs+0outputs (1major+24967minor)pagefaults 0swaps
```

On a HDD device, with a lot of small files, and some program files.

Cache is cleared before each run.

```console
$ sudo time ./xsz -j1 /mnt/guest
Processed 393486 files, 215207 regular extents (215207 refs), 178308 inline.
Type       Perc     Disk Usage   Uncompressed Referenced
TOTAL      100%       29G          29G          29G
none       100%       29G          29G          29G
0.37user 3.47system 1:07.54elapsed 5%CPU (0avgtext+0avgdata 5436maxresident)k
830912inputs+0outputs (0major+7825minor)pagefaults 0swaps

$ sudo time ./xsz -j4 /mnt/guest
Processed 393486 files, 215207 regular extents (215207 refs), 178308 inline.
Type       Perc     Disk Usage   Uncompressed Referenced
TOTAL      100%       29G          29G          29G
none       100%       29G          29G          29G
0.40user 3.97system 0:57.72elapsed 7%CPU (0avgtext+0avgdata 8064maxresident)k
830912inputs+0outputs (0major+5093minor)pagefaults 0swaps

$ sudo time compsize /mnt/guest
Processed 393486 files, 215207 regular extents (215207 refs), 178308 inline.
Type       Perc     Disk Usage   Uncompressed Referenced
TOTAL      100%       29G          29G          29G
none       100%       29G          29G          29G
0.42user 9.45system 2:49.78elapsed 5%CPU (0avgtext+0avgdata 12648maxresident)k
954080inputs+0outputs (0major+2990minor)pagefaults 0swaps
```

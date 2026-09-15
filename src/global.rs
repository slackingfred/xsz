use std::{
    hint::cold_path,
    process::exit,
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
};

use palc::Parser;

const HELP_MSG: &str = "xsz displays total space used by set of files, taking into account
compression, reflinks, partially overwritten extents.

Use -t / --tree-scan when operating on a btrfs subvolume root to scan
the tree directly instead of walking the directory hierarchy. This can
be significantly faster on large subvolumes.";

#[derive(Parser)]
#[command(long_about = HELP_MSG)]
pub struct Config {
    /// don't cross filesystem boundaries
    #[arg(short = 'x', long)]
    pub one_fs: bool,
    /// display raw bytes instead of human-readable sizes
    #[arg(short, long)]
    pub bytes: bool,
    /// allow N jobs at once
    #[arg(short, long, default_value_t = 1)]
    pub jobs: u8,
    /// print fragment length distribution summary
    #[arg(short = 'F', long)]
    pub frag: bool,
    /// scan btrfs tree instead of walking directory (faster on subvolumes)
    #[arg(short = 't', long)]
    pub tree_scan: bool,
    #[arg(required = true, value_name = "file-or-dir")]
    pub args: Vec<String>,
}
impl Config {
    fn from_args() -> Self {
        let opt = Config::parse();
        if opt.jobs == 0 {
            eprintln!("-j requires an non-zero integer");
            exit(1);
        }
        opt
    }
}
struct Global {
    err: AtomicBool,
    config: LazyLock<Config>,
}

impl Global {
    const fn new() -> Self {
        let err = AtomicBool::new(false);
        let config: LazyLock<Config> = LazyLock::new(Config::from_args);
        Self { err, config }
    }
}

#[inline]
const fn global() -> &'static Global {
    static GLOBAL: Global = Global::new();
    &GLOBAL
}

#[inline]
const fn global_err() -> &'static AtomicBool {
    &global().err
}

#[inline]
const fn bool_to_result(is_err: bool) -> Result<(), ()> {
    if is_err {
        cold_path();
        Err(())
    } else {
        Ok(())
    }
}

#[inline]
pub fn get_err() -> Result<(), ()> {
    bool_to_result(global_err().load(Ordering::Relaxed))
}

/// Claim the right to report the first error.
///
/// Returns `Ok(())` exactly once across all threads (the caller that
/// flipped the flag from `false` to `true`) and `Err(())` for every
/// later caller.  Callers use `set_err()?` so that the first thread to
/// hit an error prints it and continues unwinding locally, while the
/// rest bail out early without producing duplicate messages.
#[cold]
pub fn set_err() -> Result<(), ()> {
    cold_path();
    bool_to_result(global_err().swap(true, Ordering::Relaxed))
}

#[inline]
pub fn config() -> &'static Config {
    &global().config
}

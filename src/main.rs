// macrdp is a macOS app; the Linux build is only a compile-sanity stub (CI's
// cross-compile check), where the `#[cfg(target_os = "macos")]` paths are gone and
// a large amount of shared code/consts/helpers is naturally unused. Silence that
// class of noise on non-macOS only — the macOS `clippy -D warnings` gate stays
// fully strict (this never relaxes anything on the real build).
#![cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, unused_imports, unused_variables, unused_mut)
)]
// Every `unsafe` block states why it is sound (review finding Q2).
#![warn(clippy::undocumented_unsafe_blocks)]

mod aac;
mod app;
mod audio;
mod auth;
mod auth_guard;
mod avc444;
mod camera;
mod capture;
mod clipboard;
mod clipboard_rich;
#[cfg(test)]
mod color_metrics;
#[cfg(test)]
mod color_pattern;
#[cfg(all(test, target_os = "macos"))]
mod color_roundtrip_test;
#[cfg(test)]
mod conn_test;
mod credential_monitor;
mod cursor;
#[cfg(target_os = "macos")]
mod file_promise;
#[cfg(target_os = "macos")]
mod file_promise_lazy;
#[cfg(test)]
mod garbage_input_test;
#[cfg(target_os = "macos")]
mod h264;
mod health;
mod input;
mod keyboard_layout;
mod lock_activity;
mod logging;
#[allow(dead_code)]
mod lossless;
mod multitransport;
mod negotiator;
mod private_dir;
mod rdpdr;
mod reaper;
mod refine;
mod resync;
#[cfg(target_os = "macos")]
mod runloop_thread;
mod shield;
mod stats;
mod switcher_hud;
mod sync_ext;
mod tunables;
mod usb_redirect;
mod videotoolbox;
mod virtual_display;

use anyhow::Result;

/// The connection tests build servers with the same codec list.
#[cfg(test)]
pub(crate) use app::bitmap_codecs;

fn main() -> Result<()> {
    // Build the tokio runtime explicitly so every worker thread starts
    // at boosted QoS (see `boost_thread_qos`). `#[tokio::main]`'s
    // generated runtime gives us no hook for this. Crucially: the main
    // thread itself is NOT boosted — see the warning in
    // `boost_thread_qos` about CGVirtualDisplay + SCK enumeration.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .on_thread_start(|| {
            #[cfg(target_os = "macos")]
            app::boost_thread_qos();
        })
        .build()?;
    rt.block_on(app::run())
}

/// Every external program is started by absolute path, so nothing earlier on
/// `PATH` can stand in for it (review finding S6). Test-only files, which run
/// developer tools such as ffmpeg, are exempt.
#[cfg(test)]
mod command_path_tests {
    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn external_commands_use_absolute_paths() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        let needle = concat!("Command::new", "(\"");
        let mut bare = Vec::new();
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            if name.ends_with("_test.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            for (i, line) in text.lines().enumerate() {
                if let Some(at) = line.find(needle) {
                    if !line[at + needle.len()..].starts_with('/') {
                        bare.push(format!("{}:{}", file.display(), i + 1));
                    }
                }
            }
        }
        assert!(bare.is_empty(), "commands started by bare name: {bare:?}");
    }
}

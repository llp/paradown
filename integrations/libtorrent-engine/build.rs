#[cfg(feature = "native-libtorrent")]
use std::env;
#[cfg(feature = "native-libtorrent")]
use std::path::PathBuf;

#[cfg(not(feature = "native-libtorrent"))]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}

#[cfg(feature = "native-libtorrent")]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=src/native_engine.cc");
    println!("cargo:rerun-if-changed=include/paradown_libtorrent/native_engine.hpp");

    let mut build = cxx_build::bridge("src/ffi.rs");
    build
        .file("src/native_engine.cc")
        .include("include")
        .flag_if_supported("-std=c++17");

    match pkg_config::Config::new()
        .statik(env::var_os("CARGO_FEATURE_STATIC_LIBTORRENT").is_some())
        .probe("libtorrent-rasterbar")
    {
        Ok(library) => {
            for path in library.include_paths {
                build.include(path);
            }
        }
        Err(_) => {
            let mut found_header = false;
            for prefix in candidate_prefixes() {
                let include = prefix.join("include");
                if include.join("libtorrent/add_torrent_params.hpp").exists() {
                    found_header = true;
                    build.include(&include);
                }

                let lib = prefix.join("lib");
                if lib.exists() {
                    println!("cargo:rustc-link-search=native={}", lib.display());
                }
            }

            if !found_header {
                panic!(
                    "native-libtorrent requires libtorrent-rasterbar headers. Install libtorrent-rasterbar, or set PKG_CONFIG_PATH / LIBTORRENT_RASTERBAR_ROOT so libtorrent/add_torrent_params.hpp can be found."
                );
            }

            println!("cargo:rustc-link-lib=torrent-rasterbar");
        }
    }

    build.compile("paradown_libtorrent_bridge");
}

#[cfg(feature = "native-libtorrent")]
fn candidate_prefixes() -> Vec<PathBuf> {
    let mut prefixes = Vec::new();

    if let Some(root) = env::var_os("LIBTORRENT_RASTERBAR_ROOT") {
        prefixes.push(PathBuf::from(root));
    }

    if let Some(homebrew_prefix) = env::var_os("HOMEBREW_PREFIX") {
        let homebrew_prefix = PathBuf::from(homebrew_prefix);
        prefixes.push(homebrew_prefix.join("opt/libtorrent-rasterbar"));
        prefixes.push(homebrew_prefix);
    }

    prefixes.extend([
        PathBuf::from("/opt/homebrew/opt/libtorrent-rasterbar"),
        PathBuf::from("/usr/local/opt/libtorrent-rasterbar"),
        PathBuf::from("/opt/homebrew"),
        PathBuf::from("/usr/local"),
        PathBuf::from("/usr"),
        PathBuf::from("/opt/local"),
    ]);

    prefixes
}

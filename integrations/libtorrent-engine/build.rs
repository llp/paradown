// `#[cfg(...)]` 是条件编译属性。
// 当 `native-libtorrent` feature 没有开启时，下面这些 import 不会参与编译。
// 宏和条件编译的系统解释见 `docs/rust/macros-and-attributes.md`。
#[cfg(feature = "native-libtorrent")]
use std::env;
#[cfg(feature = "native-libtorrent")]
use std::path::PathBuf;

// `not(feature = "...")` 表示 feature 未开启。
// build script 仍然需要有一个 main，但这里只告诉 Cargo：build.rs 变化时重新运行。
#[cfg(not(feature = "native-libtorrent"))]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}

// feature 开启时编译并链接 C++ native bridge。
#[cfg(feature = "native-libtorrent")]
fn main() {
    // build script 通过 stdout 和 Cargo 通信。
    // `cargo:rerun-if-changed=...` 不是普通日志，而是告诉 Cargo 这些文件变化时重跑 build.rs。
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=src/native_engine.cc");
    println!("cargo:rerun-if-changed=include/paradown_libtorrent/native_engine.hpp");

    // `cxx_build::bridge("src/ffi.rs")` 会读取 cxx::bridge 模块并生成 C++ 桥接代码。
    let mut build = cxx_build::bridge("src/ffi.rs");
    build
        .file("src/native_engine.cc")
        .include("include")
        .flag_if_supported("-std=c++17");

    let candidate_prefixes = candidate_prefixes();
    for include in native_include_dirs(&candidate_prefixes) {
        build.include(include);
    }

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
            for prefix in candidate_prefixes {
                let include = prefix.join("include");
                if include.join("libtorrent/add_torrent_params.hpp").exists() {
                    found_header = true;
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

    if let Some(root) = env::var_os("BOOST_ROOT") {
        prefixes.push(PathBuf::from(root));
    }

    if let Some(homebrew_prefix) = env::var_os("HOMEBREW_PREFIX") {
        let homebrew_prefix = PathBuf::from(homebrew_prefix);
        prefixes.push(homebrew_prefix.join("opt/libtorrent-rasterbar"));
        prefixes.push(homebrew_prefix.join("opt/boost"));
        prefixes.push(homebrew_prefix);
    }

    prefixes.extend([
        PathBuf::from("/opt/homebrew/opt/libtorrent-rasterbar"),
        PathBuf::from("/opt/homebrew/opt/boost"),
        PathBuf::from("/usr/local/opt/libtorrent-rasterbar"),
        PathBuf::from("/usr/local/opt/boost"),
        PathBuf::from("/opt/homebrew"),
        PathBuf::from("/usr/local"),
        PathBuf::from("/usr"),
        PathBuf::from("/opt/local"),
    ]);

    prefixes
}

#[cfg(feature = "native-libtorrent")]
fn native_include_dirs(prefixes: &[PathBuf]) -> Vec<PathBuf> {
    let mut includes = Vec::new();

    if let Some(include_dir) = env::var_os("BOOST_INCLUDEDIR") {
        includes.push(PathBuf::from(include_dir));
    }

    for prefix in prefixes {
        let include = prefix.join("include");
        if include.join("libtorrent/add_torrent_params.hpp").exists()
            || include.join("boost/config.hpp").exists()
        {
            includes.push(include);
        }
    }

    includes
}

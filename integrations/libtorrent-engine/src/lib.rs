#[cfg(feature = "native-libtorrent")]
mod ffi;
#[cfg(feature = "native-libtorrent")]
mod native;
#[cfg(not(feature = "native-libtorrent"))]
mod stub;

#[cfg(feature = "native-libtorrent")]
pub use native::LibtorrentRasterbarEngine;
#[cfg(not(feature = "native-libtorrent"))]
pub use stub::LibtorrentRasterbarEngine;

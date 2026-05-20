#pragma once

#include <cstdint>
#include <memory>

#include "rust/cxx.h"

namespace paradown_libtorrent {

struct NativeEngineConfig;
struct NativeEngineEvent;
struct NativeStartResult;

class NativeEngine {
public:
  struct Impl;

  explicit NativeEngine(NativeEngineConfig const& config);
  ~NativeEngine();

  std::unique_ptr<Impl> impl;
};

std::unique_ptr<NativeEngine> new_native_engine(NativeEngineConfig config);

NativeStartResult add_magnet(NativeEngine& engine,
                             rust::Str uri,
                             rust::Str save_path,
                             rust::Slice<const std::uint8_t> resume_data);

NativeStartResult add_torrent_file(NativeEngine& engine,
                                   rust::Str path,
                                   rust::Str save_path,
                                   rust::Slice<const std::uint8_t> resume_data);

rust::Vec<NativeEngineEvent> poll_alerts(NativeEngine& engine);

std::uint16_t listen_port(NativeEngine& engine);
void connect_peer(NativeEngine& engine,
                  rust::Str external_id,
                  rust::Str host,
                  std::uint16_t port);
void add_tracker(NativeEngine& engine, rust::Str external_id, rust::Str url);
void add_url_seed(NativeEngine& engine, rust::Str external_id, rust::Str url);
void pause_torrent(NativeEngine& engine, rust::Str external_id);
void resume_torrent(NativeEngine& engine, rust::Str external_id);
void remove_torrent(NativeEngine& engine, rust::Str external_id, bool delete_payload);
void save_resume_data(NativeEngine& engine, rust::Str external_id);

}  // namespace paradown_libtorrent

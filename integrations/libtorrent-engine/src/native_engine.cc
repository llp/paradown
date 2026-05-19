#include "paradown_libtorrent/native_engine.hpp"

#include <algorithm>
#include <cstdint>
#include <iomanip>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <string>
#include <unordered_map>
#include <vector>

#include "libtorrent/add_torrent_params.hpp"
#include "libtorrent/alert_types.hpp"
#include "libtorrent/address.hpp"
#include "libtorrent/bdecode.hpp"
#include "libtorrent/error_code.hpp"
#include "libtorrent/file_storage.hpp"
#include "libtorrent/magnet_uri.hpp"
#include "libtorrent/read_resume_data.hpp"
#include "libtorrent/session.hpp"
#include "libtorrent/session_params.hpp"
#include "libtorrent/settings_pack.hpp"
#include "libtorrent/torrent_handle.hpp"
#include "libtorrent/torrent_info.hpp"
#include "libtorrent/write_resume_data.hpp"

#include "paradown-libtorrent-engine/src/ffi.rs.h"

namespace lt = libtorrent;

namespace paradown_libtorrent {
namespace {

constexpr std::uint8_t EVENT_METADATA = 1;
constexpr std::uint8_t EVENT_STATE = 2;
constexpr std::uint8_t EVENT_PROGRESS = 3;
constexpr std::uint8_t EVENT_PIECE_FINISHED = 4;
constexpr std::uint8_t EVENT_RESUME_DATA = 5;
constexpr std::uint8_t EVENT_FINISHED = 6;
constexpr std::uint8_t EVENT_ERROR = 7;

constexpr std::uint8_t STATE_RESOLVING_METADATA = 1;
constexpr std::uint8_t STATE_CHECKING_FILES = 2;
constexpr std::uint8_t STATE_DOWNLOADING = 3;
constexpr std::uint8_t STATE_SEEDING = 4;
constexpr std::uint8_t STATE_PAUSED = 5;
constexpr std::uint8_t STATE_COMPLETED = 6;

std::string to_string(rust::Str value) {
    return std::string(value.data(), value.size());
}

std::string hex_bytes(char const* data, std::size_t size) {
    std::ostringstream out;
    out << std::hex << std::setfill('0');
    for (std::size_t i = 0; i < size; ++i) {
        out << std::setw(2) << (static_cast<unsigned int>(static_cast<unsigned char>(data[i])));
    }
    return out.str();
}

std::string hash_to_hex(lt::sha1_hash const& hash) {
    return hex_bytes(hash.data(), hash.size());
}

std::string hash_to_hex(lt::sha256_hash const& hash) {
    return hex_bytes(hash.data(), hash.size());
}

std::string external_id(lt::info_hash_t const& hashes) {
    if (hashes.has_v2()) {
        return hash_to_hex(hashes.v2);
    }
    if (hashes.has_v1()) {
        return hash_to_hex(hashes.v1);
    }
    return "";
}

std::string external_id(lt::torrent_handle const& handle) {
    return external_id(handle.info_hashes());
}

rust::String rust_string(std::string const& value) {
    rust::String out;
    out = value;
    return out;
}

rust::Vec<std::uint8_t> bytes_to_rust(std::vector<char> const& bytes) {
    rust::Vec<std::uint8_t> out;
    out.reserve(bytes.size());
    for (char byte : bytes) {
        out.push_back(static_cast<std::uint8_t>(byte));
    }
    return out;
}

std::vector<char> bytes_from_rust(rust::Slice<const std::uint8_t> bytes) {
    std::vector<char> out;
    out.reserve(bytes.size());
    for (std::uint8_t byte : bytes) {
        out.push_back(static_cast<char>(byte));
    }
    return out;
}

std::vector<std::string> split_path(std::string path) {
    std::replace(path.begin(), path.end(), '\\', '/');
    std::vector<std::string> components;
    std::stringstream stream(path);
    std::string component;
    while (std::getline(stream, component, '/')) {
        if (!component.empty() && component != "." && component != "..") {
            components.push_back(component);
        }
    }
    if (components.empty()) {
        components.push_back("payload.bin");
    }
    return components;
}

NativeTorrentFileEntry map_file(lt::file_storage const& files, lt::file_index_t index) {
    NativeTorrentFileEntry out;
    for (auto const& component : split_path(files.file_path(index))) {
        out.path_components.push_back(rust_string(component));
    }
    out.length = static_cast<std::uint64_t>(files.file_size(index));
    out.offset = static_cast<std::uint64_t>(files.file_offset(index));
    return out;
}

NativeTorrentMetadata empty_metadata() {
    NativeTorrentMetadata out;
    out.name = rust_string("");
    out.has_info_hash_v1 = false;
    out.info_hash_v1 = rust_string("");
    out.has_info_hash_v2 = false;
    out.info_hash_v2 = rust_string("");
    out.piece_size = 0;
    out.piece_count = 0;
    out.total_size = 0;
    out.private_torrent = false;
    return out;
}

NativeTorrentMetadata map_metadata(lt::torrent_info const& info) {
    NativeTorrentMetadata out = empty_metadata();
    out.name = rust_string(info.name());
    auto hashes = info.info_hashes();
    out.has_info_hash_v1 = hashes.has_v1();
    out.info_hash_v1 = rust_string(out.has_info_hash_v1 ? hash_to_hex(hashes.v1) : "");
    out.has_info_hash_v2 = hashes.has_v2();
    out.info_hash_v2 = rust_string(out.has_info_hash_v2 ? hash_to_hex(hashes.v2) : "");
    out.piece_size = static_cast<std::uint32_t>(std::max(1, info.piece_length()));
    out.piece_count = static_cast<std::uint32_t>(std::max(0, info.num_pieces()));
    out.total_size = static_cast<std::uint64_t>(std::max<std::int64_t>(0, info.total_size()));
    out.private_torrent = info.priv();

    auto const& files = info.files();
    for (lt::file_index_t i(0); i < files.end_file(); ++i) {
        out.files.push_back(map_file(files, i));
    }

    if (info.v1()) {
        for (lt::piece_index_t i(0); i < info.end_piece(); ++i) {
            NativeTorrentPieceHash mapped;
            mapped.piece_index = static_cast<std::uint32_t>(static_cast<int>(i));
            mapped.sha1 = rust_string(hash_to_hex(info.hash_for_piece(i)));
            mapped.sha256 = rust_string("");
            out.piece_hashes.push_back(mapped);
        }
    }

    for (auto const& tracker : info.trackers()) {
        NativeTorrentTracker mapped;
        mapped.url = rust_string(tracker.url);
        mapped.tier = static_cast<std::uint32_t>(tracker.tier);
        out.trackers.push_back(mapped);
    }

    for (auto const& seed : info.web_seeds()) {
        out.web_seeds.push_back(rust_string(seed.url));
    }

    return out;
}

std::uint8_t map_state(lt::torrent_status const& status) {
    if (bool(status.flags & lt::torrent_flags::paused)) {
        return STATE_PAUSED;
    }

    switch (status.state) {
        case lt::torrent_status::checking_files:
        case lt::torrent_status::checking_resume_data:
            return STATE_CHECKING_FILES;
        case lt::torrent_status::downloading_metadata:
            return STATE_RESOLVING_METADATA;
        case lt::torrent_status::downloading:
            return STATE_DOWNLOADING;
        case lt::torrent_status::finished:
            return STATE_COMPLETED;
        case lt::torrent_status::seeding:
            return STATE_SEEDING;
        default:
            return STATE_DOWNLOADING;
    }
}

NativeEngineEvent event_base(std::uint8_t kind, std::string const& id) {
    NativeEngineEvent event;
    event.kind = kind;
    event.external_id = rust_string(id);
    event.state = 0;
    event.message = rust_string("");
    event.piece_index = 0;
    event.downloaded = 0;
    event.total = 0;
    event.download_rate_bps = 0;
    event.upload_rate_bps = 0;
    event.connected_peers = 0;
    event.seeds = 0;
    event.has_metadata = false;
    event.metadata = empty_metadata();
    return event;
}

NativeEngineEvent error_event(std::string const& id, std::string const& message) {
    NativeEngineEvent event = event_base(EVENT_ERROR, id);
    event.message = rust_string(message);
    return event;
}

void set_add_params_common(lt::add_torrent_params& params, std::string const& save_path) {
    params.save_path = save_path;
    params.flags |= lt::torrent_flags::auto_managed;
    params.flags &= ~lt::torrent_flags::paused;
}

lt::add_torrent_params params_from_resume(rust::Slice<const std::uint8_t> resume_data) {
    auto bytes = bytes_from_rust(resume_data);
    lt::error_code ec;
    auto params = lt::read_resume_data(bytes, ec);
    if (ec) {
        throw std::runtime_error("failed to read libtorrent resume data: " + ec.message());
    }
    return params;
}

}  // namespace

struct NativeEngine::Impl {
    explicit Impl(NativeEngineConfig const& config) {
        lt::settings_pack settings;
        settings.set_int(
            lt::settings_pack::alert_mask,
            lt::alert_category::error | lt::alert_category::status |
                lt::alert_category::storage | lt::alert_category::tracker |
                lt::alert_category::dht | lt::alert_category::piece_progress |
                lt::alert_category::file_progress);
        settings.set_bool(lt::settings_pack::enable_dht, config.enable_dht);
        settings.set_bool(lt::settings_pack::enable_lsd, config.enable_lsd);
        settings.set_bool(lt::settings_pack::enable_upnp, config.enable_upnp);
        settings.set_bool(lt::settings_pack::enable_natpmp, config.enable_natpmp);
        if (config.has_listen_interfaces) {
            settings.set_str(lt::settings_pack::listen_interfaces, std::string(config.listen_interfaces));
        }
        session = std::make_unique<lt::session>(settings);
    }

    std::unique_ptr<lt::session> session;
    std::unordered_map<std::string, lt::torrent_handle> handles;
};

lt::torrent_handle add_torrent(NativeEngine& engine, lt::add_torrent_params params) {
    lt::error_code ec;
    auto handle = (*engine.impl->session).add_torrent(std::move(params), ec);
    if (ec) {
        throw std::runtime_error("failed to add torrent: " + ec.message());
    }
    auto id = external_id(handle);
    if (!id.empty()) {
        engine.impl->handles[id] = handle;
    }
    return handle;
}

NativeEngine::NativeEngine(NativeEngineConfig const& config)
    : impl(std::make_unique<NativeEngine::Impl>(config)) {}

NativeEngine::~NativeEngine() = default;

std::unique_ptr<NativeEngine> new_native_engine(NativeEngineConfig config) {
    return std::make_unique<NativeEngine>(config);
}

NativeStartResult add_magnet(NativeEngine& engine,
                             rust::Str uri,
                             rust::Str save_path,
                             rust::Slice<const std::uint8_t> resume_data) {
    lt::add_torrent_params params =
        resume_data.empty() ? lt::parse_magnet_uri(to_string(uri)) : params_from_resume(resume_data);
    set_add_params_common(params, to_string(save_path));
    auto handle = add_torrent(engine, std::move(params));
    auto id = external_id(handle);

    NativeStartResult result;
    result.external_id = rust_string(id);
    result.has_metadata = false;
    result.metadata = empty_metadata();
    return result;
}

NativeStartResult add_torrent_file(NativeEngine& engine,
                                   rust::Str path,
                                   rust::Str save_path,
                                   rust::Slice<const std::uint8_t> resume_data) {
    std::shared_ptr<lt::torrent_info> info;
    lt::add_torrent_params params;
    if (resume_data.empty()) {
        info = std::make_shared<lt::torrent_info>(to_string(path));
        params.ti = info;
        params.info_hashes = info->info_hashes();
    } else {
        params = params_from_resume(resume_data);
        info = params.ti;
    }
    set_add_params_common(params, to_string(save_path));
    NativeTorrentMetadata metadata = info ? map_metadata(*info) : empty_metadata();
    auto handle = add_torrent(engine, std::move(params));
    auto id = external_id(handle);

    NativeStartResult result;
    result.external_id = rust_string(id);
    result.has_metadata = info != nullptr;
    result.metadata = metadata;
    return result;
}

rust::Vec<NativeEngineEvent> poll_alerts(NativeEngine& engine) {
    rust::Vec<NativeEngineEvent> events;
    (*engine.impl->session).post_torrent_updates(
        lt::torrent_handle::query_accurate_download_counters |
        lt::torrent_handle::query_name | lt::torrent_handle::query_save_path);

    std::vector<lt::alert*> alerts;
    (*engine.impl->session).pop_alerts(&alerts);
    for (lt::alert* alert : alerts) {
        if (auto* add = lt::alert_cast<lt::add_torrent_alert>(alert)) {
            auto id = external_id(add->handle);
            if (add->error) {
                events.push_back(error_event(id, add->error.message()));
                continue;
            }
            engine.impl->handles[id] = add->handle;
            NativeEngineEvent event = event_base(EVENT_STATE, id);
            event.state = STATE_RESOLVING_METADATA;
            events.push_back(event);
        } else if (auto* metadata = lt::alert_cast<lt::metadata_received_alert>(alert)) {
            auto id = external_id(metadata->handle);
            NativeEngineEvent event = event_base(EVENT_METADATA, id);
            if (auto info = metadata->handle.torrent_file()) {
                event.has_metadata = true;
                event.metadata = map_metadata(*info);
            }
            events.push_back(event);
        } else if (auto* piece = lt::alert_cast<lt::piece_finished_alert>(alert)) {
            auto id = external_id(piece->handle);
            NativeEngineEvent event = event_base(EVENT_PIECE_FINISHED, id);
            event.piece_index = static_cast<std::uint32_t>(piece->piece_index);
            events.push_back(event);
        } else if (auto* finished = lt::alert_cast<lt::torrent_finished_alert>(alert)) {
            events.push_back(event_base(EVENT_FINISHED, external_id(finished->handle)));
        } else if (auto* error = lt::alert_cast<lt::torrent_error_alert>(alert)) {
            events.push_back(error_event(external_id(error->handle), error->message()));
        } else if (auto* save = lt::alert_cast<lt::save_resume_data_alert>(alert)) {
            auto id = external_id(save->handle);
            NativeEngineEvent event = event_base(EVENT_RESUME_DATA, id);
            event.resume_data = bytes_to_rust(lt::write_resume_data_buf(save->params));
            events.push_back(event);
        } else if (auto* save_failed = lt::alert_cast<lt::save_resume_data_failed_alert>(alert)) {
            events.push_back(error_event(external_id(save_failed->handle), save_failed->message()));
        } else if (auto* state = lt::alert_cast<lt::state_update_alert>(alert)) {
            for (auto const& status : state->status) {
                auto id = external_id(status.handle);
                NativeEngineEvent state_event = event_base(EVENT_STATE, id);
                state_event.state = map_state(status);
                events.push_back(state_event);

                NativeEngineEvent progress = event_base(EVENT_PROGRESS, id);
                progress.downloaded = static_cast<std::uint64_t>(std::max<std::int64_t>(0, status.total_wanted_done));
                progress.total = static_cast<std::uint64_t>(std::max<std::int64_t>(0, status.total_wanted));
                progress.download_rate_bps = static_cast<std::uint64_t>(std::max(0, status.download_rate));
                progress.upload_rate_bps = static_cast<std::uint64_t>(std::max(0, status.upload_rate));
                progress.connected_peers = static_cast<std::uint32_t>(std::max(0, status.num_peers));
                progress.seeds = static_cast<std::uint32_t>(std::max(0, status.num_seeds));
                events.push_back(progress);
            }
        }
    }
    return events;
}

std::uint16_t listen_port(NativeEngine& engine) {
    return (*engine.impl->session).listen_port();
}

void connect_peer(NativeEngine& engine,
                  rust::Str external_id,
                  rust::Str host,
                  std::uint16_t port) {
    auto it = engine.impl->handles.find(to_string(external_id));
    if (it == engine.impl->handles.end()) {
        throw std::runtime_error("unknown torrent handle");
    }

    lt::error_code ec;
    auto address = lt::make_address(to_string(host), ec);
    if (ec) {
        throw std::runtime_error("invalid peer host: " + ec.message());
    }
    it->second.connect_peer(lt::tcp::endpoint(address, port));
}

void pause_torrent(NativeEngine& engine, rust::Str external_id) {
    auto it = engine.impl->handles.find(to_string(external_id));
    if (it == engine.impl->handles.end()) {
        throw std::runtime_error("unknown torrent handle");
    }
    it->second.pause();
    it->second.save_resume_data(lt::torrent_handle::save_info_dict);
}

void resume_torrent(NativeEngine& engine, rust::Str external_id) {
    auto it = engine.impl->handles.find(to_string(external_id));
    if (it == engine.impl->handles.end()) {
        throw std::runtime_error("unknown torrent handle");
    }
    it->second.resume();
}

void remove_torrent(NativeEngine& engine, rust::Str external_id, bool delete_payload) {
    auto key = to_string(external_id);
    auto it = engine.impl->handles.find(key);
    if (it == engine.impl->handles.end()) {
        return;
    }
    auto flags = delete_payload ? lt::session::delete_files : lt::remove_flags_t{};
    (*engine.impl->session).remove_torrent(it->second, flags);
    engine.impl->handles.erase(it);
}

void save_resume_data(NativeEngine& engine, rust::Str external_id) {
    auto it = engine.impl->handles.find(to_string(external_id));
    if (it == engine.impl->handles.end()) {
        throw std::runtime_error("unknown torrent handle");
    }
    it->second.save_resume_data(lt::torrent_handle::save_info_dict);
}

}  // namespace paradown_libtorrent

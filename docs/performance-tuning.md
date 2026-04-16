# Performance Tuning

`paradown` currently targets production `HTTP/HTTPS` downloads. The defaults are conservative and should work well on developer machines, but sustained download hosts usually need a few knobs tuned together.

## Recommended starting points

- `concurrent_tasks = 2-4` for desktop usage
- `segments_per_task = 4-8` for large artifacts on range-capable origins
- `progress_throttle.interval_ms = 200-500` and `progress_throttle.threshold_bytes = 1-8 MiB` when SQLite persistence is enabled
- `rate_limit_kbps` unset unless the downloader is sharing bandwidth with other production traffic

## Concurrency guidance

- Increase `concurrent_tasks` only when the host has spare network and disk bandwidth.
- Increase `segments_per_task` only for large files served by origins that support `Range` well.
- For smaller files or slow origins, extra segments can add overhead without improving throughput.

## Persistence guidance

- Prefer the SQLite backend for long-lived task history and crash recovery.
- The runtime now throttles progress persistence using `progress_throttle`; if the database becomes hot, raise the byte threshold before raising the interval.
- Keep the download directory and SQLite database on a local disk whenever possible.

## Retry and resilience

- Leave `retry.max_retries` at `3` for normal downloads.
- Raise it for flaky long-haul origins only when the server is known to recover.
- Lower it in CI or scripted environments when fast failure is more important than self-healing.

## Memory profiling

Use the helper script to capture a platform-native memory report:

```bash
./scripts/profile-memory.sh \
  --download-dir /tmp/paradown-mem \
  --url https://example.com/large.iso
```

## Soak testing

Use the soak helper to run repeated real downloads for a fixed duration:

```bash
./scripts/soak-http.sh \
  --download-dir /tmp/paradown-soak \
  --urls-file ./examples/urls.txt \
  --duration-minutes 30 \
  -- --max-concurrent 2 --workers 4
```

## Benchmarking

Run the Criterion benchmarks for the local HTTP download path:

```bash
cargo bench --bench download_bench
```

The current bench suite covers:

- single-asset HTTP download on the in-memory backend
- multi-task HTTP download with SQLite persistence enabled

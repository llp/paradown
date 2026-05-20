use crate::domain::DownloadSpec;
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use crate::job::finalize::finish_job;
use crate::job::prepare::PreparationOutcome;
use crate::p2p::{
    TorrentDiagnosticEvent, TorrentDiagnosticScope, TorrentDiagnosticSeverity, TorrentEngineEvent,
    TorrentEngineRequest, TorrentEngineState, TorrentSwarmHints, TorrentSwarmProviderCandidateKind,
    TorrentSwarmProviderDiagnostic, TorrentSwarmProviderSeverity, TorrentTransferStats,
    build_swarm_provider_resolver, manifest_from_torrent_metadata,
};
use log::{debug, warn};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::fs;
use tokio::sync::mpsc;

pub(crate) async fn prepare_swarm_download(job: &Arc<Task>) -> Result<PreparationOutcome, Error> {
    let manager = job
        .manager
        .upgrade()
        .ok_or_else(|| Error::Other("task is not attached to a manager".into()))?;

    let download_dir = &job.config.download_dir;
    if !download_dir.exists() {
        fs::create_dir_all(download_dir)
            .await
            .map_err(|e| Error::Io(format!("Failed to create download directory: {}", e)))?;
    }

    let requested_file_path = job.file_path.get().cloned();
    let resume = job.torrent_resume_snapshot().await;
    let mut source_set = job.source_set_snapshot().await;
    let initial_hints = TorrentSwarmHints::from_spec_and_sources(&job.spec, &source_set)?;
    let mut engine_spec = job.spec.clone();
    let swarm_hints = match build_swarm_provider_resolver(&job.config.p2p.swarm, download_dir) {
        Ok(Some(resolver)) => {
            let resolution = resolver.resolve(job.spec.clone(), initial_hints).await;
            for candidate in &resolution.candidates {
                if let Some(source) = candidate.source_descriptor() {
                    source_set.push_unique(source);
                }
                if matches!(
                    candidate.kind,
                    TorrentSwarmProviderCandidateKind::TorrentFile
                ) && matches!(
                    &job.spec,
                    DownloadSpec::Magnet { .. } | DownloadSpec::Metadata { .. }
                ) && matches!(
                    &engine_spec,
                    DownloadSpec::Magnet { .. } | DownloadSpec::Metadata { .. }
                ) && let Ok(DownloadSpec::TorrentFile { path }) =
                    DownloadSpec::parse(candidate.locator.clone())
                {
                    engine_spec = DownloadSpec::TorrentFile { path };
                }
            }
            for diagnostic in &resolution.diagnostics {
                job.record_torrent_diagnostic(provider_diagnostic_to_torrent(diagnostic))
                    .await;
            }
            if !resolution.candidates.is_empty() {
                job.set_source_set(source_set.clone()).await;
            }
            resolution.hints
        }
        Ok(None) => initial_hints,
        Err(err) => {
            job.record_torrent_diagnostic(TorrentDiagnosticEvent {
                scope: TorrentDiagnosticScope::Session,
                severity: TorrentDiagnosticSeverity::Warning,
                message: format!("swarm provider setup failed: {err}"),
                url: None,
                endpoint: None,
                peers: None,
            })
            .await;
            initial_hints
        }
    };
    let (event_sender, event_receiver) = mpsc::unbounded_channel();
    let request = TorrentEngineRequest {
        session_id: job.id,
        spec: engine_spec.clone(),
        download_dir: download_dir.clone(),
        requested_file_name: job.file_name.get().cloned(),
        requested_file_path: requested_file_path.clone(),
        rate_limit_kib_per_sec: job.config.rate_limit_kib_per_sec.map(u64::from),
        swarm_hints: swarm_hints.clone(),
        resume: resume.clone(),
        event_sender: Some(event_sender.clone()),
    };

    let engine_session = match manager.torrent_engine.start_session(request).await {
        Ok(session) => session,
        Err(err) if engine_spec != job.spec => {
            job.record_torrent_diagnostic(TorrentDiagnosticEvent {
                scope: TorrentDiagnosticScope::Session,
                severity: TorrentDiagnosticSeverity::Warning,
                message: format!(
                    "provider torrent metadata candidate failed; falling back to original spec: {err}"
                ),
                url: Some(engine_spec.locator().to_string()),
                endpoint: None,
                peers: None,
            })
            .await;
            manager
                .torrent_engine
                .start_session(TorrentEngineRequest {
                    session_id: job.id,
                    spec: job.spec.clone(),
                    download_dir: download_dir.clone(),
                    requested_file_name: job.file_name.get().cloned(),
                    requested_file_path: requested_file_path.clone(),
                    rate_limit_kib_per_sec: job.config.rate_limit_kib_per_sec.map(u64::from),
                    swarm_hints,
                    resume: resume.clone(),
                    event_sender: Some(event_sender),
                })
                .await?
        }
        Err(err) => return Err(err),
    };

    if let Some(metadata) = engine_session.metadata.as_ref().or_else(|| {
        resume
            .as_ref()
            .and_then(|snapshot| snapshot.metadata.as_ref())
    }) {
        install_torrent_metadata(job, metadata, requested_file_path.as_deref()).await?;
    } else if let Some(manifest) = engine_session.manifest.clone() {
        job.update_protocol_probe(Some(manifest.total_size), true);
        job.install_manifest(manifest).await;
    }

    job.set_torrent_session(engine_session).await;
    job.persist_task().await?;

    Ok(PreparationOutcome::StartedByEngine(event_receiver))
}

fn provider_diagnostic_to_torrent(
    diagnostic: &TorrentSwarmProviderDiagnostic,
) -> TorrentDiagnosticEvent {
    TorrentDiagnosticEvent {
        scope: TorrentDiagnosticScope::Session,
        severity: match diagnostic.severity {
            TorrentSwarmProviderSeverity::Info => TorrentDiagnosticSeverity::Info,
            TorrentSwarmProviderSeverity::Warning => TorrentDiagnosticSeverity::Warning,
            TorrentSwarmProviderSeverity::Error => TorrentDiagnosticSeverity::Error,
        },
        message: format!(
            "swarm provider {}: {}",
            diagnostic.provider, diagnostic.message
        ),
        url: diagnostic.source.clone(),
        endpoint: None,
        peers: None,
    }
}

pub(crate) fn spawn_torrent_event_listener(
    job: &Arc<Task>,
    mut event_receiver: mpsc::UnboundedReceiver<TorrentEngineEvent>,
) {
    let job = Arc::clone(job);
    tokio::spawn(async move {
        while let Some(event) = event_receiver.recv().await {
            if let Err(err) = handle_torrent_engine_event(&job, event).await {
                warn!(
                    "[Task {}] Failed to handle torrent engine event: {:?}",
                    job.id, err
                );
            }
        }
    });
}

async fn handle_torrent_engine_event(
    job: &Arc<Task>,
    event: TorrentEngineEvent,
) -> Result<(), Error> {
    match event {
        TorrentEngineEvent::MetadataDiscovered(metadata) => {
            debug!("[Task {}] Torrent metadata discovered", job.id);
            job.record_torrent_metadata(metadata.clone()).await;
            install_torrent_metadata(job, &metadata, job.file_path.get().map(PathBuf::as_path))
                .await?;
            job.persist_task().await?;
        }
        TorrentEngineEvent::StateChanged(state) => {
            job.record_torrent_state(state.clone()).await;
            if matches!(
                state,
                TorrentEngineState::Completed | TorrentEngineState::Seeding
            ) {
                finish_job(job, Ok(())).await?;
                job.release_permit().await;
            } else if matches!(state, TorrentEngineState::Paused) {
                job.set_status(crate::Status::Paused).await;
                job.emit_manager_event(Event::Pause(job.id));
            }
        }
        TorrentEngineEvent::Progress {
            downloaded,
            total,
            download_rate_bps,
            upload_rate_bps,
            connected_peers,
            seeds,
        } => {
            job.record_torrent_transfer(TorrentTransferStats {
                downloaded,
                total,
                download_rate_bps,
                upload_rate_bps,
                connected_peers,
                seeds,
            })
            .await;
            job.downloaded_size.store(downloaded, Ordering::Relaxed);
            job.total_size.store(total, Ordering::Relaxed);
            job.total_size_known.store(true, Ordering::Relaxed);
            job.emit_manager_event(Event::Progress {
                id: job.id,
                downloaded,
                total,
            });
        }
        TorrentEngineEvent::PieceFinished { piece_index } => {
            {
                let mut piece_states = job.piece_states.write().await;
                if let Some(piece) = piece_states
                    .iter_mut()
                    .find(|piece| piece.piece_index == piece_index)
                {
                    piece.completed = true;
                }
            }
            job.persist_task().await?;
        }
        TorrentEngineEvent::ResumeData { bytes } => {
            job.record_torrent_resume_data(bytes).await;
            job.persist_task().await?;
        }
        TorrentEngineEvent::Diagnostic(diagnostic) => {
            let should_retain = should_retain_torrent_diagnostic(&diagnostic);
            let should_emit = should_emit_torrent_diagnostic(&diagnostic);
            if should_retain {
                job.record_torrent_diagnostic(diagnostic.clone()).await;
            }
            if should_emit {
                job.emit_manager_event(Event::TorrentDiagnostic {
                    id: job.id,
                    diagnostic,
                });
            }
        }
        TorrentEngineEvent::Finished => {
            job.record_torrent_state(TorrentEngineState::Completed)
                .await;
            finish_job(job, Ok(())).await?;
            job.release_permit().await;
        }
        TorrentEngineEvent::Error(message) => {
            job.record_torrent_diagnostic(crate::p2p::TorrentDiagnosticEvent {
                scope: crate::p2p::TorrentDiagnosticScope::Session,
                severity: crate::p2p::TorrentDiagnosticSeverity::Error,
                message: message.clone(),
                url: None,
                endpoint: None,
                peers: None,
            })
            .await;
            let err = Error::Other(message);
            finish_job(job, Err(err)).await?;
            job.release_permit().await;
        }
    }

    Ok(())
}

fn should_retain_torrent_diagnostic(diagnostic: &TorrentDiagnosticEvent) -> bool {
    if diagnostic.peers.is_some_and(|peers| peers > 0) {
        return true;
    }
    if is_noisy_torrent_diagnostic(diagnostic) {
        return false;
    }
    !matches!(diagnostic.severity, TorrentDiagnosticSeverity::Info)
}

fn should_emit_torrent_diagnostic(diagnostic: &TorrentDiagnosticEvent) -> bool {
    should_retain_torrent_diagnostic(diagnostic)
        && (matches!(
            diagnostic.severity,
            TorrentDiagnosticSeverity::Warning | TorrentDiagnosticSeverity::Error
        ) || (diagnostic.scope == TorrentDiagnosticScope::Tracker
            && diagnostic.peers.is_some_and(|peers| peers > 0)))
}

fn is_noisy_torrent_diagnostic(diagnostic: &TorrentDiagnosticEvent) -> bool {
    if matches!(
        diagnostic.scope,
        TorrentDiagnosticScope::PortMapping
            if !matches!(diagnostic.severity, TorrentDiagnosticSeverity::Error)
    ) {
        return true;
    }

    if diagnostic.scope != TorrentDiagnosticScope::Tracker {
        return diagnostic.scope == TorrentDiagnosticScope::Peer
            && is_noisy_peer_message(diagnostic);
    }

    let message = diagnostic.message.to_ascii_lowercase();
    let endpoint = diagnostic.endpoint.as_deref().unwrap_or_default();
    endpoint.starts_with("127.")
        || endpoint.starts_with("[::1]")
        || endpoint.starts_with("::1")
        || message.contains("sending announce")
        || message.contains("skipping tracker announce")
        || message.contains("timed out")
        || message.contains("end of file")
}

fn is_noisy_peer_message(diagnostic: &TorrentDiagnosticEvent) -> bool {
    let message = diagnostic.message.to_ascii_lowercase();
    message.contains("disconnecting")
        || message.contains("end of file")
        || message.contains("connection reset")
        || message.contains("redirecting")
        || message.contains("404 not found")
}

async fn install_torrent_metadata(
    job: &Arc<Task>,
    metadata: &crate::p2p::TorrentMetadata,
    requested_file_path: Option<&std::path::Path>,
) -> Result<(), Error> {
    let source_set = job.source_set_snapshot().await;
    let manifest = manifest_from_torrent_metadata(
        job.spec.clone(),
        source_set,
        metadata,
        &job.config.download_dir,
        requested_file_path,
    )?;
    let total_size = manifest.total_size;
    let primary_file_path = manifest.files.first().map(|file| file.path.clone());

    if let Some(file_path) = primary_file_path
        && job.file_path.get().is_none()
    {
        let _ = job.file_path.set(file_path);
    }

    job.update_protocol_probe(Some(total_size), true);
    job.install_manifest(manifest).await;
    Ok(())
}

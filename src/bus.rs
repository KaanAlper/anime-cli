use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::config::{HistoryEntry, save_history_entry};
use crate::download::{DownloadJob, DownloadManager};

#[derive(Debug, Clone)]
pub enum AppEvent {
    PlaybackStarted,
    PlaybackEnded {
        history: Option<HistoryEntry>,
    },
    EnqueueDownload(DownloadJob),
    CacheCleared,
}

#[derive(Debug, Default, Clone)]
pub struct AppBusStatus {
    pub queued_downloads: usize,
    pub active_download: bool,
    pub download_paused: bool,
}

struct EventBus {
    tx: Sender<AppEvent>,
    status: Arc<Mutex<AppBusStatus>>,
}

static BUS: OnceLock<EventBus> = OnceLock::new();

fn init_bus() -> &'static EventBus {
    BUS.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<AppEvent>();
        let status = Arc::new(Mutex::new(AppBusStatus::default()));
        let status_clone = Arc::clone(&status);

        thread::spawn(move || {
            let mut dl_manager = DownloadManager::new();

            loop {
                while let Ok(ev) = rx.try_recv() {
                    match ev {
                        AppEvent::PlaybackStarted => {
                            dl_manager.on_playback_started();
                        }
                        AppEvent::PlaybackEnded { history } => {
                            dl_manager.on_playback_ended();
                            if let Some(entry) = history {
                                let _ = save_history_entry(entry);
                            }
                        }
                        AppEvent::EnqueueDownload(job) => {
                            dl_manager.enqueue(job);
                        }
                        AppEvent::CacheCleared => {
                            // Future cache subscribers can react here
                        }
                    }
                }

                dl_manager.tick();

                if let Ok(mut st) = status_clone.lock() {
                    let dl = dl_manager.status();
                    st.queued_downloads = dl.queued;
                    st.active_download = dl.active;
                    st.download_paused = dl.paused;
                }

                thread::sleep(std::time::Duration::from_millis(150));
            }
        });

        EventBus { tx, status }
    })
}

pub fn emit(event: AppEvent) {
    let bus = init_bus();
    let _ = bus.tx.send(event);
}

pub fn status() -> AppBusStatus {
    let bus = init_bus();
    bus.status.lock().map(|s| s.clone()).unwrap_or_default()
}

use crate::{
    model_manager::begin,
    state::{Host, Result, blocking, lock},
};
use nvsf_app::updates::{self, Manifest};
use serde::Serialize;
use std::{
    io::Read,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{
    Manager,
    utils::{config::BundleType, platform::bundle_type},
};
use tauri_plugin_updater::UpdaterExt;

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub enabled: bool,
    pub state: String,
    pub token: Option<String>,
    pub version: Option<String>,
    pub message: String,
    pub downloaded: u64,
    pub total: u64,
    pub automatic_install: bool,
}

pub struct Offer {
    token: String,
    manifest: Manifest,
    target: String,
    file: Option<tempfile::NamedTempFile>,
}

#[derive(Default)]
pub struct Maintenance {
    pub status: Mutex<Status>,
    offer: Mutex<Option<Offer>>,
    checking: AtomicBool,
    pub(crate) installing: AtomicBool,
    cancel: AtomicBool,
    timer: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl Maintenance {
    pub fn stop(&self) -> bool {
        self.cancel.store(true, Ordering::Release);
        if self.checking.load(Ordering::Acquire) {
            return false;
        }
        if let Ok(mut timer) = self.timer.lock() {
            if let Some(timer) = timer.take() {
                timer.abort();
            }
        }
        !self.checking.load(Ordering::Acquire)
    }
}

fn format() -> &'static str {
    match bundle_type() {
        Some(BundleType::Nsis) => "nsis",
        Some(BundleType::Msi) => "msi",
        Some(BundleType::App)
            if std::env::current_exe().ok().is_some_and(|exe| {
                exe.ancestors()
                    .any(|p| p.extension().is_some_and(|ext| ext == "app"))
            }) =>
        {
            "app"
        }
        Some(BundleType::AppImage) => "appimage",
        Some(BundleType::Deb) => "deb",
        Some(BundleType::Rpm) => "rpm",
        _ => "portable",
    }
}
fn automatic() -> bool {
    matches!(format(), "nsis" | "msi" | "app" | "appimage")
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn check(host: &Host, manual: bool) -> Result<Status> {
    let maintenance = &host.0.maintenance;
    if maintenance.checking.swap(true, Ordering::AcqRel) {
        return update_status_inner(host);
    }
    struct Reset<'a>(&'a AtomicBool);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _reset = Reset(&maintenance.checking);
    {
        let _operation = lock(&host.0.operation)?;
        if host.0.closing.load(Ordering::Acquire) {
            return update_status_inner(host);
        }
        let mut preferences = updates::preferences(&host.0.paths).map_err(|e| e.to_string())?;
        if !preferences.due(now(), manual) {
            return update_status_inner(host);
        }
        if host.0.model_busy.load(Ordering::Acquire) {
            return Err("A resource operation is active; check again when it finishes".into());
        }
        preferences.last_attempt = Some(now());
        updates::save_preferences(&host.0.paths, &preferences).map_err(|e| e.to_string())?;
        *lock(&maintenance.offer)? = None;
        *lock(&maintenance.status)? = Status {
            enabled: preferences.enabled,
            state: "checking".into(),
            automatic_install: automatic(),
            ..Default::default()
        };
    }
    let target = updates::target(format());
    let found = updates::check(env!("CARGO_PKG_VERSION"), &target);
    let _operation = lock(&host.0.operation)?;
    if host.0.closing.load(Ordering::Acquire) {
        return update_status_inner(host);
    }
    let mut offers = lock(&maintenance.offer)?;
    let mut status = lock(&maintenance.status)?;
    match found {
        Ok(Some(manifest)) => {
            let token = host.token("update");
            let artifact = manifest
                .platforms
                .get(&target)
                .ok_or_else(|| "Release target missing".to_string())?;
            *status = Status {
                enabled: updates::preferences(&host.0.paths)
                    .map_err(|e| e.to_string())?
                    .enabled,
                state: "available".into(),
                token: Some(token.clone()),
                version: Some(manifest.version.clone()),
                message: "A newer stable release is available. Nothing has been downloaded.".into(),
                total: artifact.bytes,
                automatic_install: automatic(),
                ..Default::default()
            };
            *offers = Some(Offer {
                token,
                manifest,
                target,
                file: None,
            });
            host.0.log.event(
                "INFO",
                "UPDATES",
                "Official stable release offered; payload awaits explicit approval",
            );
        }
        Ok(None) => {
            status.state = "current".into();
            status.message = "No newer published stable update is available.".into();
        }
        Err(error) => {
            status.state = "error".into();
            status.message =
                format!("Update check unavailable; offline extraction remains available: {error}");
            host.0.log.event(
                "WARNING",
                "UPDATES",
                "Metadata check failed; extraction remains available",
            );
        }
    }
    Ok(status.clone())
}

pub(crate) fn update_status_inner(host: &Host) -> Result<Status> {
    let mut status = lock(&host.0.maintenance.status)?.clone();
    status.enabled = updates::preferences(&host.0.paths)
        .map_err(|e| e.to_string())?
        .enabled;
    status.automatic_install = automatic();
    if status.state.is_empty() {
        status.state = "idle".into();
    }
    Ok(status)
}

#[tauri::command]
pub fn update_status(host: tauri::State<'_, Host>) -> Result<Status> {
    update_status_inner(host.inner())
}
#[tauri::command]
pub async fn check_updates(host: tauri::State<'_, Host>) -> Result<Status> {
    let host = host.inner().clone();
    blocking(move || check(&host, true)).await
}
#[tauri::command]
pub fn set_update_checks(host: tauri::State<'_, Host>, enabled: bool) -> Result<Status> {
    let host = host.inner();
    let _operation = lock(&host.0.operation)?;
    let mut preferences = updates::preferences(&host.0.paths).map_err(|e| e.to_string())?;
    preferences.enabled = enabled;
    updates::save_preferences(&host.0.paths, &preferences).map_err(|e| e.to_string())?;
    update_status_inner(host)
}
#[tauri::command]
pub fn cancel_update(host: tauri::State<'_, Host>) {
    host.inner()
        .0
        .maintenance
        .cancel
        .store(true, Ordering::Release);
}

fn updater(app: &tauri::AppHandle, offer: &Offer) -> Result<tauri_plugin_updater::Update> {
    let raw = serde_json::to_value(&offer.manifest).map_err(|e| e.to_string())?;
    let mut update = app
        .updater_builder()
        .target(&offer.target)
        .build()
        .map_err(|e| e.to_string())?
        .restore_update(raw)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Update offer is no longer newer".to_string())?;
    update.timeout = Some(Duration::from_secs(600));
    Ok(update)
}

#[tauri::command]
pub async fn download_update(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    token: String,
    consent: bool,
) -> Result<Status> {
    if !consent {
        return Err("Explicit update download approval required".into());
    }
    let host = host.inner().clone();
    let operation = begin(&host)?;
    if host.0.maintenance.checking.load(Ordering::Acquire) {
        return Err("Wait for the release check to finish".into());
    }
    host.0.maintenance.cancel.store(false, Ordering::Release);
    blocking(move || {
        let _operation = operation;
        let result = download(&app, &host, &token);
        if let Err(error) = &result {
            let mut status = lock(&host.0.maintenance.status)?;
            status.state = "error".into();
            status.message = error.clone();
        }
        result
    })
    .await
}

fn download(app: &tauri::AppHandle, host: &Host, token: &str) -> Result<Status> {
    let mut offers = lock(&host.0.maintenance.offer)?;
    let offer = offers
        .as_mut()
        .filter(|o| o.token == token)
        .ok_or_else(|| "Update offer expired; check again".to_string())?;
    let artifact = offer
        .manifest
        .platforms
        .get(&offer.target)
        .cloned()
        .ok_or_else(|| "Update target missing".to_string())?;
    updates::validate_artifact(&artifact).map_err(|e| e.to_string())?;
    let update = updater(app, offer)?;
    offer.file = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    let response = updates::client(Duration::from_secs(20))
        .map_err(|e| e.to_string())?
        .get(&artifact.url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    if response
        .content_length()
        .is_some_and(|n| n != artifact.bytes)
    {
        return Err("Update payload size differs from release inventory".into());
    }
    let mut response = response;
    let mut file =
        tempfile::NamedTempFile::new_in(&host.0.paths.cache).map_err(|e| e.to_string())?;
    {
        let mut status = lock(&host.0.maintenance.status)?;
        status.state = "downloading".into();
        status.downloaded = 0;
    }
    let mut received = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("Update transfer exceeded 600-second deadline".into());
        }
        if host.0.maintenance.cancel.load(Ordering::Acquire)
            || host.0.model_cancel.load(Ordering::Acquire)
        {
            return Err("Update download cancelled; installed application unchanged".into());
        }
        let n = response.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        received = received
            .checked_add(n as u64)
            .ok_or_else(|| "Update size overflow".to_string())?;
        if received > artifact.bytes {
            return Err("Update exceeds declared payload size".into());
        }
        use std::io::Write;
        file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        lock(&host.0.maintenance.status)?.downloaded = received;
    }
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    if received != artifact.bytes
        || nvsf_app::storage::hash_file(file.path()).map_err(|e| e.to_string())? != artifact.sha256
    {
        return Err("Update checksum or length mismatch; installed application unchanged".into());
    }
    let bytes = std::fs::read(file.path()).map_err(|e| e.to_string())?;
    update.verify(&bytes).map_err(|e| e.to_string())?;
    nvsf_app::update_payload::validate(&bytes, &offer.target).map_err(|e| e.to_string())?;
    if host.0.maintenance.cancel.load(Ordering::Acquire)
        || host.0.model_cancel.load(Ordering::Acquire)
    {
        return Err("Update download cancelled before verified activation".into());
    }
    offer.file = Some(file);
    let mut status = lock(&host.0.maintenance.status)?;
    status.state = "verified".into();
    status.message = if automatic() { "Signature and checksum verified. Save edits and approve installation when ready." } else { "Verified download ready. Use Save download and the format-specific installation guide; no automatic replacement is supported." }.into();
    Ok(status.clone())
}

#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    token: String,
    consent: bool,
) -> Result<()> {
    if !consent {
        return Err("Explicit update installation approval required".into());
    }
    let host = host.inner().clone();
    blocking(move || {
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        if host.0.maintenance.checking.load(Ordering::Acquire) {
            return Err("Wait for the release check to finish".into());
        }
        nvsf_core::maintenance::authorize_install(
            consent,
            host.0.source_busy.load(Ordering::Acquire) || host.0.closing.load(Ordering::Acquire),
            host.0.unsaved.load(Ordering::Acquire),
            automatic(),
        )
        .map_err(|e| e.to_string())?;
        if host.0.maintenance.cancel.load(Ordering::Acquire) {
            return Err("Update was cancelled; approve a fresh download".into());
        }
        let offers = lock(&host.0.maintenance.offer)?;
        let offer = offers
            .as_ref()
            .filter(|o| o.token == token)
            .ok_or_else(|| "Update offer expired".to_string())?;
        let file = offer
            .file
            .as_ref()
            .ok_or_else(|| "Download and verify the update first".to_string())?;
        let bytes = std::fs::read(file.path()).map_err(|e| e.to_string())?;
        let update = updater(&app, offer)?;
        update.verify(&bytes).map_err(|e| e.to_string())?;
        nvsf_app::update_payload::validate(&bytes, &offer.target).map_err(|e| e.to_string())?;
        if host.0.maintenance.cancel.load(Ordering::Acquire)
            || host.0.unsaved.load(Ordering::Acquire)
            || host.0.closing.load(Ordering::Acquire)
        {
            return Err(
                "Update cancelled, edits changed or close requested before installation".into(),
            );
        }
        host.0.log.event(
            "INFO",
            "UPDATES",
            "Explicit signed installation approved with idle jobs and saved edits",
        );
        host.0.maintenance.installing.store(true, Ordering::Release);
        struct Installing<'a>(&'a AtomicBool);
        impl Drop for Installing<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _installing = Installing(&host.0.maintenance.installing);
        if host.0.closing.load(Ordering::Acquire) || host.0.unsaved.load(Ordering::Acquire) {
            return Err("Close or edit change interrupted update admission".into());
        }
        lock(&host.0.maintenance.status)?.state = "installing".into();
        if let Err(error) = update.install(bytes) {
            let mut status = lock(&host.0.maintenance.status)?;
            status.state = "error".into();
            status.message =
                format!("Native installation failed; inspect the platform installer: {error}");
            return Err(status.message.clone());
        }
        let mut status = lock(&host.0.maintenance.status)?;
        status.state = "installed".into();
        status.message =
            "Update installed. Close and reopen the application to use the new version.".into();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn save_update(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    token: String,
) -> Result<Option<String>> {
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move || {
        let _operation = operation;
        let offers = lock(&host.0.maintenance.offer)?;
        let offer = offers
            .as_ref()
            .filter(|o| o.token == token)
            .ok_or_else(|| "Update offer expired".to_string())?;
        let file = offer
            .file
            .as_ref()
            .ok_or_else(|| "Download and verify the update first".to_string())?;
        let artifact = offer
            .manifest
            .platforms
            .get(&offer.target)
            .ok_or_else(|| "Update target missing".to_string())?;
        let bytes = std::fs::read(file.path()).map_err(|e| e.to_string())?;
        updater(&app, offer)?
            .verify(&bytes)
            .map_err(|e| e.to_string())?;
        nvsf_app::update_payload::validate(&bytes, &offer.target).map_err(|e| e.to_string())?;
        use tauri_plugin_dialog::DialogExt;
        let name = artifact
            .url
            .rsplit('/')
            .next()
            .unwrap_or("application-update");
        let Some(selected) = app
            .dialog()
            .file()
            .set_title("Save verified update; install using the platform guide")
            .set_file_name(name)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let destination = selected.into_path().map_err(|e| e.to_string())?;
        if destination.exists() || destination.is_symlink() {
            return Err("Choose a new filename; existing files are never replaced".into());
        }
        nvsf_app::storage::write_atomic(&destination, &bytes, false).map_err(|e| e.to_string())?;
        Ok(Some(crate::state::display_name(&destination)))
    })
    .await
}

pub fn schedule(app: &tauri::AppHandle) {
    let handle = app.clone();
    let timer = tauri::async_runtime::spawn(async move {
        loop {
            let host = handle.state::<Host>().inner().clone();
            let _ = blocking(move || check(&host, false).map(|_| ())).await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
    match app.state::<Host>().inner().0.maintenance.timer.lock() {
        Ok(mut slot) => *slot = Some(timer),
        Err(_) => timer.abort(),
    }
}

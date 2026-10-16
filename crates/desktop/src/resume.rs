use crate::{
    queue::{Grant, MAX_ENTRIES, QueueEntry, Record},
    state::{Destination, Host, Result, Source, blocking, display_name, lock, path_text},
};
use nvsf_core::Project;
use std::path::{Path, PathBuf};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

fn same_path(selected: &Path, expected: &Path) -> Result<bool> {
    let canonical = |p: &Path| -> Result<PathBuf> {
        if p.is_symlink() {
            return Err("Symlink destinations are not permitted".into());
        }
        if p.exists() {
            p.canonicalize().map_err(|e| e.to_string())
        } else {
            let parent = p
                .parent()
                .ok_or_else(|| "Missing destination parent".to_string())?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            Ok(parent.join(
                p.file_name()
                    .ok_or_else(|| "Missing filename".to_string())?,
            ))
        }
    };
    Ok(canonical(selected)? == canonical(expected)?)
}

pub(crate) fn regrant(
    app: &tauri::AppHandle,
    host: &Host,
    record: &Record,
) -> Result<Option<Grant>> {
    let Some(file) = app
        .dialog()
        .file()
        .set_title("Regrant original queued source (fingerprint must match)")
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let source = file
        .into_path()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !same_path(&source, &record.source)?
        || nvsf_app::storage::hash_file(&source).map_err(|e| e.to_string())? != record.source_hash
    {
        return Err("Selected source does not match its saved path/fingerprint. Existing queue/checkpoint is preserved; restore the original source location.".into());
    }
    let Some(file) = app
        .dialog()
        .file()
        .set_title("Regrant the same queued SRT destination")
        .set_file_name(display_name(&record.output))
        .add_filter("SubRip", &["srt"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let output = file.into_path().map_err(|e| e.to_string())?;
    if !same_path(&output, &record.output)? {
        return Err(
            "Choose the original output location; changing it invalidates checkpoint compatibility"
                .into(),
        );
    }
    let media = nvsf_app::media::MediaRuntime::load(&host.0.paths)
        .and_then(|runtime| runtime.probe(&source))
        .map_err(|e| e.to_string())?;
    let source_token = host.token("source");
    let output_token = host.token("output");
    let mut sources = lock(&host.0.sources)?;
    let mut destinations = lock(&host.0.destinations)?;
    if sources.len() >= 1024 || destinations.len() >= 256 {
        return Err("Session grant limit reached; restart the application and explicitly regrant the saved queue".into());
    }
    sources.insert(source_token.clone(), source.clone());
    destinations.insert(output_token.clone(), record.output.clone());
    Ok(Some(Grant {
        source: Source {
            token: source_token,
            name: display_name(&source),
            fingerprint: record.source_hash.clone(),
            media,
        },
        output: Destination {
            token: output_token,
            name: display_name(&output),
        },
    }))
}

pub fn check_checkpoint(project: &Project, path: &Path) -> Result<()> {
    project.validate().map_err(|e| e.to_string())?;
    if !Path::new(&project.config.input).is_absolute()
        || !Path::new(&project.config.output).is_absolute()
    {
        return Err("Checkpoint uses relative source/output paths. Restore it through the CLI from its original working directory; GUI does not guess filesystem grants".into());
    }
    if project.completed {
        return Err("This project is already complete; open it in Review instead".into());
    }
    if Path::new(&project.config.output).with_extension("nvsf.json") != path {
        return Err(
            "Choose the original output-associated .nvsf.json checkpoint, not a copied review file"
                .into(),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn pick_checkpoint(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
) -> Result<Option<Vec<QueueEntry>>> {
    let host = host.inner().clone();
    blocking(move ||{
        host.ensure_idle()?;
        if host.0.unsaved.load(std::sync::atomic::Ordering::Acquire) { return Err("Save current review corrections before opening a checkpoint".into()); }
        let Some(file)=app.dialog().file().set_title("Resume existing extraction checkpoint").add_filter("Numera checkpoint", &["json"]).blocking_pick_file()else{return Ok(None);};
        let path=file.into_path().map_err(|e|e.to_string())?;
        let project=nvsf_app::storage::load_project(&path).map_err(|e|e.to_string())?;
        let expected=Path::new(&project.config.output).with_extension("nvsf.json");
        if !same_path(&path,&expected)? {return Err("Checkpoint is not at its original output-associated location; original data preserved".into());}
        check_checkpoint(&project,&expected)?;
        let root=path_text(&host.0.paths.root)?;
        if project.config.data_root.as_deref().is_some_and(|saved|saved!=root.as_str()){return Err("Checkpoint uses a different data root. Restore that root before resuming; models and existing state are preserved.".into());}
        let message=format!("Resume at {} ms with {} completed cues and {} active appearances. Corrections stored in this checkpoint will be kept. Corrections in other review files are not imported; save them separately before replacing the current workspace. Source, models, regions, overwrite policy and output configuration must remain compatible. Unsaved review drafts must be saved first. Continue to regrant the original source/output?",project.processed_ms,project.cues.len(),project.active_appearances.len());
        if !app.dialog().message(message).title("Resume preservation check").buttons(MessageDialogButtons::OkCancelCustom("Regrant source/output".into(),"Cancel".into())).blocking_show(){return Ok(None);}
        let record=Record {id:host.token("queue"),source:PathBuf::from(&project.config.input),output:PathBuf::from(&project.config.output),source_hash:project.source_hash,config:project.config,checkpoint:Some(expected),state:"interrupted".into(),error:None};
        let Some(grant)=regrant(&app,&host,&record)?else{return Ok(None);};
        let _operation=lock(&host.0.operation)?;host.ensure_idle()?;
        let mut store=lock(&host.0.queue)?;store.check()?;
        if let Some(index)=store.file.records.iter().position(|r|r.output==record.output) {
            let previous=store.file.records[index].clone();
            if previous.source_hash!=record.source_hash || serde_json::to_vec(&previous.config).map_err(|e|e.to_string())?!=serde_json::to_vec(&record.config).map_err(|e|e.to_string())? {
                return Err("Existing queue record conflicts with this checkpoint; original record preserved".into());
            }
            let id=previous.id.clone();let mut resumed=record;resumed.id=id.clone();store.file.records[index]=resumed;
            if let Err(e)=store.save(&host.0.paths.root){store.file.records[index]=previous;return Err(e);}
            store.grants.insert(id,grant);return Ok(Some(store.entries()?));
        }
        if store.file.records.len()>=MAX_ENTRIES{return Err("Queue limit reached; remove a finished record".into());}
        store.file.records.push(record.clone());
        if let Err(e)=store.save(&host.0.paths.root){store.file.records.pop();return Err(e);}
        store.grants.insert(record.id,grant);Ok(Some(store.entries()?))
    }).await
}

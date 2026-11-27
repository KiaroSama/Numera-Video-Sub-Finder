use clap::{ArgAction, Parser, Subcommand};
use nvsf_core::Rect;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Extract visible video subtitles locally")]
pub(crate) struct Cli {
    #[arg(long, global = true)]
    pub(crate) data_root: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    Extract(ExtractArgs),
    Probe {
        input: PathBuf,
    },
    Doctor {
        #[arg(long, default_value = "json", value_parser = ["json"])]
        format: String,
    },
    /// Verify native APIs or repair this application without changing system software.
    Dependencies {
        #[arg(long)]
        repair: bool,
        #[arg(long, requires = "repair")]
        consent: bool,
    },
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Languages {
        #[command(subcommand)]
        command: LanguageCommand,
    },
    /// Resume the explicitly selected checkpoint, preserving its saved corrections and location.
    Resume {
        project: PathBuf,
    },
    Export {
        project: PathBuf,
        output: PathBuf,
        #[arg(long, default_value = "srt")]
        format: String,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        bom: bool,
        #[arg(long)]
        crlf: bool,
    },
    Batch {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long, default_value = "en")]
        language: String,
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
    },
    /// Extract subtitles from multiple videos concurrently with isolated error capture.
    Bulk(Box<BulkArgs>),
    /// Inspect, search, edit, review, and save project files non-interactively.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Detect candidate subtitle regions in a video.
    Regions {
        #[command(subcommand)]
        command: RegionsCommand,
    },
    /// Decode and save video preview frames as PNG.
    Preview {
        #[command(subcommand)]
        command: PreviewCommand,
    },
    /// Manage and execute the persistent extraction queue.
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    /// Inspect, check, configure, download, and verify signed application updates.
    Updates {
        #[command(subcommand)]
        command: UpdatesCommand,
    },
    /// Read or update local application preferences.
    Preferences {
        #[command(subcommand)]
        command: PreferencesCommand,
    },
    Benchmark(ExtractArgs),
    #[command(hide = true)]
    NativeRequest {
        #[arg(long)]
        request: PathBuf,
    },
    #[command(hide = true)]
    Worker {
        #[command(subcommand)]
        command: Option<WorkerCommand>,
        #[arg(long, requires = "control")]
        request: Option<PathBuf>,
        #[arg(long, requires = "request")]
        control: Option<PathBuf>,
        #[arg(long, hide = true)]
        provider_profile: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with = "provider_profile")]
        resume_project: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with_all = ["provider_profile", "resume_project"])]
        srt_bom: bool,
        #[arg(long, hide = true, conflicts_with_all = ["provider_profile", "resume_project"])]
        srt_crlf: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum WorkerCommand {
    PreviewDaemon {
        #[arg(long)]
        input: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum ModelCommand {
    List,
    Install {
        #[arg(long)]
        id: Option<String>,
    },
    Verify,
    Import {
        id: String,
        path: PathBuf,
    },
    Export {
        id: String,
        path: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum LanguageCommand {
    List,
    Install { code: String },
    Verify { code: String },
    Import { code: String, path: PathBuf },
    Export { code: String, path: PathBuf },
}

#[derive(Clone, clap::Args)]
pub(crate) struct ExtractArgs {
    #[arg(long)]
    pub(crate) input: PathBuf,
    #[arg(long)]
    pub(crate) output: PathBuf,
    #[arg(long, alias = "languages", default_value = "en")]
    pub(crate) language: String,
    #[arg(long, default_value = "auto")]
    pub(crate) device: String,
    #[arg(long, default_value = "balanced")]
    pub(crate) quality: String,
    #[arg(long)]
    pub(crate) roi: Vec<Rect>,
    #[arg(long)]
    pub(crate) exclude: Vec<Rect>,
    #[arg(long, default_value_t = 0)]
    pub(crate) start_ms: u64,
    #[arg(long)]
    pub(crate) end_ms: Option<u64>,
    #[arg(long, default_value_t = 2)]
    pub(crate) threads: usize,
    #[arg(long)]
    pub(crate) overwrite: bool,
    #[arg(long)]
    pub(crate) normalize_persian: bool,
    #[arg(long)]
    pub(crate) gpu_required: bool,
    #[arg(long)]
    pub(crate) accelerator_required: bool,
    /// Record actual two-stage ONNX placement; requires an explicit device and a new absolute directory.
    #[arg(long, value_name = "ABSOLUTE_NEW_DIR")]
    pub(crate) provider_profile: Option<PathBuf>,
}

#[derive(Clone, clap::Args)]
pub(crate) struct BulkArgs {
    #[arg(long = "input")]
    pub(crate) inputs: Vec<PathBuf>,
    #[arg(long)]
    pub(crate) inputs_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) input_dir: Option<PathBuf>,
    #[arg(long, default_value = "mp4,mkv,mov,webm,avi")]
    pub(crate) extensions: String,
    #[arg(long)]
    pub(crate) output_dir: Option<PathBuf>,
    #[arg(long, default_value_t = 2)]
    pub(crate) concurrency: usize,
    #[arg(long)]
    pub(crate) fail_fast: bool,
    #[arg(long, alias = "languages", default_value = "en")]
    pub(crate) language: String,
    #[arg(long, default_value = "auto")]
    pub(crate) device: String,
    #[arg(long, default_value = "balanced")]
    pub(crate) quality: String,
    #[arg(long = "roi", alias = "region")]
    pub(crate) roi: Vec<Rect>,
    #[arg(long)]
    pub(crate) exclude: Vec<Rect>,
    #[arg(long, default_value_t = 0)]
    pub(crate) start_ms: u64,
    #[arg(long)]
    pub(crate) end_ms: Option<u64>,
    #[arg(long, default_value_t = 2)]
    pub(crate) threads: usize,
    #[arg(long, default_value = "srt")]
    pub(crate) format: String,
    #[arg(long)]
    pub(crate) overwrite: bool,
    #[arg(long)]
    pub(crate) resume: bool,
    #[arg(long)]
    pub(crate) bom: bool,
    #[arg(long)]
    pub(crate) crlf: bool,
    #[arg(long)]
    pub(crate) normalize_persian: bool,
    #[arg(long, default_value_t = 0.7)]
    pub(crate) min_confidence: f32,
    #[arg(long)]
    pub(crate) gpu_required: bool,
    #[arg(long)]
    pub(crate) accelerator_required: bool,
}

#[derive(Subcommand)]
pub(crate) enum ProjectCommand {
    Inspect {
        project: PathBuf,
    },
    Cues {
        project: PathBuf,
        #[arg(long, default_value = "")]
        query: String,
        #[arg(long, default_value = "all")]
        filter: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        limit: Option<usize>,
    },
    EditCue {
        project: PathBuf,
        #[arg(long)]
        id: u64,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        start_ms: Option<u64>,
        #[arg(long)]
        end_ms: Option<u64>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    AcceptCue {
        project: PathBuf,
        #[arg(long)]
        id: u64,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    RevertCue {
        project: PathBuf,
        #[arg(long)]
        id: u64,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    SplitCue {
        project: PathBuf,
        #[arg(long)]
        id: u64,
        #[arg(long)]
        at_ms: u64,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    MergeCues {
        project: PathBuf,
        #[arg(long)]
        first: u64,
        #[arg(long)]
        second: u64,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    ReOcr {
        project: PathBuf,
        #[arg(long)]
        id: u64,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        language: Option<String>,
        #[arg(long)]
        region: Option<Rect>,
        #[arg(long)]
        at_ms: Option<u64>,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    VisualRetry {
        project: PathBuf,
        #[arg(long)]
        id: u64,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        opt_in: bool,
        #[arg(long)]
        region: Option<Rect>,
        #[arg(long)]
        at_ms: Option<u64>,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    VisualRetryInstall {
        #[arg(long)]
        opt_in: bool,
    },
    CueEvidence {
        project: PathBuf,
        #[arg(long)]
        id: u64,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
    },
    Save {
        project: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    History {
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
}

#[derive(Subcommand)]
pub(crate) enum RegionsCommand {
    Suggest {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "en")]
        language: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum PreviewCommand {
    Frame {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        at_ms: u64,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        crop: Option<Rect>,
        #[arg(long)]
        grid: bool,
        #[arg(long = "draw-box")]
        draw_box: Vec<Rect>,
        #[arg(long = "pixel-box")]
        pixel_box: Vec<crate::frame_overlay::PixelBox>,
        #[arg(long)]
        overwrite: bool,
    },
    Strip {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long, default_value_t = 6)]
        count: usize,
        #[arg(long, default_value_t = 0)]
        start_ms: u64,
        #[arg(long)]
        end_ms: Option<u64>,
        #[arg(long)]
        crop: Option<Rect>,
        #[arg(long)]
        grid: bool,
        #[arg(long = "draw-box")]
        draw_box: Vec<Rect>,
        #[arg(long = "pixel-box")]
        pixel_box: Vec<crate::frame_overlay::PixelBox>,
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(Clone, clap::Args)]
pub(crate) struct QueueAddArgs {
    #[arg(long)]
    pub(crate) input: PathBuf,
    #[arg(long)]
    pub(crate) output: PathBuf,
    #[arg(long, alias = "languages", default_value = "en")]
    pub(crate) language: String,
    #[arg(long, default_value = "auto")]
    pub(crate) device: String,
    #[arg(long, default_value = "balanced")]
    pub(crate) quality: String,
    #[arg(long = "roi", alias = "region")]
    pub(crate) roi: Vec<Rect>,
    #[arg(long)]
    pub(crate) exclude: Vec<Rect>,
    #[arg(long, default_value_t = 0)]
    pub(crate) start_ms: u64,
    #[arg(long)]
    pub(crate) end_ms: Option<u64>,
    #[arg(long, default_value_t = 2)]
    pub(crate) threads: usize,
    #[arg(long)]
    pub(crate) overwrite: bool,
    #[arg(long)]
    pub(crate) normalize_persian: bool,
    #[arg(long, default_value_t = 0.7)]
    pub(crate) min_confidence: f32,
    #[arg(long)]
    pub(crate) gpu_required: bool,
    #[arg(long)]
    pub(crate) accelerator_required: bool,
    #[arg(long)]
    pub(crate) checkpoint: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum QueueCommand {
    List,
    Add(Box<QueueAddArgs>),
    Remove {
        #[arg(long)]
        id: String,
    },
    Clear {
        #[arg(long, default_value = "all")]
        state: String,
    },
    Run {
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
        #[arg(long)]
        fail_fast: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum UpdatesCommand {
    Status {
        #[arg(long, default_value = "portable")]
        format: String,
    },
    Check {
        #[arg(long, default_value = "portable")]
        format: String,
    },
    Config {
        #[arg(long, action = ArgAction::Set)]
        enabled: bool,
    },
    Download {
        #[arg(long)]
        token: String,
        #[arg(long)]
        consent: bool,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    Verify {
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        signature: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        sha256: Option<String>,
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum PreferencesCommand {
    Get,
    Set {
        #[arg(long)]
        update_checks: Option<bool>,
        #[arg(long)]
        reset_last_attempt: bool,
    },
}

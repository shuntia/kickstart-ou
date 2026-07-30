use std::{
    env::{self, consts::ARCH, home_dir},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use eframe::{App, CreationContext, NativeOptions};
use egui::{RichText, Ui};
use parselnk::Lnk;
use rfd::MessageDialog;
use serde::{Deserialize, Serialize};

/// Boxed error type shared by the background installer threads.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

fn main() {
    if os_info::get().os_type() == os_info::Type::Windows {
        rfd::MessageDialog::new()
            .set_title(":/")
            .set_description("このソフトはWindows専用です。LinuxやMacOSの動作は保証されません。")
            .set_level(rfd::MessageLevel::Warning)
            .show();
    }
    let native_options = NativeOptions::default();
    if let Err(e) = eframe::run_native(
        "Kickstart UTAU!",
        native_options,
        Box::new(|cc| Ok(Box::new(Kickstart::new(cc)))),
    ) {
        let _ = MessageDialog::new()
            .set_description(format!(
                "エラーが発生しました。開発者に報告してください。\n{:?}",
                e
            ))
            .set_title(":(")
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

static KICKSTART_DATA_DIR: &'static str = ".kickstart";
static STORAGE_FILENAME: &'static str = "state.json";
static LAST_PAGE: u32 = 5;
static TABLE_OF_CONTENTS: &[&str] = &["はじめに", "目次"];

#[derive(Default)]
pub struct Kickstart {
    state: KickstartData,
    save_path: PathBuf,
    /// The currently running install (official installer or portable), if any.
    subprocess: Option<InstallJob>,
    /// Last install error, surfaced in the UI until the next attempt.
    install_error: Option<String>,
}

/// Paths discovered after installing OpenUtau: where the program lives, and
/// where OpenUtau expects singers (voicebanks) to live.
struct OuInstall {
    install_dir: PathBuf,
    singer_dir: PathBuf,
}

/// A running install thread plus a shared progress handle the UI can poll.
struct InstallJob {
    handle: thread::JoinHandle<Result<OuInstall, BoxError>>,
    progress: Arc<Mutex<Progress>>,
}

/// Coarse progress reported by an install thread.
#[derive(Clone, Default)]
struct Progress {
    /// Human-readable description of the current stage.
    stage: String,
    /// Fraction in [0, 1] when known; `None` shows a spinner instead.
    fraction: Option<f32>,
}

/// Update the shared progress handle from a worker thread.
fn set_progress(progress: &Arc<Mutex<Progress>>, stage: &str, fraction: Option<f32>) {
    if let Ok(mut guard) = progress.lock() {
        guard.stage = stage.to_string();
        guard.fraction = fraction;
    }
}

/// Map the host architecture to the suffix used by OpenUtau / .NET downloads.
fn win_arch() -> Result<&'static str, BoxError> {
    match ARCH {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("arm64"),
        "x86" => Ok("x86"),
        other => Err(format!("未対応のアーキテクチャです: {other}").into()),
    }
}

/// Stream a download to `dest`, reporting a fraction when the server sends a
/// Content-Length.
fn download_with_progress(
    url: &str,
    dest: &Path,
    progress: &Arc<Mutex<Progress>>,
    stage: &str,
) -> Result<(), BoxError> {
    let mut resp = reqwest::blocking::get(url)?.error_for_status()?;
    let total = resp.content_length();
    let mut file = File::create(dest)?;
    let mut buf = [0u8; 65536];
    let mut downloaded: u64 = 0;
    loop {
        let read = resp.read(&mut buf)?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])?;
        downloaded += read as u64;
        let fraction = total.and_then(|t| (t > 0).then(|| downloaded as f32 / t as f32));
        set_progress(progress, stage, fraction);
    }
    Ok(())
}

/// Extract every entry of a zip into `dest`, reporting per-entry progress.
fn extract_zip(
    zip_path: &Path,
    dest: &Path,
    progress: &Arc<Mutex<Progress>>,
) -> Result<(), BoxError> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?)?;
    let count = archive.len();
    for i in 0..count {
        let mut entry = archive.by_index(i)?;
        // `enclosed_name` rejects path traversal (zip-slip); skip anything unsafe.
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
        }
        let fraction = if count > 0 {
            (i + 1) as f32 / count as f32
        } else {
            1.0
        };
        set_progress(progress, "ファイルを展開しています...", Some(fraction));
    }
    Ok(())
}

fn dotnet8_desktop_present() -> bool {
    Command::new("dotnet")
        .arg("--list-runtimes")
        .output()
        .ok()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.starts_with("Microsoft.WindowsDesktop.App 8."))
        })
        .unwrap_or(false)
}

fn ensure_dotnet8(progress: &Arc<Mutex<Progress>>, tmp_dir: &Path) -> Result<(), BoxError> {
    if dotnet8_desktop_present() {
        return Ok(());
    }
    let arch = win_arch()?;
    let url = format!("https://aka.ms/dotnet/8.0/windowsdesktop-runtime-win-{arch}.exe");
    let installer = tmp_dir.join("windowsdesktop-runtime-8.exe");
    download_with_progress(
        &url,
        &installer,
        progress,
        ".NET 8 ランタイムをダウンロードしています...",
    )?;
    set_progress(
        progress,
        ".NET 8 ランタイムをインストールしています...",
        None,
    );
    let status = Command::new(&installer)
        .args(["/install", "/quiet", "/norestart"])
        .status()?;
    match status.code() {
        Some(0) | Some(3010) => {}
        Some(code) => {
            return Err(format!(
                ".NET 8 ランタイムのインストールに失敗しました。(終了コード: {code})"
            )
            .into());
        }
        None => return Err(".NET 8 ランタイムのインストールが中断されました。".into()),
    }
    let _ = std::fs::remove_file(&installer);
    Ok(())
}

#[cfg(windows)]
fn create_start_menu_shortcut(target: &Path, lnk_path: &Path) -> Result<(), BoxError> {
    use mslnk::ShellLink;
    let mut link =
        ShellLink::new(target).map_err(|e| format!("ショートカットの作成に失敗しました: {e:?}"))?;
    if let Some(parent) = target.parent() {
        link.set_working_dir(Some(parent.to_string_lossy().into_owned()));
    }
    link.set_name(Some("OpenUtau".to_string()));
    link.create_lnk(lnk_path)
        .map_err(|e| format!("ショートカットの書き込みに失敗しました: {e:?}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn create_start_menu_shortcut(_target: &Path, _lnk_path: &Path) -> Result<(), BoxError> {
    Err("スタートメニューへの登録はWindowsでのみ対応しています。".into())
}

#[cfg(windows)]
/// Per-user Start Menu \ Programs directory on Windows.
fn start_menu_programs_dir() -> PathBuf {
    PathBuf::from(env::var("APPDATA").unwrap_or_default())
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
}

impl Kickstart {
    fn new(_cc: &CreationContext<'_>) -> Self {
        let save_path = home_dir()
            .unwrap_or_default()
            .join(KICKSTART_DATA_DIR)
            .join(STORAGE_FILENAME);
        let state = File::open(&save_path)
            .ok()
            .and_then(|mut file| {
                let mut buf = String::new();
                file.read_to_string(&mut buf).ok()?;
                serde_json::from_str(&buf).ok()
            })
            .unwrap_or_default();
        Self {
            save_path,
            state,
            ..Default::default()
        }
    }
    #[expect(unused)]
    fn page_template(&mut self, ui: &mut Ui) {
        todo!()
    }
    // PAGES BEGIN
    /// 0: はじめに
    fn page_0(&mut self, ui: &mut Ui) {
        ui.label("このソフトは自動でUTAUのボイスバンクを作る補助をするソフトです。\n基本的にはUTAU音源制作Wikiの内情に沿っています。早速右下の「次へ」をクリックして進みましょう！");
        ui.hyperlink_to(
            "参考：UTAU音源制作Wikiの元記事",
            "https://w.atwiki.jp/vbmaker/pages/43.html",
        );
    }
    fn page_1(&mut self, ui: &mut Ui) {
        ui.label("目次");
        for (i, title) in TABLE_OF_CONTENTS.iter().enumerate() {
            if ui.button(*title).clicked() {
                self.state.page = i as u32;
            }
        }
    }
    fn page_2(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("必要なもの").size(20.0));
        ui.separator();
        ui.label("- パソコン(Windows)");
        ui.label("- マイク、オーディオインターフェースなどの録音機材");
        ui.hyperlink_to("→詳細", "https://w.atwiki.jp/vbmaker/pages/14.html");
        ui.label("- 収録環境");
        ui.label("録音、原音設定、調声のソフトは自動でインストールされます。");
    }
    fn page_3(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("データの初期設定").size(20.0));
        ui.label("このソフトは全て管理して最小限で自動インストールすることができますが、OpenUtau公式のインストーラを使用しますか？");
        ui.radio_value(&mut self.state.use_openutau_installer, true, "はい");
        ui.radio_value(&mut self.state.use_openutau_installer, false, "いいえ");

        // While an install is running, just show its progress and keep the UI
        // repainting; hide the action buttons until it finishes.
        #[cfg(windows)]
        if self.subprocess.is_some() {
            self.poll_install_job(ui);
            return;
        }
        if self.state.ou_singer_dir.is_some() {
            ui.separator();
            ui.label("OpenUtauのインストールとファイルの確認が完了しました。");
            if let Some(dir) = &self.state.ou_dir {
                ui.label(format!("インストール先: {}", dir.display()));
            }
            if let Some(dir) = &self.state.ou_singer_dir {
                ui.label(format!("音源フォルダ: {}", dir.display()));
            }
            return;
        }
        if let Some(err) = &self.install_error {
            ui.colored_label(egui::Color32::RED, format!("エラー: {err}"));
        }

        #[cfg(windows)]
        {
            if self.state.use_openutau_installer {
                ui.label("OpenUtau公式のインストーラをダウンロードします。よろしいですか？");
                if ui.button("はい").clicked() {
                    self.install_error = None;
                    self.subprocess = Some(self.exec_ou_installer());
                }
            } else {
                // Portable flow: download the zip, install .NET 8, register a Start
                // Menu entry. Defaults the destination to ~/OpenUtau.
                let default_dir = home_dir().unwrap_or_default().join("OpenUtau");
                let dest = self.state.ou_dir.clone().unwrap_or(default_dir);
                ui.label("OpenUtauのポータブル版をインストールする場所を指定してください。");
                ui.label(format!("インストール先: {}", dest.display()));
                ui.horizontal(|ui| {
                    if ui.button("フォルダを選択").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_directory(dest.parent().unwrap_or(&dest))
                            .pick_folder()
                        {
                            self.state.ou_dir = Some(path);
                        }
                    }
                    if ui.button("インストール開始").clicked() {
                        self.install_error = None;
                        self.state.ou_dir = Some(dest.clone());
                        self.subprocess = Some(self.exec_portable_install(dest.clone()));
                    }
                });
            }
        }

        #[cfg(unix)]
        {
            ui.label("Linuxの場合はディストロに応じた方法でOpenUtauをインストールしてください。インストール後、OpenUtauの実行ファイルと音源フォルダの場所を入力してください。");
            if ui.button("インストール完了").clicked() {
                if Command::new("which")
                    .arg("openutau")
                    .output()
                    .ok()
                    .and_then(|out| {
                        String::from_utf8_lossy(&out.stdout)
                            .lines()
                            .next()
                            .map(|line| PathBuf::from(line.trim()))
                    })
                    .is_none()
                {
                    self.install_error =
                        Some("システムのPATHにOpenUtauが見つかりませんでした。必要無い、又は既にインストールされている場合は無視しても大丈夫です。".into());
                    return;
                }
            }
        }
    }
    fn page_4(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("録音・原音設定ソフトの初期設定").size(20.0));
        ui.label("このソフトはRecStar及びvLabelerを使用します。");
        ui.label("RecStarは録音ソフト、vLabelerは原音設定ソフトです。");
        ui.label("RecStarは1.2.0、vLabelerは1.5.0を使用します。最新版を使用する場合は手動でインストールしてください。別のバージョンではソフトの動作が異なる場合があります。");
        if ui.button("RecStarの初期設定").clicked() {
            if let Err(e) = self.exec_recstar_setup() {
                self.install_error = Some(format!("RecStarの初期設定に失敗しました: {e}"));
            }
        }
        if ui.button("vLabelerの初期設定").clicked() {
            if let Err(e) = self.exec_vlabeler_setup() {
                self.install_error = Some(format!("vLabelerの初期設定に失敗しました: {e}"));
            }
        }
        if ui.button("両方インストールする").clicked() {
            if let Err(e) = self.exec_recstar_setup() {
                self.install_error = Some(format!("RecStarの初期設定に失敗しました: {e}"));
            }
            if let Err(e) = self.exec_vlabeler_setup() {
                self.install_error = Some(format!("vLabelerの初期設定に失敗しました: {e}"));
            }
        }
        if self.install_error.is_none() {
            ui.label("初期設定が完了しました。次に進んで下さい。");
        } else {
            ui.colored_label(
                egui::Color32::RED,
                format!("エラー: {}", self.install_error.as_ref().unwrap()),
            );
        }
    }

    fn page_5(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("録音しよう").size(20.0));
        ui.label("では、早速録音をはじめましょう！下のボタンを押すと録音ソフトが起動します。");
        if ui.button("録音ソフトを起動する").clicked() {
            let recstar_path = dirs::data_dir()
                .unwrap_or_default()
                .join("recstar")
                .join("RecStar.exe");
            if !recstar_path.exists() {
                ui.label("RecStarが見つかりませんでした。RecStarの初期設定を行ってください。");
                return;
            } else {
                if let Err(e) = Command::new(&recstar_path).spawn() {
                    ui.label(format!("録音ソフトの起動に失敗しました: {e}"));
                    ui.label("RecStarを手動で起動して録音してください。");
                } else {
                    ui.label("録音ソフトを起動しました。次のページで使い方を説明します。");
                }
            }
        }
    }

    // PAGES END

    fn exec_recstar_setup(&self) -> Result<(), BoxError> {
        let recstar_path = self.state.data_dir.join("recstar");
        if recstar_path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&recstar_path)?;
        let url =
            "https://github.com/sdercolin/recstar/releases/download/1.2.0/recstar-1.2.0-win64.zip";
        let zip_path = env::temp_dir().join("recstar.zip");
        download_with_progress(
            url,
            &zip_path,
            &Arc::new(Mutex::new(Progress::default())),
            "RecStarをダウンロードしています...",
        )?;
        extract_zip(
            &zip_path,
            &recstar_path,
            &Arc::new(Mutex::new(Progress::default())),
        )?;
        let _ = std::fs::remove_file(&zip_path);
        Ok(())
    }

    fn exec_vlabeler_setup(&self) -> Result<(), BoxError> {
        let vlabeler_path = self.state.data_dir.join("recstar");
        if vlabeler_path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&vlabeler_path)?;
        let url =
            "https://github.com/sdercolin/vlabeler/releases/download/1.5.0/recstar-1.5.0-win64.zip";
        let zip_path = env::temp_dir().join("vlabeler.zip");
        download_with_progress(
            url,
            &zip_path,
            &Arc::new(Mutex::new(Progress::default())),
            "vLabelerをダウンロードしています...",
        )?;
        extract_zip(
            &zip_path,
            &vlabeler_path,
            &Arc::new(Mutex::new(Progress::default())),
        )?;
        let _ = std::fs::remove_file(&zip_path);
        Ok(())
    }

    #[cfg(windows)]
    /// Poll the running install job: show progress while it runs, and record the
    /// result (paths or error) once it finishes.
    fn poll_install_job(&mut self, ui: &mut Ui) {
        let Some(job) = self.subprocess.take() else {
            return;
        };
        if job.handle.is_finished() {
            match job.handle.join() {
                Ok(Ok(paths)) => {
                    self.state.ou_dir = Some(paths.install_dir);
                    self.state.ou_singer_dir = Some(paths.singer_dir);
                }
                Ok(Err(e)) => {
                    self.install_error = Some(format!("{e}"));
                }
                Err(_) => {
                    self.install_error =
                        Some("インストール中にスレッドがパニックしました。手動インストールをおすすめします。".into());
                }
            }
            // Job is consumed; leaving `subprocess` as None ends the progress UI.
            ui.ctx().request_repaint();
            return;
        }
        let progress = job
            .progress
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        ui.separator();
        ui.label(&progress.stage);
        match progress.fraction {
            Some(fraction) => {
                ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
            }
            None => {
                ui.spinner();
            }
        }
        // Keep repainting so progress updates from the worker thread are shown.
        ui.ctx().request_repaint();
        self.subprocess = Some(job);
    }

    #[cfg(windows)]
    /// Run OpenUtau's official installer: download it, execute it, then locate
    /// where it installed and where singers should go (see [`OuInstall`]).
    fn exec_ou_installer(&self) -> InstallJob {
        let progress = Arc::new(Mutex::new(Progress::default()));
        let worker = progress.clone();
        let handle = thread::spawn(move || -> Result<OuInstall, BoxError> {
            let installer_path = env::temp_dir().join("OpenUtauInstaller.exe");
            let url = format!(
                "https://github.com/openutau/OpenUtau/releases/latest/download/OpenUtau-win-{}.exe",
                win_arch()?
            );
            download_with_progress(
                &url,
                &installer_path,
                &worker,
                "インストーラをダウンロードしています...",
            )?;
            set_progress(&worker, "インストーラを実行しています...", None);
            Command::new(&installer_path).spawn()?.wait()?;

            // Find start menu entry of OpenUtau and get its target path.
            set_progress(&worker, "インストール先を確認しています...", None);
            let start_menu = start_menu_programs_dir().join("OpenUtau.lnk");
            if !start_menu.exists() {
                return Err("OpenUtauのインストールに失敗しました。スタートメニューのショートカットが見つかりませんでした。".into());
            }
            // Parse the .lnk shortcut to locate where OpenUtau was installed.
            let lnk = Lnk::try_from(start_menu.as_path())?;
            // Prefer the shortcut's working directory (the install folder); fall
            // back to the parent directory of the target executable.
            let install_dir = lnk
                .working_dir()
                .or_else(|| {
                    lnk.link_info
                        .local_base_path
                        .as_ref()
                        .map(PathBuf::from)
                        .and_then(|target| target.parent().map(PathBuf::from))
                })
                .ok_or("OpenUtauのショートカットからインストール先を取得できませんでした。")?;
            // OpenUtau stores singers separately from the program files. It
            // mirrors PathManager.cs: an installed build (one with `installed.txt`
            // next to the exe) keeps its data under <Documents>\OpenUtau, so
            // singers live in <Documents>\OpenUtau\Singers. Only a portable build
            // uses <install dir>\Singers.
            let singer_dir = singer_dir_for(&install_dir)?;
            // Probe the singer directory: OpenUtau creates it lazily, so ensure it
            // exists for later voicebank placement.
            std::fs::create_dir_all(&singer_dir)?;
            Ok(OuInstall {
                install_dir,
                singer_dir,
            })
        });
        InstallJob { handle, progress }
    }

    #[cfg(windows)]
    /// Install the portable build into `dest`: download and extract the zip,
    /// ensure the .NET 8 runtime is present, and register a Start Menu entry.
    fn exec_portable_install(&self, dest: PathBuf) -> InstallJob {
        let progress = Arc::new(Mutex::new(Progress::default()));
        let worker = progress.clone();
        let handle = thread::spawn(move || -> Result<OuInstall, BoxError> {
            std::fs::create_dir_all(&dest)?;
            let arch = win_arch()?;

            // 1. Download the portable zip.
            let url = format!(
                "https://github.com/openutau/OpenUtau/releases/latest/download/OpenUtau-win-{arch}.zip"
            );
            let zip_path = env::temp_dir().join("OpenUtau-portable.zip");
            download_with_progress(
                &url,
                &zip_path,
                &worker,
                "OpenUtauをダウンロードしています...",
            )?;

            // 2. Extract it into the destination folder.
            extract_zip(&zip_path, &dest, &worker)?;
            let _ = std::fs::remove_file(&zip_path);

            // 3. Ensure the .NET 8 desktop runtime (required by OpenUtau) is present.
            set_progress(&worker, ".NET 8 ランタイムを確認しています...", None);
            ensure_dotnet8(&worker, &env::temp_dir())?;

            // 4. Register a Start Menu shortcut to OpenUtau.exe.
            set_progress(&worker, "スタートメニューに登録しています...", None);
            let exe = dest.join("OpenUtau.exe");
            if !exe.exists() {
                return Err("展開後にOpenUtau.exeが見つかりませんでした。".into());
            }
            let start_menu_dir = start_menu_programs_dir();
            std::fs::create_dir_all(&start_menu_dir)?;
            create_start_menu_shortcut(&exe, &start_menu_dir.join("OpenUtau.lnk"))?;

            // 5. Portable builds keep singers next to the executable.
            let singer_dir = dest.join("Singers");
            std::fs::create_dir_all(&singer_dir)?;

            set_progress(&worker, "完了しました。", Some(1.0));
            Ok(OuInstall {
                install_dir: dest,
                singer_dir,
            })
        });
        InstallJob { handle, progress }
    }
}

/// Compute OpenUtau's singer directory for a given install directory, mirroring
/// OpenUtau's PathManager: installed builds store singers under
/// <Documents>\OpenUtau\Singers, portable builds under <install dir>\Singers.
fn singer_dir_for(install_dir: &Path) -> Result<PathBuf, BoxError> {
    if install_dir.exists() {
        let documents =
            dirs::document_dir().ok_or("Documentsフォルダの場所を特定できませんでした。")?;
        Ok(documents.join("OpenUtau").join("Singers"))
    } else {
        Ok(install_dir.join("Singers"))
    }
}

impl App for Kickstart {
    #[expect(unused)]
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| match self.state.page {
            0 => self.page_0(ui),
            1 => self.page_1(ui),
            _ => {
                ui.label(
                    RichText::new("このページはまだ実装されていません").color(egui::Color32::RED),
                );
                ui.button("目次に戻る")
                    .clicked()
                    .then(|| self.state.page = 1);
            }
        });
        egui::Panel::bottom("Controls").show_inside(ui, |ui| {
            ui.separator();
            ui.columns(2, |columns| {
                columns[0].with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let prev = ui.add_enabled_ui(self.state.page != 0, |ui| ui.button("戻る"));
                    if prev.inner.clicked() {
                        self.state.prev();
                    }
                });
                columns[1].with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    let next =
                        ui.add_enabled_ui(self.state.page != LAST_PAGE, |ui| ui.button("次へ"));
                    if next.inner.clicked() {
                        self.state.next();
                    }
                });
            });
        });
    }
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {}
    fn auto_save_interval(&self) -> std::time::Duration {
        Duration::from_mins(30)
    }
}

#[derive(Serialize, Deserialize, Default)]
struct KickstartData {
    data_dir: PathBuf,
    page: u32,
    max_page: u32,
    use_openutau_installer: bool,
    ou_dir: Option<PathBuf>,
    ou_singer_dir: Option<PathBuf>,
    recording: bool,
    recording_dir: Option<PathBuf>,
}

impl KickstartData {
    fn next(&mut self) -> bool {
        if self.page == LAST_PAGE {
            return false;
        }
        self.page += 1;
        if self.max_page < self.page {
            self.max_page = self.page
        };
        true
    }
    fn prev(&mut self) -> bool {
        if self.page == 0 {
            return false;
        }
        self.page -= 1;
        true
    }
}

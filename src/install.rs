use std::{
    env,
    fs::{self, File},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI8, Ordering},
    },
    thread,
};

use crate::BoxError;

pub struct OuInstall {
    pub install_dir: PathBuf,
    pub singer_dir: PathBuf,
}

pub enum InstallResult {
    #[cfg_attr(not(windows), allow(dead_code))]
    OuInstall(OuInstall),
    VlInstall,
    RsInstall,
    #[cfg_attr(not(feature = "include_zip"), allow(dead_code))]
    Huw,
    Oto(PathBuf),
}

/// A running install thread plus a shared progress handle the UI can poll.
pub struct InstallJob {
    pub handle: thread::JoinHandle<Result<InstallResult, BoxError>>,
    pub progress: Arc<Progress>,
}

#[derive(Default)]
pub struct Progress {
    pub stage: Mutex<String>,
    /// Progress between [0,100], or -1 if unknown.
    pub fraction: AtomicI8,
}

/// Set the stage text and fraction (`None` = unknown).
pub fn set_progress(progress: &Progress, stage: &str, fraction: Option<f32>) {
    if let Ok(mut guard) = progress.stage.lock() {
        *guard = stage.to_string();
    }
    set_fraction(progress, fraction);
}

fn set_fraction(progress: &Progress, fraction: Option<f32>) {
    let value = fraction.map_or(-1, |f| (f.clamp(0.0, 1.0) * 100.0) as i8);
    progress.fraction.store(value, Ordering::Relaxed);
}

/// Run `work` on a background thread with a fresh progress handle.
pub fn spawn_job(
    work: impl FnOnce(&Progress) -> Result<InstallResult, BoxError> + Send + 'static,
) -> InstallJob {
    let progress = Arc::new(Progress::default());
    let worker = progress.clone();
    InstallJob {
        handle: thread::spawn(move || work(&worker)),
        progress,
    }
}

pub fn download_with_progress(
    url: &str,
    dest: &Path,
    progress: &Progress,
    stage: &str,
) -> Result<(), BoxError> {
    set_progress(progress, stage, None);
    let client = reqwest::blocking::Client::builder().timeout(None).build()?;
    let mut resp = client.get(url).send()?.error_for_status()?;
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
        set_fraction(progress, total.map(|t| downloaded as f32 / t as f32));
    }
    Ok(())
}

pub fn extract_zip(
    reader: impl Read + Seek,
    dest: &Path,
    progress: &Progress,
) -> Result<(), BoxError> {
    let mut archive = zip::ZipArchive::new(reader)?;
    let count = archive.len();
    for i in 0..count {
        let mut entry = archive.by_index(i)?;
        // `enclosed_name` rejects path traversal (zip-slip); skip anything unsafe.
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut File::create(&out_path)?)?;
        }
        set_fraction(progress, Some((i + 1) as f32 / count as f32));
    }
    Ok(())
}

/// Download a zip and extract it into `dest`, skipping if `dest` already exists.
fn spawn_zip_setup(
    name: &'static str,
    url: &'static str,
    dest: PathBuf,
    done: InstallResult,
) -> InstallJob {
    spawn_job(move |progress| {
        if dest.exists() {
            return Ok(done);
        }
        let zip_path = env::temp_dir().join(format!("{name}.zip"));
        download_with_progress(
            url,
            &zip_path,
            progress,
            &format!("{name}をダウンロードしています..."),
        )?;
        set_progress(progress, &format!("{name}を展開しています..."), None);
        extract_zip(File::open(&zip_path)?, &dest, progress)?;
        let _ = fs::remove_file(&zip_path);
        Ok(done)
    })
}

pub fn recstar_setup(data_dir: &Path) -> InstallJob {
    spawn_zip_setup(
        "RecStar",
        "https://github.com/sdercolin/recstar/releases/download/1.2.0/recstar-1.2.0-win64.zip",
        data_dir.join("recstar"),
        InstallResult::RsInstall,
    )
}

pub fn vlabeler_setup(data_dir: &Path) -> InstallJob {
    spawn_zip_setup(
        "vLabeler",
        "https://github.com/sdercolin/vlabeler/releases/download/1.5.0/vlabeler-1.5.0-win64.zip",
        data_dir.join("vlabeler"),
        InstallResult::VlInstall,
    )
}

#[cfg(windows)]
pub mod win {
    use std::{
        env,
        fs::{self, File},
        path::{Path, PathBuf},
        process::Command,
    };

    use super::{
        InstallJob, InstallResult, OuInstall, Progress, download_with_progress, extract_zip,
        set_progress, spawn_job,
    };
    use crate::BoxError;

    /// Host architecture as the suffix used by OpenUtau / .NET downloads.
    fn arch() -> Result<&'static str, BoxError> {
        match env::consts::ARCH {
            "x86_64" => Ok("x64"),
            "aarch64" => Ok("arm64"),
            "x86" => Ok("x86"),
            other => Err(format!("未対応のアーキテクチャです: {other}").into()),
        }
    }

    fn start_menu_programs_dir() -> PathBuf {
        PathBuf::from(env::var("APPDATA").unwrap_or_default())
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
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

    fn ensure_dotnet8(progress: &Progress) -> Result<(), BoxError> {
        if dotnet8_desktop_present() {
            return Ok(());
        }
        let arch = arch()?;
        let url = format!("https://aka.ms/dotnet/8.0/windowsdesktop-runtime-win-{arch}.exe");
        let installer = env::temp_dir().join("windowsdesktop-runtime-8.exe");
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
        let _ = fs::remove_file(&installer);
        Ok(())
    }

    fn create_start_menu_shortcut(target: &Path, lnk_path: &Path) -> Result<(), BoxError> {
        let mut link = mslnk::ShellLink::new(target)
            .map_err(|e| format!("ショートカットの作成に失敗しました: {e:?}"))?;
        if let Some(parent) = target.parent() {
            link.set_working_dir(Some(parent.to_string_lossy().into_owned()));
        }
        link.set_name(Some("OpenUtau".to_string()));
        link.create_lnk(lnk_path)
            .map_err(|e| format!("ショートカットの書き込みに失敗しました: {e:?}"))?;
        Ok(())
    }

    /// Mirrors OpenUtau's PathManager: installed builds (with `installed.txt`
    /// next to the exe) keep singers under <Documents>\OpenUtau\Singers,
    /// portable builds under <install dir>\Singers.
    fn singer_dir_for(install_dir: &Path) -> Result<PathBuf, BoxError> {
        if install_dir.join("installed.txt").exists() {
            let documents =
                dirs::document_dir().ok_or("Documentsフォルダの場所を特定できませんでした。")?;
            Ok(documents.join("OpenUtau").join("Singers"))
        } else {
            Ok(install_dir.join("Singers"))
        }
    }

    /// Run OpenUtau's official installer, then locate the install and singer
    /// directories through its Start Menu shortcut.
    pub fn ou_installer() -> InstallJob {
        spawn_job(|progress| {
            let installer_path = env::temp_dir().join("OpenUtauInstaller.exe");
            let url = format!(
                "https://github.com/openutau/OpenUtau/releases/latest/download/OpenUtau-win-{}.exe",
                arch()?
            );
            download_with_progress(
                &url,
                &installer_path,
                progress,
                "インストーラをダウンロードしています...",
            )?;
            set_progress(progress, "インストーラを実行しています...", None);
            Command::new(&installer_path).spawn()?.wait()?;

            set_progress(progress, "インストール先を確認しています...", None);
            let start_menu = start_menu_programs_dir().join("OpenUtau.lnk");
            if !start_menu.exists() {
                return Err("OpenUtauのインストールに失敗しました。スタートメニューのショートカットが見つかりませんでした。".into());
            }
            let lnk = parselnk::Lnk::try_from(start_menu.as_path())?;
            // Prefer the working directory; fall back to the target's parent.
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
            let singer_dir = singer_dir_for(&install_dir)?;
            // OpenUtau creates the singer directory lazily.
            fs::create_dir_all(&singer_dir)?;
            Ok(InstallResult::OuInstall(OuInstall {
                install_dir,
                singer_dir,
            }))
        })
    }

    /// Install the portable build into `dest`, ensure .NET 8, and register a
    /// Start Menu entry.
    pub fn portable_install(dest: PathBuf) -> InstallJob {
        spawn_job(move |progress| {
            fs::create_dir_all(&dest)?;
            let url = format!(
                "https://github.com/openutau/OpenUtau/releases/latest/download/OpenUtau-win-{}.zip",
                arch()?
            );
            let zip_path = env::temp_dir().join("OpenUtau-portable.zip");
            download_with_progress(
                &url,
                &zip_path,
                progress,
                "OpenUtauをダウンロードしています...",
            )?;

            set_progress(progress, "OpenUtauを展開しています...", None);
            extract_zip(File::open(&zip_path)?, &dest, progress)?;
            let _ = fs::remove_file(&zip_path);

            set_progress(progress, ".NET 8 ランタイムを確認しています...", None);
            ensure_dotnet8(progress)?;

            set_progress(progress, "スタートメニューに登録しています...", None);
            let exe = dest.join("OpenUtau.exe");
            if !exe.exists() {
                return Err("展開後にOpenUtau.exeが見つかりませんでした。".into());
            }
            let start_menu_dir = start_menu_programs_dir();
            fs::create_dir_all(&start_menu_dir)?;
            create_start_menu_shortcut(&exe, &start_menu_dir.join("OpenUtau.lnk"))?;

            let singer_dir = dest.join("Singers");
            fs::create_dir_all(&singer_dir)?;

            set_progress(progress, "完了しました。", Some(1.0));
            Ok(InstallResult::OuInstall(OuInstall {
                install_dir: dest,
                singer_dir,
            }))
        })
    }
}

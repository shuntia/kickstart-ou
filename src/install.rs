use std::{
    env,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
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
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    OuInstall(OuInstall),
    VlInstall,
    RsInstall,
    #[cfg_attr(not(feature = "include_zip"), allow(dead_code))]
    Huw,
    Oto(PathBuf),
    Package(PathBuf),
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

/// Zip every file under `src` into `dest`, each entry named `root` + its path relative to `src`.
/// `extra` entries go at the archive root as-is (e.g. UTAU's install.txt).
/// With `sjis`, names are stored as Shift_JIS without the UTF-8 flag, which UTAU reads as-is.
pub fn zip_dir(
    src: &Path,
    dest: &Path,
    root: &str,
    extra: &[(&str, &[u8])],
    sjis: bool,
) -> Result<(), BoxError> {
    let encode = |name: String| -> Result<Vec<u8>, BoxError> {
        if !sjis {
            return Ok(name.into_bytes());
        }
        let (bytes, _, unmappable) = encoding_rs::SHIFT_JIS.encode(&name);
        if unmappable {
            return Err(
                format!("Shift_JISで表せない文字がファイル名に含まれています: {name}").into(),
            );
        }
        Ok(bytes.into_owned())
    };
    let options = zip::write::SimpleFileOptions::default();
    let mut zip = zip::ZipWriter::new(File::create(dest)?);
    // `dest` may sit inside `src`; don't archive the archive.
    let dest_abs = fs::canonicalize(dest)?;
    for (name, data) in extra {
        zip.start_file_with_options(options.name(&encode(name.to_string())?))?;
        zip.write_all(data)?;
    }
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if fs::canonicalize(&path)? == dest_abs {
                continue;
            }
            let rel = path.strip_prefix(src)?.to_string_lossy().replace('\\', "/");
            zip.start_file_with_options(options.name(&encode(format!("{root}{rel}"))?))?;
            io::copy(&mut File::open(&path)?, &mut zip)?;
        }
    }
    zip.finish()?;
    if sjis {
        clear_utf8_flags(dest)?;
    }
    Ok(())
}

/// The zip crate sets the UTF-8 flag (bit 11) on any name whose bytes happen to be valid UTF-8,
/// which some Shift_JIS names are. Clear it in every local and central header so readers fall
/// back to the system codepage. CRCs and sizes don't cover the flags, so this is safe.
fn clear_utf8_flags(path: &Path) -> Result<(), BoxError> {
    let mut f = fs::OpenOptions::new().read(true).write(true).open(path)?;
    let offsets = {
        let mut archive = zip::ZipArchive::new(&mut f)?;
        (0..archive.len())
            .map(|i| {
                let e = archive.by_index_raw(i)?;
                Ok([e.header_start() + 6, e.central_header_start() + 8])
            })
            .collect::<Result<Vec<_>, zip::result::ZipError>>()?
    };
    for pos in offsets.into_iter().flatten() {
        let mut b = [0u8; 2];
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut b)?;
        f.seek(SeekFrom::Start(pos))?;
        f.write_all(&(u16::from_le_bytes(b) & !0x0800).to_le_bytes())?;
    }
    Ok(())
}

/// Host architecture as the suffix used by OpenUtau / .NET downloads.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
fn arch() -> Result<&'static str, BoxError> {
    match env::consts::ARCH {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("arm64"),
        "x86" => Ok("x86"),
        other => Err(format!("未対応のアーキテクチャです: {other}").into()),
    }
}

/// Path to an app's executable inside the folder `app_setup` unpacked it into.
pub fn app_exe(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join("bin").join(name)
    }
}

/// Unpack a .deb's payload into `dest`, dropping the `./opt/<app>/` prefix the
/// jpackage builds install under.
fn extract_deb(deb: &Path, dest: &Path, progress: &Progress) -> Result<(), BoxError> {
    let mut archive = ar::Archive::new(File::open(deb)?);
    while let Some(entry) = archive.next_entry() {
        let entry = entry?;
        if entry.header().identifier() != b"data.tar.zst" {
            continue;
        }
        let total = entry.header().size();
        let reader = CountingReader {
            inner: entry,
            read: 0,
            total,
            progress,
        };
        let mut tar = tar::Archive::new(zstd::Decoder::new(reader)?);
        for file in tar.entries()? {
            let mut file = file?;
            let path = file.path()?.into_owned();
            let rel: PathBuf = path
                .components()
                .filter(|c| *c != Component::CurDir)
                .skip(2)
                .collect();
            if rel.as_os_str().is_empty()
                || rel.components().any(|c| !matches!(c, Component::Normal(_)))
            {
                continue;
            }
            let out = dest.join(rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            file.unpack(&out)?;
        }
        return Ok(());
    }
    Err(".debにdata.tar.zstが見つかりませんでした。".into())
}

/// Reports how far through the compressed stream extraction has read.
struct CountingReader<'a, R> {
    inner: R,
    read: u64,
    total: u64,
    progress: &'a Progress,
}

impl<R: Read> Read for CountingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        set_fraction(self.progress, Some(self.read as f32 / self.total as f32));
        Ok(n)
    }
}

/// Download `url` and unpack it into `dest`, skipping if `dest` already exists.
fn app_setup(
    name: &'static str,
    url: &'static str,
    dest: PathBuf,
    done: InstallResult,
) -> InstallJob {
    spawn_job(move |progress| {
        if dest.exists() {
            return Ok(done);
        }
        let archive = env::temp_dir().join(url.rsplit('/').next().unwrap_or(name));
        download_with_progress(
            url,
            &archive,
            progress,
            &format!("{name}をダウンロードしています..."),
        )?;
        set_progress(progress, &format!("{name}を展開しています..."), None);
        let result = if url.ends_with(".deb") {
            extract_deb(&archive, &dest, progress)
        } else {
            extract_zip(File::open(&archive)?, &dest, progress)
        };
        let _ = fs::remove_file(&archive);
        if result.is_err() {
            let _ = fs::remove_dir_all(&dest);
        }
        result.map(|()| done)
    })
}

pub fn recstar_setup(data_dir: &Path) -> InstallJob {
    app_setup(
        "RecStar",
        if cfg!(windows) {
            "https://github.com/sdercolin/recstar/releases/download/1.2.0/recstar-1.2.0-win64.zip"
        } else {
            "https://github.com/sdercolin/recstar/releases/download/1.2.0/recstar-1.2.0-amd64.deb"
        },
        data_dir.join("recstar"),
        InstallResult::RsInstall,
    )
}

pub fn vlabeler_setup(data_dir: &Path) -> InstallJob {
    app_setup(
        "vLabeler",
        if cfg!(windows) {
            "https://github.com/sdercolin/vlabeler/releases/download/1.5.0/vlabeler-1.5.0-win64.zip"
        } else {
            "https://github.com/sdercolin/vlabeler/releases/download/1.5.0/vlabeler-1.5.0-amd64.deb"
        },
        data_dir.join("vlabeler"),
        InstallResult::VlInstall,
    )
}

#[cfg(target_os = "linux")]
pub use linux::portable_install;
#[cfg(windows)]
pub use win::portable_install;

#[cfg(target_os = "linux")]
mod linux {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use super::{InstallJob, InstallResult, OuInstall, arch, download_with_progress, spawn_job};

    /// Download the OpenUtau AppImage into `dest` and add a desktop entry.
    pub fn portable_install(dest: PathBuf) -> InstallJob {
        spawn_job(move |progress| {
            fs::create_dir_all(&dest)?;
            let exe = dest.join("OpenUtau.AppImage");
            let url = format!(
                "https://github.com/openutau/OpenUtau/releases/latest/download/OpenUtau-linux-{}.AppImage",
                arch()?
            );
            download_with_progress(&url, &exe, progress, "OpenUtauをダウンロードしています...")?;
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))?;

            let data = dirs::data_dir().ok_or("データフォルダの場所を特定できませんでした。")?;
            let apps = data.join("applications");
            fs::create_dir_all(&apps)?;
            fs::write(
                apps.join("OpenUtau.desktop"),
                format!(
                    "[Desktop Entry]\nType=Application\nName=OpenUtau\nExec=\"{}\"\nCategories=AudioVideo;Audio;\n",
                    exe.display()
                ),
            )?;

            let singer_dir = data.join("OpenUtau").join("Singers");
            fs::create_dir_all(&singer_dir)?;
            Ok(InstallResult::OuInstall(OuInstall {
                install_dir: dest,
                singer_dir,
            }))
        })
    }
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
        InstallJob, InstallResult, OuInstall, Progress, arch, download_with_progress, extract_zip,
        set_progress, spawn_job,
    };
    use crate::BoxError;

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

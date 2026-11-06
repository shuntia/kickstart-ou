// No console window behind the GUI on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod otoini;
mod pages;

use std::{
    env::home_dir,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use eframe::egui::{self, FontData, FontDefinitions, Margin, Style, Ui};
use eframe::{App, CreationContext, NativeOptions};
use rfd::{MessageDialog, MessageLevel};
use serde::{Deserialize, Serialize};

use install::{InstallJob, InstallResult};
use pages::PAGES;

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

const OTOINI_TOKEN: Option<&str> = option_env!("OTOINI_TOKEN");
const DISPLAY_TIMEOUT: Duration = Duration::from_secs(5);

fn main() {
    if !cfg!(any(windows, target_os = "linux")) {
        MessageDialog::new()
            .set_title(":/")
            .set_description("このソフトはWindowsとLinux用です。MacOSの動作は保証されません。")
            .set_level(MessageLevel::Warning)
            .show();
    }
    if let Err(e) = eframe::run_native(
        "Kickstart UTAU!",
        NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(Kickstart::new(cc)))),
    ) {
        MessageDialog::new()
            .set_description(format!(
                "エラーが発生しました。開発者に報告してください。\n{e:?}"
            ))
            .set_title(":(")
            .set_level(MessageLevel::Error)
            .show();
    }
}

#[derive(Default)]
pub struct Kickstart {
    state: KickstartData,
    save_path: PathBuf,
    recstar_state: RsState,
    ou_install: Option<InstallJob>,
    vl_install: Option<InstallJob>,
    rs_install: Option<InstallJob>,
    oto_job: Option<InstallJob>,
    huw_job: Option<InstallJob>,
    pkg_job: Option<InstallJob>,
    install_error: Option<(String, Instant)>,
    success_message: Option<(String, Instant)>,
}

#[derive(Default, PartialEq)]
enum RsState {
    /// Sessions found, plus the one the user has picked but not yet moved.
    Listed(Vec<PathBuf>, Option<PathBuf>),
    Failed(String),
    Selected(PathBuf),
    Moved(PathBuf),
    #[default]
    Waiting,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct PackageData {
    readme: String,
    name: String,
    author: String,
    voice: String,
    /// Absolute here; made relative to the package root when written out.
    icon_path: Option<PathBuf>,
    /// Absolute here; made relative to the package root when written out.
    bg_path: Option<PathBuf>,
    version: String,
    web: String,
    /// NOTE: DO NOT expose to user
    encoding: String,
    portrait_opacity: f64,
    portrait_height: u32,
    en_us_localized: String,
}

impl Default for PackageData {
    fn default() -> Self {
        Self {
            readme: String::new(),
            name: String::new(),
            author: String::new(),
            voice: String::new(),
            icon_path: None,
            bg_path: None,
            version: "0.0.0".into(),
            web: String::new(),
            encoding: "Shift-JIS".into(),
            portrait_opacity: 0.8,
            portrait_height: 0,
            en_us_localized: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CharacterTxt {
    name: String,
    image: String,
    author: Option<String>,
    web: Option<String>,
}

impl From<PackageData> for CharacterTxt {
    fn from(pkg: PackageData) -> Self {
        Self {
            name: pkg.name,
            image: pkg
                .icon_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            author: pkg.author.is_empty().then_some(pkg.author),
            web: pkg.web.is_empty().then_some(pkg.web),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CharacterYaml {
    name: String,
    text_file_encoding: String,
    singer_type: String,
    author: Option<String>,
    voice: Option<String>,
    version: Option<String>,
    web: Option<String>,
    portrait: Option<PathBuf>,
    portrait_opacity: f64,
    portrait_height: u32,
    default_phonemizer: String,
    localized_names: Option<LocalizedNames>,
}

#[derive(Serialize, Deserialize)]
struct LocalizedNames {
    #[serde(rename = "en-US")]
    en_us: Option<String>,
}

impl From<PackageData> for CharacterYaml {
    fn from(pkg: PackageData) -> Self {
        Self {
            name: pkg.name,
            text_file_encoding: pkg.encoding,
            singer_type: "utau".into(),
            author: pkg.author.is_empty().then_some(pkg.author),
            voice: pkg.voice.is_empty().then_some(pkg.voice),
            version: pkg.version.is_empty().then_some(pkg.version),
            web: pkg.web.is_empty().then_some(pkg.web),
            portrait: pkg.bg_path,
            portrait_opacity: pkg.portrait_opacity,
            portrait_height: pkg.portrait_height,
            default_phonemizer: "OpenUtau.Plugin.Builtin.JapanesePresampPhonemizer".into(),
            localized_names: pkg.en_us_localized.is_empty().then_some(LocalizedNames {
                en_us: Some(pkg.en_us_localized),
            }),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct KickstartData {
    installed_to_ou: bool,
    proj_dir: Option<PathBuf>,
    data_dir: PathBuf,
    page: u32,
    use_openutau_installer: bool,
    ou_dir: Option<PathBuf>,
    ou_singer_dir: Option<PathBuf>,
    rs_installed: bool,
    vl_installed: bool,
    otoini_url: String,
    otoini_token: String,
    oto_result_dir: Option<PathBuf>,
    package_data: PackageData,
}

impl Default for KickstartData {
    fn default() -> Self {
        Self {
            proj_dir: None,
            data_dir: home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("kickstart"),
            page: 0,
            use_openutau_installer: false,
            ou_dir: None,
            ou_singer_dir: None,
            rs_installed: false,
            vl_installed: false,
            otoini_url: "https://otoini.shuntia.net".into(),
            otoini_token: OTOINI_TOKEN.unwrap_or_default().into(),
            oto_result_dir: None,
            package_data: PackageData::default(),
            installed_to_ou: false,
        }
    }
}

impl KickstartData {
    fn next(&mut self) {
        if self.page + 1 < PAGES.len() as u32 {
            self.page += 1;
        }
    }

    fn prev(&mut self) {
        self.page = self.page.saturating_sub(1);
    }
}

impl Kickstart {
    fn new(cc: &CreationContext<'_>) -> Self {
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "M PLUS 2".to_owned(),
            Arc::new(FontData::from_static(include_bytes!(
                "../fonts/MPLUS2-VariableFont_wght.ttf"
            ))),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "M PLUS 2".to_owned());
        cc.egui_ctx.set_fonts(fonts);
        cc.egui_ctx.set_global_style(Style {
            text_styles: [
                (
                    egui::TextStyle::Heading,
                    egui::FontId::new(30.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Body,
                    egui::FontId::new(20.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Monospace,
                    egui::FontId::new(20.0, egui::FontFamily::Monospace),
                ),
                (
                    egui::TextStyle::Button,
                    egui::FontId::new(20.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Small,
                    egui::FontId::new(15.0, egui::FontFamily::Proportional),
                ),
            ]
            .into(),
            visuals: egui::Visuals {
                text_edit_bg_color: Some(egui::Color32::from_gray(40)),
                ..egui::Visuals::dark()
            },
            ..Default::default()
        });
        let save_path = KickstartData::default().data_dir.join("state.json");
        let state = std::fs::read_to_string(&save_path)
            .ok()
            .and_then(|buf| serde_json::from_str(&buf).ok())
            .unwrap_or_default();
        Self {
            save_path,
            state,
            ..Default::default()
        }
    }

    fn save_state(&self) {
        if let Some(parent) = self.save_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.state) {
            let _ = std::fs::write(&self.save_path, json);
        }
    }

    /// Show progress for running jobs and record the result of finished ones.
    fn poll_jobs(&mut self, ui: &mut Ui) {
        for slot in [
            &mut self.ou_install,
            &mut self.rs_install,
            &mut self.vl_install,
            &mut self.oto_job,
            &mut self.huw_job,
            &mut self.pkg_job,
        ] {
            let Some(job) = slot.take_if(|job| job.handle.is_finished()) else {
                if let Some(job) = slot {
                    if let Ok(guard) = job.progress.stage.lock() {
                        ui.label(guard.as_str());
                    }
                    let fraction = job.progress.fraction.load(Ordering::Relaxed);
                    let bar = egui::ProgressBar::new(fraction.max(0) as f32 / 100.);
                    ui.add(if fraction < 0 {
                        bar.animate(true)
                    } else {
                        bar.show_percentage()
                    });
                    // Keep repainting so worker progress shows up.
                    ui.ctx().request_repaint();
                }
                continue;
            };
            let msg = match job.handle.join() {
                Ok(Ok(InstallResult::OuInstall(ou))) => {
                    self.state.ou_dir = Some(ou.install_dir);
                    self.state.ou_singer_dir = Some(ou.singer_dir);
                    "OpenUtauのインストールに成功しました。"
                }
                Ok(Ok(InstallResult::RsInstall)) => {
                    self.state.rs_installed = true;
                    "RecStarの初期設定に成功しました。"
                }
                Ok(Ok(InstallResult::VlInstall)) => {
                    self.state.vl_installed = true;
                    "vLabelerの初期設定に成功しました。"
                }
                Ok(Ok(InstallResult::Oto(dir))) => {
                    // Package the auto-labelled voicebank from here on.
                    self.state.proj_dir = Some(dir.clone());
                    self.state.oto_result_dir = Some(dir);
                    "自動原音設定に成功しました。"
                }
                Ok(Ok(InstallResult::Huw)) => "Hello UTAU Worldの解凍に成功しました。",
                Ok(Ok(InstallResult::Package(path))) => {
                    self.success_message = Some((
                        format!("音源のzipを書き出しました: {}", path.display()),
                        Instant::now(),
                    ));
                    continue;
                }
                Ok(Err(e)) => {
                    self.install_error = Some((e.to_string(), Instant::now()));
                    continue;
                }
                Err(_) => {
                    self.install_error = Some((
                        "インストール中にスレッドがパニックしました。手動インストールをおすすめします。"
                            .into(),
                        Instant::now(),
                    ));
                    continue;
                }
            };
            self.success_message = Some((msg.into(), Instant::now()));
        }
    }
}

/// Show `slot`'s message until it is older than `DISPLAY_TIMEOUT`, then clear it.
fn show_timed(
    ui: &mut Ui,
    slot: &mut Option<(String, Instant)>,
    color: egui::Color32,
    prefix: &str,
) {
    match slot {
        Some((msg, time)) if time.elapsed() < DISPLAY_TIMEOUT => {
            ui.colored_label(color, format!("{prefix}{msg}"));
            // Wake up again so the message disappears without user input.
            ui.ctx()
                .request_repaint_after(DISPLAY_TIMEOUT - time.elapsed());
        }
        _ => *slot = None,
    }
}

impl App for Kickstart {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("Controls").show(ui, |ui| {
            self.poll_jobs(ui);
            show_timed(ui, &mut self.install_error, egui::Color32::RED, "エラー: ");
            show_timed(ui, &mut self.success_message, egui::Color32::GREEN, "");
            ui.separator();
            ui.columns(3, |columns| {
                columns[0].with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let prev = ui.add_enabled_ui(self.state.page != 0, |ui| ui.button("戻る"));
                    if prev.inner.clicked() {
                        self.state.prev();
                    }
                });
                // Not `centered_and_justified`: justify fills the panel's remembered
                // height, so the bar would never shrink back after a message.
                columns[1].vertical_centered(|ui| {
                    ui.label(format!("ページ {}/{}", self.state.page + 1, PAGES.len()));
                });
                columns[2].with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    let next = ui.add_enabled_ui(self.state.page != PAGES.len() as u32 - 1, |ui| {
                        ui.button("次へ")
                    });
                    if next.inner.clicked() {
                        self.state.next();
                    }
                });
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame {
                inner_margin: Margin::same(20),
                ..Default::default()
            })
            .show(ui, |ui| {
                if let Some((_, page_fn)) = PAGES.get(self.state.page as usize) {
                    if let Err(e) = page_fn(self, ui) {
                        self.install_error = Some((e.to_string(), Instant::now()));
                    }
                } else {
                    ui.label("不明なページです。");
                }
            });
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_state();
    }

    fn auto_save_interval(&self) -> Duration {
        Duration::from_mins(5)
    }
}

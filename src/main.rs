mod install;
mod otoini;
mod pages;

use std::{
    env::home_dir,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use eframe::egui::{self, FontData, FontDefinitions, Margin, Style, Ui};
use eframe::{App, CreationContext, NativeOptions};
use rfd::{MessageDialog, MessageLevel};
use serde::{Deserialize, Serialize};

use install::{InstallJob, InstallResult};
use pages::PAGES;

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

const KICKSTART_OTO_TOKEN: Option<&str> = option_env!("KICKSTART_OTO_TOKEN");

fn main() {
    if !cfg!(windows) {
        MessageDialog::new()
            .set_title(":/")
            .set_description("このソフトはWindows専用です。LinuxやMacOSの動作は保証されません。")
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
    install_error: Option<String>,
}

#[derive(Default, PartialEq)]
enum RsState {
    Listed(Vec<PathBuf>),
    Failed(String),
    Selected(PathBuf),
    Moved(PathBuf),
    #[default]
    Waiting,
}

#[derive(Serialize, Deserialize)]
struct KickstartData {
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
}

impl Default for KickstartData {
    fn default() -> Self {
        Self {
            proj_dir: None,
            data_dir: home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".kickstart"),
            page: 0,
            use_openutau_installer: true,
            ou_dir: None,
            ou_singer_dir: None,
            rs_installed: false,
            vl_installed: false,
            otoini_url: "https://otoini.shuntia.net".into(),
            otoini_token: KICKSTART_OTO_TOKEN.unwrap_or_default().into(),
            oto_result_dir: None,
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

    /// Show progress for running jobs and record the result of finished ones.
    fn poll_jobs(&mut self, ui: &mut Ui) {
        for slot in [
            &mut self.ou_install,
            &mut self.rs_install,
            &mut self.vl_install,
            &mut self.oto_job,
            &mut self.huw_job,
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
            match job.handle.join() {
                Ok(Ok(InstallResult::OuInstall(ou))) => {
                    self.state.ou_dir = Some(ou.install_dir);
                    self.state.ou_singer_dir = Some(ou.singer_dir);
                }
                Ok(Ok(InstallResult::RsInstall)) => self.state.rs_installed = true,
                Ok(Ok(InstallResult::VlInstall)) => self.state.vl_installed = true,
                Ok(Ok(InstallResult::Oto(dir))) => self.state.oto_result_dir = Some(dir),
                Ok(Ok(InstallResult::Huw)) => {}
                Ok(Err(e)) => self.install_error = Some(e.to_string()),
                Err(_) => {
                    self.install_error = Some(
                        "インストール中にスレッドがパニックしました。手動インストールをおすすめします。"
                            .into(),
                    );
                }
            }
        }
    }
}

impl App for Kickstart {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("Controls").show(ui, |ui| {
            self.poll_jobs(ui);
            if let Some(err) = &self.install_error {
                ui.colored_label(egui::Color32::RED, format!("エラー: {err}"));
            }
            ui.separator();
            ui.columns(3, |columns| {
                columns[0].with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let prev = ui.add_enabled_ui(self.state.page != 0, |ui| ui.button("戻る"));
                    if prev.inner.clicked() {
                        self.state.prev();
                    }
                });
                columns[1].with_layout(
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        ui.label(format!("ページ {}/{}", self.state.page + 1, PAGES.len()));
                    },
                );
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
                if let Some(page_fn) = PAGES.get(self.state.page as usize) {
                    page_fn(self, ui);
                } else {
                    ui.label("不明なページです。");
                }
            });
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        if let Some(parent) = self.save_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.state) {
            let _ = std::fs::write(&self.save_path, json);
        }
    }

    fn auto_save_interval(&self) -> Duration {
        Duration::from_mins(5)
    }
}

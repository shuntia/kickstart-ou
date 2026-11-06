use std::{
    env::home_dir,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::{self, Command},
    time::Instant,
};

use eframe::egui::{self, Color32, RichText, Ui};
use encoding_rs::SHIFT_JIS;

use crate::{CharacterTxt, CharacterYaml, Kickstart, RsState, install, otoini};

#[cfg(feature = "include_zip")]
const HELLO_UTAU_ZIP: &[u8] = include_bytes!("../Hello UTAU World!_v111.zip");

pub type PageResult = Result<(), Box<dyn Error>>;
type PageFn = fn(&mut Kickstart, &mut Ui) -> PageResult;

/// Every page with its table-of-contents title, in order.
pub const PAGES: &[(&str, PageFn)] = &[
    ("はじめに", Kickstart::intro),
    ("目次", Kickstart::toc),
    ("必要なもの", Kickstart::requirements),
    ("OpenUtauの初期設定", Kickstart::openutau_setup),
    ("録音・原音設定ソフトの初期設定", Kickstart::tools_setup),
    ("Hello UTAU Worldの解凍", Kickstart::huw_extract),
    ("録音ソフトの起動", Kickstart::launch_recstar),
    ("RecStarの使い方", Kickstart::recstar_howto),
    ("収録環境", Kickstart::environment),
    ("収録", Kickstart::recording),
    ("vLabelerの下準備", Kickstart::move_session),
    ("自動原音設定", Kickstart::auto_oto),
    ("原音設定", Kickstart::label),
    ("音源情報", Kickstart::information),
    ("音源テスト", Kickstart::testing),
    ("パッケージング", Kickstart::packaging_and_install),
    ("おわりに", Kickstart::end),
];

impl Kickstart {
    fn intro(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("このソフトは自動でUTAUのボイスバンクを作る補助をするソフトです。\n基本的にはUTAU音源制作Wikiの内容に沿っています。早速右下の「次へ」をクリックして進みましょう！");
        ui.hyperlink_to(
            "参考：UTAU音源制作Wikiの元記事",
            "https://w.atwiki.jp/vbmaker/pages/43.html",
        );
        Ok(())
    }

    fn toc(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("目次");
        ui.separator();
        for (i, (title, _)) in PAGES.iter().enumerate() {
            if ui.link(format!("{}. {title}", i + 1)).clicked() {
                self.state.page = i as u32;
            }
        }
        Ok(())
    }

    fn requirements(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("必要なもの");
        ui.separator();
        ui.label("- パソコン(Windows / Linux)");
        ui.label("- マイク、オーディオインターフェースなどの録音機材");
        ui.hyperlink_to("→詳細", "https://w.atwiki.jp/vbmaker/pages/14.html");
        ui.label("- 収録環境");
        ui.label("録音、原音設定、調声のソフトは自動でインストールされます。");
        Ok(())
    }

    fn openutau_setup(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("データの初期設定");
        #[cfg(windows)]
        {
            ui.label("このソフトは全て管理して最小限で自動インストールすることができますが、OpenUtau公式のインストーラを使用しますか？");
            ui.radio_value(
                &mut self.state.use_openutau_installer,
                false,
                "いいえ(推奨)",
            );
            ui.radio_value(&mut self.state.use_openutau_installer, true, "はい");
        }

        // poll_jobs shows progress while installing.
        if self.ou_install.is_some() {
            return Ok(());
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
            return Ok(());
        }

        #[cfg(windows)]
        if self.state.use_openutau_installer {
            ui.label("OpenUtau公式のインストーラをダウンロードします。よろしいですか？");
            if ui.button("はい").clicked() {
                self.install_error = None;
                self.ou_install = Some(install::win::ou_installer());
            }
            return Ok(());
        }

        #[cfg(any(windows, target_os = "linux"))]
        {
            let default_dir = home_dir().unwrap_or_default().join("OpenUtau");
            let dest = self.state.ou_dir.clone().unwrap_or(default_dir);
            ui.label("OpenUtauのポータブル版をインストールする場所を指定してください。");
            ui.label(format!("インストール先: {}", dest.display()));
            ui.horizontal(|ui| {
                if ui.button("フォルダを選択").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_directory(dest.parent().unwrap_or(&dest))
                        .pick_folder()
                {
                    self.state.ou_dir = Some(path);
                }
                if ui.button("インストール開始").clicked() {
                    self.install_error = None;
                    self.state.ou_dir = Some(dest.clone());
                    self.ou_install = Some(install::portable_install(dest.clone()));
                }
            });
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            ui.label("Linuxの場合はディストロに応じた方法でOpenUtauをインストールしてください。インストール後、OpenUtauの実行ファイルと音源フォルダの場所を入力してください。");
            if ui.button("インストール完了").clicked() {
                match Command::new("which").arg("openutau").output() {
                    Ok(out) if out.status.success() => {
                        let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                        self.success_message =
                            Some((format!("OpenUtauが見つかりました: {path}"), Instant::now()));
                        self.state.ou_dir = Some(path.into());
                    }
                    _ => {
                        self.install_error = Some((
                            "システムのPATHにOpenUtauが見つかりませんでした。必要無い、又は既にインストールされている場合は無視しても大丈夫です。".into(),
                            Instant::now(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn tools_setup(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("録音・原音設定ソフトの初期設定");
        ui.label("このソフトはRecStar及びvLabelerを使用します。");
        ui.label("RecStarは録音ソフト、vLabelerは原音設定ソフトです。");
        ui.label("RecStarは1.2.0、vLabelerは1.5.0を使用します。最新版を使用する場合は手動でインストールしてください。別のバージョンではソフトの動作が異なる場合があります。");
        let busy = self.rs_install.is_some() || self.vl_install.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let rs = ui.button("RecStarの初期設定").clicked();
            let vl = ui.button("vLabelerの初期設定").clicked();
            let both = ui.button("両方インストールする").clicked();
            if rs || vl || both {
                self.install_error = None;
            }
            if rs || both {
                self.rs_install = Some(install::recstar_setup(&self.state.data_dir));
            }
            if vl || both {
                self.vl_install = Some(install::vlabeler_setup(&self.state.data_dir));
            }
        });
        if self.state.vl_installed && self.state.rs_installed {
            ui.label("初期設定が完了しました。次に進んで下さい。");
        }
        Ok(())
    }

    fn huw_extract(&mut self, ui: &mut Ui) -> PageResult {
        ui.label(RichText::new("Hello UTAU Worldを解凍する").size(20.));
        ui.label("こちらのファイルは特別な許諾を得て配布されているデータです。解凍されたデータの再配布は禁止されています。");
        // dest exists mid-extraction, so check the job first.
        if self.huw_job.is_some() {
            return Ok(());
        }
        let dest = self.state.data_dir.clone();
        if dest.exists() {
            ui.label(format!(
                "Hello UTAU Worldは解凍済みです。次に進んでください: {}",
                dest.display()
            ));
            return Ok(());
        }
        #[cfg(feature = "include_zip")]
        if ui.button("Hello UTAU Worldを解凍する").clicked() {
            self.install_error = None;
            self.huw_job = Some(install::spawn_job(move |progress| {
                install::set_progress(progress, "Hello UTAU Worldを解凍しています...", None);
                let reader = std::io::Cursor::new(HELLO_UTAU_ZIP);
                if let Err(e) = install::extract_zip(reader, &dest, progress) {
                    // Remove the partial folder so the button reappears for a retry.
                    let _ = fs::remove_dir_all(&dest.join("Hello UTAU World!"));
                    return Err(format!("Hello UTAU Worldの解凍に失敗しました: {e}").into());
                }
                Ok(install::InstallResult::Huw)
            }));
        }
        Ok(())
    }

    fn launch_recstar(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("録音しよう");
        ui.label("では、早速録音をはじめましょう！下のボタンを押すと録音ソフトが起動します。");
        if ui.button("録音ソフトを起動する").clicked() {
            let recstar_path = install::app_exe(&self.state.data_dir.join("recstar"), "RecStar");
            let err = if !recstar_path.exists() {
                Some("RecStarが見つかりませんでした。RecStarの初期設定を行ってください。".into())
            } else if let Err(e) = Command::new(&recstar_path).spawn() {
                Some(format!(
                    "録音ソフトの起動に失敗しました: {e}\nRecStarを手動で起動して録音してください。"
                ))
            } else {
                self.success_message = Some(("録音ソフトを起動しました。".into(), Instant::now()));
                None
            };
            self.install_error = err.map(|e| (e, Instant::now()));
        }
        Ok(())
    }

    fn recstar_howto(&mut self, ui: &mut Ui) -> PageResult {
        let huw = self.huw_dir();
        ui.heading("RecStarの使い方");
        ui.label(r#" ① 起動したらまず、連続録音モードにします。
これは録音リストやガイドBGMによってはオフのほうがいい場合もありますが今回はオンがおすすめです（ループ収録になります）。
右上の三点リーダー[…]→[設定]→録音セクションの[連続録音]をオンにします。

② セッション（プロジェクトファイル）を作成します。左下の＋ボタンを押します。
これは新しい音源を録るたびに作成します。以前作成したものを選択すれば前回の続きから録れます。

③ 録音リストを選択します。
初回は、右上の三点リーダー[…]→[インポート]から録音リスト（HelloUTAUWorld_録音リスト.txt）を読み込みます。
「続いてコメントファイルをインポートしますか？」と聞かれるので、はいを押して「～-comment.txt」と書いてある方を選択します。
2回目以降は、前回選んだファイルが表示されるのでそれをクリックするだけで選択できます。

④ 録音できそうな画面が出るんですがその前に、ガイドBGMを設定します。
右上の三点リーダー[…]→[ガイドBGMを設定]→右上の三点リーダー[…]→[インポート]で、ガイドBGMを選択します。
インポートされたガイドBGMを選んでから、左上の[←]ボタンで戻ります。"#);
        ui.hyperlink_to(
            "AtWikiより抜粋",
            "https://w.atwiki.jp/vbmaker/pages/43.html",
        );
        ui.separator();
        ui.label("今回利用するのはHello UTAU World同梱のHelloUTAUWorld_録音リスト.txtです。");
        ui.label(format!(
            "録音リストの場所: {}",
            huw.join("録音リスト")
                .join("HelloUTAUWorld_録音リスト.txt")
                .display()
        ));
        ui.label("終わりましたら、いよいよ収録開始しましょう！");
        Ok(())
    }

    fn environment(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("収録環境");
        ui.label("反響や雑音をできるだけ取り除く方法を紹介します。");
        ui.label("得に気にしない方は飛ばしていただいて大丈夫です。");
        ui.separator();
        ui.label("TODO!");
        Ok(())
    }

    fn recording(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("いよいよ収録です！");
        ui.label("◎みたいなボタンを押して収録です！グッドラック！");
        ui.label("終わったら次へを押して下さい。次は原音設定です！");
        Ok(())
    }

    fn move_session(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("vLabelerの下準備");
        ui.label("収録終了おめでとうございます！こちらでデータの移動をしておきます！");
        ui.separator();
        match self.recstar_state {
            RsState::Waiting => {
                self.recstar_state = if let Some(s) = recstar_entries() {
                    RsState::Listed(s, None)
                } else {
                    RsState::Failed("RecStarから取得したフォルダの自動取得に失敗しました".into())
                };
            }
            RsState::Failed(ref msg) => {
                ui.label(RichText::new(msg).color(Color32::RED));
                ui.label("手動でRecStarの収録フォルダを選択していただけるとこちらで処理します。\nご自身で処理する場合は次に進んで大丈夫です。");
                if ui.button("フォルダ選択").clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_folder()
                {
                    self.recstar_state = RsState::Selected(path);
                }
            }
            RsState::Listed(ref l, ref picked) => {
                let mut next = None;
                if l.is_empty() {
                    ui.label("RecStarのセッションが見つかりませんでした。");
                } else {
                    ui.label("RecStarのセッションフォルダを選択してください。");
                    for i in l {
                        if ui
                            .radio(picked.as_ref() == Some(i), i.display().to_string())
                            .clicked()
                        {
                            next = Some(RsState::Listed(l.clone(), Some(i.clone())));
                        }
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            picked.is_some(),
                            egui::Button::new("このセッションを移動する"),
                        )
                        .clicked()
                        && let Some(p) = picked
                    {
                        next = Some(RsState::Selected(p.clone()));
                    }
                    if ui.button("別のフォルダを選択").clicked()
                        && let Some(path) = rfd::FileDialog::new().pick_folder()
                    {
                        next = Some(RsState::Selected(path));
                    }
                });
                if let Some(next) = next {
                    self.recstar_state = next;
                }
            }
            RsState::Selected(ref p) => {
                let session = self.state.data_dir.join("session");
                let result = copy_dir_all(p, &session).and_then(|()| {
                    fs::copy(
                        self.huw_dir().join("otoiniテンプレート/BPM120用/oto.ini"),
                        session.join("oto.ini"),
                    )
                });
                self.recstar_state = match result {
                    Ok(_) => {
                        self.state.proj_dir = Some(session.clone());
                        RsState::Moved(session)
                    }
                    Err(e) => RsState::Failed(format!("データの移動に失敗しました: {e}")),
                };
            }
            RsState::Moved(ref p) => {
                ui.label(format!(
                    "データの移動に成功しました。移動先:{}",
                    p.display()
                ));
                ui.label("次に移動しましょう！");
            }
        }
        Ok(())
    }

    fn auto_oto(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("自動原音設定");
        ui.label("収録した音声をサーバーに送り、oto.iniの下書きを自動で作成する事ができます。");
        ui.label("情報収集・AI学習には利用されませんが、AIを利用した自動原音設定になります。");
        ui.label("ご自身で原音設定を行う場合は次に進んで大丈夫です。");
        ui.separator();
        let session = self.state.data_dir.join("session");
        if !session.exists() {
            ui.label(
                RichText::new("収録データが見つかりません。前のページでデータを移動してください。")
                    .color(Color32::RED),
            );
            return Ok(());
        }
        ui.add_enabled_ui(self.oto_job.is_none(), |ui| {
            ui.horizontal(|ui| {
                ui.label("サーバーURL");
                ui.text_edit_singleline(&mut self.state.otoini_url);
            });
            ui.horizontal(|ui| {
                ui.label("招待トークン");
                ui.add(egui::TextEdit::singleline(&mut self.state.otoini_token).password(true));
            });
        });
        if let Some(dir) = &self.state.oto_result_dir {
            ui.label(format!(
                "自動原音設定が完了しました。音源の場所: {}",
                dir.display()
            ));
            ui.label("次に進みましょう！");
        }
        if self.oto_job.is_some() {
            return Ok(());
        }
        let ready =
            !self.state.otoini_url.trim().is_empty() && !self.state.otoini_token.trim().is_empty();
        if ui
            .add_enabled(ready, egui::Button::new("自動原音設定を開始"))
            .clicked()
        {
            self.install_error = None;
            let dest = self.state.data_dir.join("oto_result");
            let (url, token) = (
                self.state.otoini_url.clone(),
                self.state.otoini_token.clone(),
            );
            self.oto_job = Some(install::spawn_job(move |progress| {
                otoini::run(&url, &token, &session, &dest, progress)
            }));
        }
        Ok(())
    }

    fn label(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("原音設定に移りましょう！");
        ui.label("これからvLabelerを使って原音設定を行います。");
        if ui.button("vLabelerを起動する").clicked() {
            let vl_path = install::app_exe(&self.state.data_dir.join("vlabeler"), "vLabeler");
            let err = if !vl_path.exists() {
                Some("vLabelerが見つかりませんでした。vLabelerの初期設定を行ってください。".into())
            } else if let Err(e) = Command::new(&vl_path).spawn() {
                Some(format!(
                    "原音設定ソフトの起動に失敗しました: {e}\nvLabelerを手動で起動して原音設定してください。"
                ))
            } else {
                self.success_message =
                    Some(("原音設定ソフトを起動しました。".into(), Instant::now()));
                None
            };
            self.install_error = err.map(|e| (e, Instant::now()));
        }
        ui.label("原音設定理論は複雑すぎてここでは説明しきれません。詳しくはUTAU音源制作Wikiをご覧ください。");
        ui.hyperlink_to(
            "UTAU原音設定基礎理論",
            "https://w.atwiki.jp/vbmaker/pages/17.html",
        );
        ui.label("このソフトでの完全解説は準備中です。乞うご期待！");
        ui.label("原音設定が完了したら、次に進んでください。いよいよパッケージングです！");
        Ok(())
    }

    fn information(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("パッケージング");
        ui.label("原音設定が完了したら、音源をパッケージングして配布できるようにします。");
        // version, name, en_us_localized, readme, author, singer, web, icon, bg, portrait opacity,
        // portrait height
        ui.horizontal(|ui| {
            ui.label("バージョン");
            ui.text_edit_singleline(&mut self.state.package_data.version);
        });
        ui.horizontal(|ui| {
            ui.label("名前");
            ui.text_edit_singleline(&mut self.state.package_data.name);
        });
        ui.horizontal(|ui| {
            ui.label("英語名");
            ui.text_edit_singleline(&mut self.state.package_data.en_us_localized);
        });
        ui.horizontal(|ui| {
            ui.label("作者");
            ui.text_edit_singleline(&mut self.state.package_data.author);
        });
        ui.horizontal(|ui| {
            ui.label("声優");
            ui.text_edit_singleline(&mut self.state.package_data.voice);
        });
        ui.horizontal(|ui| {
            ui.label("Webサイト");
            ui.text_edit_singleline(&mut self.state.package_data.web);
        });
        ui.horizontal(|ui| {
            ui.label("アイコン");
            if ui.button("選択").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.state.package_data.icon_path = Some(path);
            }
        });
        ui.horizontal(|ui| {
            ui.label("立ち絵");
            if ui.button("選択").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.state.package_data.bg_path = Some(path);
            }
        });
        ui.label("詳細");
        ui.text_edit_multiline(&mut self.state.package_data.readme);
        ui.collapsing("立ち絵の詳細", |ui| {
            ui.horizontal(|ui| {
                ui.label("不透明度");
                ui.add(egui::Slider::new(
                    &mut self.state.package_data.portrait_opacity,
                    0.0..=1.0,
                ));
            });
            ui.horizontal(|ui| {
                ui.label("高さ(そのままの場合は0)");
                ui.add(
                    egui::DragValue::new(&mut self.state.package_data.portrait_height).speed(1.0),
                );
            });
        });
        if ui.button("書き込み").clicked() {
            let dir = self
                .state
                .proj_dir
                .as_ref()
                .ok_or("収録データの移動先が未設定です。")?;
            let pkg = &mut self.state.package_data;
            if let Some(icon) = &pkg.icon_path {
                let name = icon
                    .file_name()
                    .ok_or("アイコンのパスが親ディレクトリで終わっています")?;
                let dest = dir.join(name).with_extension("bmp");
                // Already converted on an earlier click: copying onto itself would empty it.
                if *icon != dest {
                    if icon
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("bmp"))
                    {
                        fs::copy(icon, &dest)?;
                    } else {
                        image::open(icon)?.save_with_format(&dest, image::ImageFormat::Bmp)?;
                    }
                }
                pkg.icon_path = Some(dest);
            }
            if let Some(bg) = &pkg.bg_path
                && !bg.starts_with(dir)
            {
                let name = bg
                    .file_name()
                    .ok_or("立ち絵のパスが親ディレクトリで終わっています")?;
                let dest = dir.join(name);
                fs::copy(bg, &dest)?;
                pkg.bg_path = Some(dest);
            }
            // The character files want paths relative to the voicebank root.
            let mut rel = pkg.clone();
            for p in [&mut rel.icon_path, &mut rel.bg_path].into_iter().flatten() {
                *p = p.strip_prefix(dir)?.to_path_buf();
            }
            fs::write(
                dir.join("character.txt"),
                SHIFT_JIS
                    .encode(&serde_java_properties::to_string(&CharacterTxt::from(
                        rel.clone(),
                    ))?)
                    .0,
            )?;
            if !rel.readme.trim().is_empty() {
                // UTAU shows readme.txt as Shift_JIS with CRLF line endings.
                let readme = rel.readme.replace("\r\n", "\n").replace('\n', "\r\n");
                fs::write(dir.join("readme.txt"), SHIFT_JIS.encode(&readme).0)?;
            }
            // OpenUtau reads character.yaml as UTF-8.
            fs::write(
                dir.join("character.yaml"),
                yaml_serde::to_string(&CharacterYaml::from(rel))?,
            )?;
            self.success_message = Some((
                "character.txt等の書き込みに成功しました。".into(),
                Instant::now(),
            ));
        }
        Ok(())
    }

    fn testing(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("音源テスト");
        if ui.button("OpenUtauにインストールする").clicked() {
            let dir = self
                .state
                .proj_dir
                .as_ref()
                .ok_or("収録データの移動先が未設定です。")?;
            let ou_dir = self.state.ou_singer_dir.as_ref().ok_or(
                "OpenUtauの音源フォルダが未設定です。OpenUtauの初期設定を行ってください。",
            )?;
            // Exactly one plain folder name, or remove_dir_all below could hit the whole
            // Singers folder ("") or escape it ("../x").
            let name = self.state.package_data.name.trim();
            let mut parts = Path::new(name).components();
            if !matches!(
                (parts.next(), parts.next()),
                (Some(std::path::Component::Normal(_)), None)
            ) {
                return Err("音源の名前が空か、フォルダ名として使えません。音源情報のページで名前を設定してください。".into());
            }
            let dest = ou_dir.join(name);
            if dest.exists() {
                let ok = rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("上書きの確認")
                    .set_description(format!(
                        "{}は既に存在します。削除して上書きしますか？",
                        dest.display()
                    ))
                    .set_buttons(rfd::MessageButtons::OkCancel)
                    .show();
                if ok != rfd::MessageDialogResult::Ok {
                    return Ok(());
                }
                fs::remove_dir_all(&dest)?;
            }
            copy_dir_all(dir, &dest)?;
            self.success_message = Some((
                format!("OpenUtauにインストールしました: {}", dest.display()),
                Instant::now(),
            ));
            self.state.installed_to_ou = true;
        }
        if self.state.installed_to_ou {
            ui.label("OpenUtauにインストール済みです。テストしてみましょう！");
            if ui.button("OpenUtauを起動する").clicked() {
                let ou_path = self.state.ou_dir.as_ref().ok_or(
                    "OpenUtauの実行ファイルが未設定です。OpenUtauの初期設定を行ってください。",
                )?;
                // ou_dir is the install folder (Windows/Linux) or the binary itself (`which`).
                let exe = if ou_path.is_dir() {
                    ou_path.join(if cfg!(windows) {
                        "OpenUtau.exe"
                    } else {
                        "OpenUtau.AppImage"
                    })
                } else {
                    ou_path.clone()
                };
                if let Err(e) = Command::new(&exe)
                    .arg(self.huw_dir().join("テスト歌唱用データ/TEST.ustx"))
                    .spawn()
                {
                    self.install_error =
                        Some((format!("OpenUtauの起動に失敗しました: {e}"), Instant::now()));
                } else {
                    self.success_message =
                        Some(("OpenUtauを起動しました。".into(), Instant::now()));
                }
            }
            ui.add_space(10.0);
            ui.label(format!(
                "OpenUtauで {} のシンガーを選択して、スペースキーで再生してみましょう！",
                self.state.package_data.name
            ));
        }
        Ok(())
    }

    fn packaging_and_install(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("パッケージングとインストール");
        ui.label("音源の情報をまとたら、zipに圧縮したり、そのままOpenUtauにインストールすることができます。");
        if self.pkg_job.is_none() && ui.button("zipで書き出し").clicked() {
            let dir = self
                .state
                .proj_dir
                .clone()
                .ok_or("収録データの移動先が未設定です。")?;
            let name = match self.state.package_data.name.trim() {
                "" => "voicebank".to_owned(),
                n => n.to_owned(),
            };
            if let Some(dest) = rfd::FileDialog::new()
                .set_file_name(format!("{name}.zip"))
                .add_filter("zip", &["zip"])
                .save_file()
            {
                self.install_error = None;
                self.pkg_job = Some(install::spawn_job(move |progress| {
                    install::set_progress(progress, "音源をzipに圧縮しています...", None);
                    let install_txt = format!(
                        "type=voiceset\r\nfolder={name}\r\ncontentsdir={name}\r\ndescription={name}\r\n"
                    );
                    let install_txt = SHIFT_JIS.encode(&install_txt).0;
                    install::zip_dir(
                        &dir,
                        &dest,
                        &format!("{name}/"),
                        &[("install.txt", &install_txt)],
                        true,
                    )?;
                    Ok(install::InstallResult::Package(dest))
                }));
            }
        }

        Ok(())
    }

    fn end(&mut self, ui: &mut Ui) -> PageResult {
        ui.heading("お疲れ様でした！");
        ui.separator();
        ui.label("これではじめてのUTAUガイドの終了です！単独音の音源が完成しました！");
        ui.label("先ほど描き出したzipファイルをそのまま配布して大丈夫です！");
        if let Some(ref p) = self.state.proj_dir {
            ui.label(format!(
                "もとの収録ファイルは{}に保存されています。",
                p.display()
            ));
        }
        ui.label("他にも連続音、VCCV等のボイスバンクの種類がありますので次はそちらを試す事をおすすめします！");
        ui.label("連続音のバージョンも鋭意製作中です！お楽しみに！");
        ui.label(
            "下記のボタンを押すとファイルの自動片付けを行います。全て終わった後に押してください。",
        );
        if ui.button("終了する").clicked() {
            // Only what this app can download or unpack again; the voicebank stays.
            let mut failed: Vec<_> = ["recstar", "vlabeler", "hello-utau-world"]
                .into_iter()
                .map(|d| self.state.data_dir.join(d))
                .filter(|d| {
                    fs::remove_dir_all(d).is_err_and(|e| e.kind() != io::ErrorKind::NotFound)
                })
                .map(|d| d.display().to_string())
                .collect();
            if fs::remove_file(self.state.data_dir.join("state.json")).is_err() {
                failed.push(
                    self.state
                        .data_dir
                        .join("state.json")
                        .to_string_lossy()
                        .into(),
                );
            }
            for f in ["otoini-upload.zip", "otoini-result.zip"] {
                let _ = fs::remove_file(std::env::temp_dir().join(f));
            }
            self.state.rs_installed = false;
            self.state.vl_installed = false;
            let voicebank = self
                .state
                .proj_dir
                .as_ref()
                .map(|p| format!("\n音源の場所: {}", p.display()))
                .unwrap_or_default();
            let (level, msg) = if failed.is_empty() {
                (
                    rfd::MessageLevel::Info,
                    format!("片付けが完了しました。良いUTAUライフを！{voicebank}"),
                )
            } else {
                (
                    rfd::MessageLevel::Warning,
                    format!(
                        "次のファイル及びフォルダを削除できませんでした。手動で削除してください:\n{}{voicebank}",
                        failed.join("\n")
                    ),
                )
            };
            rfd::MessageDialog::new()
                .set_level(level)
                .set_title("お疲れ様でした！")
                .set_description(msg)
                .show();
            process::exit(0)
        }
        Ok(())
    }

    fn huw_dir(&self) -> PathBuf {
        self.state.data_dir.join("Hello UTAU World!")
    }
}

fn recstar_entries() -> Option<Vec<PathBuf>> {
    let dir = home_dir()?.join("RecStar/sessions/");
    Some(dir.read_dir().ok()?.flatten().map(|e| e.path()).collect())
}

fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

use std::{
    env::home_dir,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use eframe::egui::{self, Color32, RichText, Ui};

use crate::{Kickstart, RsState, install, otoini};

#[cfg(feature = "include_zip")]
const HELLO_UTAU_ZIP: &[u8] = include_bytes!("../Hello UTAU World!_v111.zip");

static TABLE_OF_CONTENTS: &[&str] = &["はじめに", "目次"];

pub const PAGES: &[fn(&mut Kickstart, &mut Ui)] = &[
    Kickstart::intro,
    Kickstart::toc,
    Kickstart::requirements,
    Kickstart::openutau_setup,
    Kickstart::tools_setup,
    Kickstart::huw_extract,
    Kickstart::launch_recstar,
    Kickstart::recstar_howto,
    Kickstart::environment,
    Kickstart::recording,
    Kickstart::move_session,
    Kickstart::auto_oto,
];

impl Kickstart {
    fn intro(&mut self, ui: &mut Ui) {
        ui.heading("このソフトは自動でUTAUのボイスバンクを作る補助をするソフトです。\n基本的にはUTAU音源制作Wikiの内情に沿っています。早速右下の「次へ」をクリックして進みましょう！");
        ui.hyperlink_to(
            "参考：UTAU音源制作Wikiの元記事",
            "https://w.atwiki.jp/vbmaker/pages/43.html",
        );
    }

    fn toc(&mut self, ui: &mut Ui) {
        ui.label("目次");
        for (i, title) in TABLE_OF_CONTENTS.iter().enumerate() {
            if ui.button(*title).clicked() {
                self.state.page = i as u32;
            }
        }
    }

    fn requirements(&mut self, ui: &mut Ui) {
        ui.heading("必要なもの");
        ui.separator();
        ui.label("- パソコン(Windows)");
        ui.label("- マイク、オーディオインターフェースなどの録音機材");
        ui.hyperlink_to("→詳細", "https://w.atwiki.jp/vbmaker/pages/14.html");
        ui.label("- 収録環境");
        ui.label("録音、原音設定、調声のソフトは自動でインストールされます。");
    }

    fn openutau_setup(&mut self, ui: &mut Ui) {
        ui.heading("データの初期設定");
        ui.label("このソフトは全て管理して最小限で自動インストールすることができますが、OpenUtau公式のインストーラを使用しますか？");
        ui.radio_value(&mut self.state.use_openutau_installer, true, "はい");
        ui.radio_value(&mut self.state.use_openutau_installer, false, "いいえ");

        // poll_jobs shows progress while installing.
        if self.ou_install.is_some() {
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

        #[cfg(windows)]
        {
            if self.state.use_openutau_installer {
                ui.label("OpenUtau公式のインストーラをダウンロードします。よろしいですか？");
                if ui.button("はい").clicked() {
                    self.install_error = None;
                    self.ou_install = Some(install::win::ou_installer());
                }
            } else {
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
                        self.ou_install = Some(install::win::portable_install(dest.clone()));
                    }
                });
            }
        }

        #[cfg(not(windows))]
        {
            ui.label("Linuxの場合はディストロに応じた方法でOpenUtauをインストールしてください。インストール後、OpenUtauの実行ファイルと音源フォルダの場所を入力してください。");
            if ui.button("インストール完了").clicked() {
                match Command::new("which").arg("openutau").output() {
                    Ok(out) if out.status.success() => {
                        let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                        self.state.ou_dir = Some(path.into());
                    }
                    _ => {
                        self.install_error = Some(
                            "システムのPATHにOpenUtauが見つかりませんでした。必要無い、又は既にインストールされている場合は無視しても大丈夫です。".into(),
                        );
                    }
                }
            }
        }
    }

    fn tools_setup(&mut self, ui: &mut Ui) {
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
    }

    fn huw_extract(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Hello UTAU Worldを解凍する").size(20.));
        ui.label("こちらのファイルは特別な許諾を得て配布されているデータです。解凍されたデータの再配布は禁止されています。");
        // dest exists mid-extraction, so check the job first.
        if self.huw_job.is_some() {
            return;
        }
        let dest = self.huw_dir();
        if dest.exists() {
            ui.label(format!(
                "Hello UTAU Worldは解凍済みです。次に進んでください: {}",
                dest.display()
            ));
            return;
        }
        #[cfg(feature = "include_zip")]
        if ui.button("Hello UTAU Worldを解凍する").clicked() {
            self.install_error = None;
            self.huw_job = Some(install::spawn_job(move |progress| {
                install::set_progress(progress, "Hello UTAU Worldを解凍しています...", None);
                let reader = std::io::Cursor::new(HELLO_UTAU_ZIP);
                if let Err(e) = install::extract_zip(reader, &dest, progress) {
                    // Remove the partial folder so the button reappears for a retry.
                    let _ = fs::remove_dir_all(&dest);
                    return Err(format!("Hello UTAU Worldの解凍に失敗しました: {e}").into());
                }
                Ok(install::InstallResult::Huw)
            }));
        }
    }

    fn launch_recstar(&mut self, ui: &mut Ui) {
        ui.heading("録音しよう");
        ui.label("では、早速録音をはじめましょう！下のボタンを押すと録音ソフトが起動します。");
        if ui.button("録音ソフトを起動する").clicked() {
            let recstar_path = self.state.data_dir.join("recstar").join("RecStar.exe");
            self.install_error = if !recstar_path.exists() {
                Some("RecStarが見つかりませんでした。RecStarの初期設定を行ってください。".into())
            } else if let Err(e) = Command::new(&recstar_path).spawn() {
                Some(format!(
                    "録音ソフトの起動に失敗しました: {e}\nRecStarを手動で起動して録音してください。"
                ))
            } else {
                None
            };
        }
    }

    fn recstar_howto(&mut self, ui: &mut Ui) {
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
    }

    fn environment(&mut self, ui: &mut Ui) {
        ui.heading("収録環境");
        ui.label("反響や雑音をできるだけ取り除く方法を紹介します。");
        ui.label("得に気にしない方は飛ばしていただいて大丈夫です。");
        ui.separator();
        ui.label("TODO!");
    }

    fn recording(&mut self, ui: &mut Ui) {
        ui.heading("いよいよ収録です！");
        ui.label("◎みたいなボタンを押して収録です！グッドラック！");
        ui.label("終わったら次へを押して下さい。次は原音設定です！");
    }

    fn move_session(&mut self, ui: &mut Ui) {
        ui.heading("vLabelerの下準備");
        ui.label("収録終了おめでとうございます！こちらでデータの移動をしておきます！");
        ui.separator();
        match self.recstar_state {
            RsState::Waiting => {
                self.recstar_state = if let Some(s) = recstar_entries() {
                    RsState::Listed(s)
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
            RsState::Listed(ref l) => {
                ui.label("RecStarのセッションフォルダを選択してください。");
                for i in l {
                    if ui.label(i.to_str().unwrap()).clicked() {
                        self.recstar_state = RsState::Selected(i.clone());
                        break;
                    }
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
                    p.to_str().unwrap()
                ));
                ui.label("次に移動しましょう！");
            }
        }
    }

    fn auto_oto(&mut self, ui: &mut Ui) {
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
            return;
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
            return;
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
    }

    fn huw_dir(&self) -> PathBuf {
        self.state.data_dir.join("hello-utau-world")
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

use std::{
    env,
    fs::{self, File},
    path::Path,
    thread,
    time::Duration,
};

use crate::{
    BoxError,
    install::{
        InstallResult, Progress, download_with_progress, extract_zip, set_progress, zip_dir,
    },
};

/// Send `req`, returning the JSON body or the server's `{error}` message.
fn call(req: reqwest::blocking::RequestBuilder) -> Result<serde_json::Value, BoxError> {
    let resp = req.send()?;
    let status = resp.status();
    let body: serde_json::Value = serde_json::from_str(&resp.text()?).unwrap_or_default();
    if !status.is_success() {
        let msg = body["error"].as_str().unwrap_or("不明なエラー");
        return Err(format!("otoiniサーバーエラー ({status}): {msg}").into());
    }
    Ok(body)
}

/// Zip `session`, upload, start, poll, then extract the returned voicebank
/// (wavs + generated oto.ini) into `dest`.
pub fn run(
    base: &str,
    token: &str,
    session: &Path,
    dest: &Path,
    progress: &Progress,
) -> Result<InstallResult, BoxError> {
    let base = base.trim().trim_end_matches('/');
    let client = reqwest::blocking::Client::builder().timeout(None).build()?;

    set_progress(progress, "収録データを圧縮しています...", None);
    let upload = env::temp_dir().join("otoini-upload.zip");
    zip_dir(session, &upload, "", &[], false)?;

    set_progress(progress, "otoiniにアップロードしています...", None);
    let form = reqwest::blocking::multipart::Form::new()
        .text("token", token.trim().to_owned())
        .file("file", &upload)?;
    let job = call(client.post(format!("{base}/api/jobs")).multipart(form));
    let _ = fs::remove_file(&upload);
    let job = job?;
    let id = job["id"].as_str().ok_or("ジョブIDが返されませんでした。")?;

    call(
        client
            .post(format!("{base}/api/jobs/{id}/start"))
            .header("content-type", "application/json")
            .body(r#"{"reclist_type":"auto"}"#),
    )?;

    loop {
        let j = call(client.get(format!("{base}/api/jobs/{id}")))?;
        match j["state"].as_str().unwrap_or("") {
            "ok" => break,
            s @ ("rejected" | "failed") => {
                let why = j["report"]["error"].as_str().unwrap_or("");
                return Err(format!("自動原音設定に失敗しました ({s}) {why}").into());
            }
            s => {
                let p = &j["progress"];
                let frac = match (p["files_done"].as_f64(), p["files_total"].as_f64()) {
                    (Some(d), Some(t)) if t > 0.0 => Some((d / t) as f32),
                    _ => None,
                };
                set_progress(progress, &format!("otoiniで処理中です... ({s})"), frac);
            }
        }
        thread::sleep(Duration::from_secs(3));
    }

    let result = env::temp_dir().join("otoini-result.zip");
    download_with_progress(
        &format!("{base}/api/jobs/{id}/result.zip"),
        &result,
        progress,
        "結果をダウンロードしています...",
    )?;
    set_progress(progress, "音源を展開しています...", None);
    extract_zip(File::open(&result)?, dest, progress)?;
    let _ = fs::remove_file(&result);
    Ok(InstallResult::Oto(dest.to_path_buf()))
}

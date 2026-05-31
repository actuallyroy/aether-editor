// Screenshot upload for the feedback form. A captured PNG is uploaded to
// Cloudinary via an *unsigned* upload preset (cloud name + preset are public and
// safe to ship — no API key/secret lives in the binary), and the returned
// secure URL is embedded in the GitHub issue body.

use std::sync::mpsc::Sender;

use crate::marketplace::WorkerMsg;

const CLOUD_NAME: &str = "dzgwjgtyv";
const UPLOAD_PRESET: &str = "ml_default";

/// Upload PNG bytes to Cloudinary (unsigned). Returns the `secure_url` on success.
fn upload_png(png: &[u8]) -> Result<String, String> {
    let url = format!("https://api.cloudinary.com/v1_1/{CLOUD_NAME}/image/upload");
    let boundary = "----novaFeedbackBoundary7MA4YWxkTrZu0gW";

    // Build the multipart/form-data body by hand (ureq has no multipart helper):
    // an `upload_preset` text field + the binary `file` part.
    let mut body: Vec<u8> = Vec::with_capacity(png.len() + 512);
    let mut push = |s: &str, body: &mut Vec<u8>| body.extend_from_slice(s.as_bytes());
    push(&format!("--{boundary}\r\n"), &mut body);
    push("Content-Disposition: form-data; name=\"upload_preset\"\r\n\r\n", &mut body);
    push(&format!("{UPLOAD_PRESET}\r\n"), &mut body);
    push(&format!("--{boundary}\r\n"), &mut body);
    push(
        "Content-Disposition: form-data; name=\"file\"; filename=\"screenshot.png\"\r\n",
        &mut body,
    );
    push("Content-Type: image/png\r\n\r\n", &mut body);
    body.extend_from_slice(png);
    push(&format!("\r\n--{boundary}--\r\n"), &mut body);

    let resp = ureq::post(&url)
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .map_err(|e| format!("upload request failed: {e}"))?;
    let text = resp
        .into_string()
        .map_err(|e| format!("bad upload response: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad upload JSON: {e}"))?;
    json.get("secure_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no secure_url in upload response".to_string())
}

/// Off-thread: upload the screenshot (if any), append it to the issue body, create
/// the GitHub issue via `gh`, and report the created URL (or an error) back to the
/// UI thread. `gh_program` is resolved on the caller's side so this stays free of
/// platform lookups.
pub fn submit_async(
    png: Option<Vec<u8>>,
    title: String,
    mut body: String,
    gh_program: String,
    tx: Sender<WorkerMsg>,
) {
    std::thread::spawn(move || {
        // 1) Upload the screenshot, embedding it on success. A failed upload is
        //    non-fatal — the issue is still filed, just without the image.
        if let Some(png) = png {
            match upload_png(&png) {
                Ok(url) => body.push_str(&format!("\n\n### Screenshot\n\n![screenshot]({url})")),
                Err(e) => body.push_str(&format!("\n\n_(screenshot upload failed: {e})_")),
            }
        }
        // 2) File the issue.
        const REPO: &str = "actuallyroy/nova-editor";
        let mut cmd = std::process::Command::new(&gh_program);
        cmd.args(["issue", "create", "--repo", REPO, "--title", &title, "--body", &body]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // no console flash
        }
        let result = match cmd.output() {
            Ok(o) if o.status.success() => {
                Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
            }
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(WorkerMsg::FeedbackDone { result });
    });
}

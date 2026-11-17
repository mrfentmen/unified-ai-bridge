//! Browser download metadata is fetched by the background, never accepted
//! from a website/model. Keep source paths and signed URLs out of UI replies.
use crate::js;
use serde::Deserialize;
use serde_json::{json, Value};
use wasm_bindgen_futures::JsFuture;

const MAX_BYTES: i64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Download {
    id: i32,
    filename: String,
    #[serde(default)]
    mime: String,
    url: String,
    #[serde(default)]
    final_url: String,
    state: String,
    danger: String,
    exists: bool,
    #[serde(default)]
    incognito: bool,
    file_size: i64,
    #[serde(default)]
    total_bytes: i64,
    #[serde(default)]
    end_time: String,
}

/// Only rendered download links on provider/file origins are actionable. No
/// model URL, arbitrary network destination, data URI or local-file access.
pub fn allowed_file_url(provider: bridge_core::provider::Provider, raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw.strip_prefix("blob:").unwrap_or(raw)) else {return false};
    url.scheme()=="https" && url.username().is_empty() && url.password().is_none()
        && url.port().is_none() && (provider.allowed(url.as_str())
            || (provider==bridge_core::provider::Provider::ChatGpt && url.host_str()==Some("files.oaiusercontent.com")))
}

/// Chrome returns the ID of OUR request. Never correlate a website button by
/// filename, timestamp, origin or the user's latest unrelated download.
pub async fn download_link(provider: bridge_core::provider::Provider, url: &str) -> Result<Value,String> {
    if !allowed_file_url(provider,url) {return Err("Unsupported generated-file link; use the website download control and share the selected file.".into());}
    let promise = js::downloads_download(&js::to_js(&json!({"url":url,"saveAs":false,"conflictAction":"uniquify"})))
        .map_err(|_|"Chrome could not start this generated-file download. Inspect downloads; do not retry automatically.")?;
    let id = JsFuture::from(promise).await.map_err(|_|"Chrome rejected the generated-file download. Inspect downloads before retrying.")?
        .as_f64().filter(|n|*n>=0.0 && *n<=i32::MAX as f64 && n.fract()==0.0).ok_or("Chrome returned no download ID.")? as i32;
    for _ in 0..40 {
        let item = search(json!({"id":id,"limit":1})).await?.into_iter().find(|d|d.id==id).ok_or("Download disappeared; inspect Chrome downloads.")?;
        if item.total_bytes>MAX_BYTES || item.file_size>MAX_BYTES {
            if let Ok(p) = js::downloads_cancel(id) {let _ = JsFuture::from(p).await;}
            return Err("Generated file exceeds 64 MiB. Cancellation requested; any partial download remains in Chrome. Nothing shared.".into());
        }
        if item.state=="complete" {return selected(id).await;}
        if item.state=="interrupted" || item.incognito || item.danger!="safe" {
            return Err("Download interrupted, private or flagged by Chrome. Nothing shared; inspect Chrome downloads.".into());
        }
        let pause = js_sys::Promise::new(&mut |resolve,_| {js::schedule(1_000,move||{let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);});});
        let _ = JsFuture::from(pause).await;
    }
    Err("Download still pending after 40 seconds. Inspect Chrome downloads and share it manually when complete. It will not be retried.".into())
}

impl Download {
    fn eligible(&self) -> bool {
        self.state == "complete"
            && self.exists
            && self.danger == "safe"
            && !self.incognito
            && (0..=MAX_BYTES).contains(&self.file_size)
            && !self.filename.is_empty()
    }
    fn origin(&self) -> String {
        // blob URLs can contain an origin but never expose the blob ID.
        let raw = if self.final_url.is_empty() {
            &self.url
        } else {
            &self.final_url
        };
        url::Url::parse(raw.strip_prefix("blob:").unwrap_or(raw))
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default()
    }
    fn public(&self) -> Value {
        json!({"download_id":self.id,"name":self.filename.rsplit('/').next().unwrap_or("file"),
            "bytes":self.file_size,"mime":self.mime,"origin":self.origin(),"completed_at":self.end_time})
    }
}

async fn search(query: Value) -> Result<Vec<Download>, String> {
    let promise = js::downloads_search(&js::to_js(&query)).map_err(|_|"Chrome download access unavailable; reload the extension and check its Downloads permission.")?;
    let value = JsFuture::from(promise)
        .await
        .map_err(|_| "Chrome could not read its downloads list.")?;
    js::from_js(value).ok_or_else(|| "Invalid Chrome downloads response.".into())
}

pub async fn list() -> Result<Value, String> {
    let found =
        search(json!({"limit":50,"orderBy":["-startTime"],"state":"complete","exists":true}))
            .await?;
    Ok(
        json!({"downloads":found.iter().filter(|d|d.eligible()).map(Download::public).collect::<Vec<_>>()}),
    )
}

pub async fn selected(id: i32) -> Result<Value, String> {
    let item = search(json!({"id":id,"limit":1}))
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or("Download no longer exists in Chrome.")?;
    if !item.eligible() {
        return Err("Select a completed, safe, non-private download of at most 64 MiB that still exists on disk.".into());
    }
    Ok(
        json!({"download_id":item.id,"filename":item.filename,"bytes":item.file_size,"mime":item.mime,"origin":item.origin()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn download() -> Download {
        serde_json::from_value(json!({"id":1,"filename":"/private/Downloads/image.png","mime":"image/png","url":"https://files.example/image?secret=token#fragment","state":"complete","danger":"safe","exists":true,"fileSize":12})).unwrap()
    }
    #[test]
    fn metadata_omits_paths_and_signed_urls() {
        let d = download();
        assert!(d.eligible());
        let text = d.public().to_string();
        assert!(!text.contains("private"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("token"));
        assert_eq!(d.origin(), "files.example");
    }
    #[test]
    fn unsafe_incomplete_missing_private_and_large_downloads_are_excluded() {
        for field in ["state", "danger", "exists", "incognito", "fileSize"] {
            let mut d = download();
            match field {
                "state" => d.state = "in_progress".into(),
                "danger" => d.danger = "accepted".into(),
                "exists" => d.exists = false,
                "incognito" => d.incognito = true,
                _ => d.file_size = MAX_BYTES + 1,
            };
            assert!(!d.eligible(), "{field}");
        }
    }
    #[test]
    fn download_links_cannot_escape_the_supported_provider_origins() {
        use bridge_core::provider::Provider::*;
        for raw in ["https://chatgpt.com/file","blob:https://chatgpt.com/id","https://files.oaiusercontent.com/file?token=private"] {
            assert!(allowed_file_url(ChatGpt,raw));
        }
        for raw in ["file:///etc/passwd","javascript:alert(1)","data:image/svg+xml,abc","http://chatgpt.com/file","https://user:secret@chatgpt.com/file","https://chatgpt.com.evil.test/file","https://127.0.0.1/file","https://chatgpt.com:8000/file","blob:file:///etc/passwd"] {
            assert!(!allowed_file_url(ChatGpt,raw),"{raw}");
        }
        assert!(!allowed_file_url(Claude,"https://files.oaiusercontent.com/file"));
        assert!(!allowed_file_url(ChatGpt,"https://claude.ai/file"));
    }
}

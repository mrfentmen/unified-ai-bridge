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
    end_time: String,
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
}

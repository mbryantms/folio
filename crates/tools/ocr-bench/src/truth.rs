//! Test pages and reference transcriptions on disk.
//!
//! Pages: `<pages>/<book>/pNNN.jpg` (any image extension). Truth:
//! `<truth>/<book>-pNNN.json`, a list of regions in reading order
//! (or an object holding that list under `regions`), each with `id`,
//! `kind`, `text`, an approximate `bbox_pct` (x, y, w, h in percent of
//! the page; an object or a four-number list), `colour_case` and
//! `notes`. Unknown fields are ignored and missing ones default, so a
//! hand-written file with small shape differences still loads.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct PageFile {
    /// `<book>-pNNN`, the key truth files are named by.
    pub key: String,
    pub path: PathBuf,
}

/// Every page image under `pages_dir/<book>/`, sorted by key.
pub(crate) fn list_pages(pages_dir: &Path) -> anyhow::Result<Vec<PageFile>> {
    let mut out = Vec::new();
    for book in
        std::fs::read_dir(pages_dir).with_context(|| format!("reading {}", pages_dir.display()))?
    {
        let book = book?;
        if !book.file_type()?.is_dir() {
            continue;
        }
        let book_name = book.file_name().to_string_lossy().into_owned();
        for f in std::fs::read_dir(book.path())? {
            let f = f?;
            let p = f.path();
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
                continue;
            }
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(PageFile {
                key: format!("{book_name}-{stem}"),
                path: p,
            });
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RefRegion {
    pub id: String,
    pub kind: String,
    pub text: String,
    /// x, y, w, h in percent of the page.
    pub bbox_pct: [f64; 4],
    pub colour_case: String,
    pub notes: String,
}

impl RefRegion {
    /// The approximate box in page pixels: (x0, y0, x1, y1).
    pub(crate) fn bbox_px(&self, page_w: u32, page_h: u32) -> [f64; 4] {
        let [x, y, w, h] = self.bbox_pct;
        let (pw, ph) = (f64::from(page_w), f64::from(page_h));
        [
            x / 100.0 * pw,
            y / 100.0 * ph,
            (x + w) / 100.0 * pw,
            (y + h) / 100.0 * ph,
        ]
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TruthFile {
    List(Vec<Value>),
    Object { regions: Vec<Value> },
}

fn as_string(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn as_bbox(v: Option<&Value>) -> Option<[f64; 4]> {
    let v = v?;
    let nums: Vec<f64> = match v {
        Value::Array(a) => a.iter().filter_map(Value::as_f64).collect(),
        Value::Object(o) => ["x", "y", "w", "h"]
            .iter()
            .filter_map(|k| o.get(*k).and_then(Value::as_f64))
            .collect(),
        _ => return None,
    };
    (nums.len() == 4).then(|| [nums[0], nums[1], nums[2], nums[3]])
}

/// Load `<truth_dir>/<key>.json`; `Ok(None)` when it does not exist.
pub(crate) fn load_truth(truth_dir: &Path, key: &str) -> anyhow::Result<Option<Vec<RefRegion>>> {
    let path = truth_dir.join(format!("{key}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    let parsed: TruthFile =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    let items = match parsed {
        TruthFile::List(v) | TruthFile::Object { regions: v } => v,
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let text = as_string(item.get("text"));
        let Some(bbox_pct) = as_bbox(item.get("bbox_pct")) else {
            anyhow::bail!("{}: region {i} has no usable bbox_pct", path.display());
        };
        let id = match item.get("id") {
            None | Some(Value::Null) => format!("#{}", i + 1),
            v => as_string(v),
        };
        out.push(RefRegion {
            id,
            kind: or_unknown(as_string(item.get("kind"))),
            text,
            bbox_pct,
            colour_case: or_unknown(as_string(item.get("colour_case"))),
            notes: as_string(item.get("notes")),
        });
    }
    Ok(Some(out))
}

fn or_unknown(s: String) -> String {
    if s.trim().is_empty() {
        "(unset)".to_owned()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_list_and_object_shapes() {
        let dir = std::env::temp_dir().join(format!("ocr-bench-truth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a-p001.json"),
            r#"[{"id":"r1","kind":"balloon","text":"HI","bbox_pct":{"x":1,"y":2,"w":3,"h":4},"colour_case":"b/w","notes":""}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("a-p002.json"),
            r#"{"regions":[{"id":7,"text":"YO","bbox_pct":[10,20,30,40]}]}"#,
        )
        .unwrap();
        let a = load_truth(&dir, "a-p001").unwrap().unwrap();
        assert_eq!(a[0].bbox_pct, [1.0, 2.0, 3.0, 4.0]);
        let b = load_truth(&dir, "a-p002").unwrap().unwrap();
        assert_eq!(b[0].id, "7");
        assert_eq!(b[0].kind, "(unset)");
        assert_eq!(b[0].bbox_px(200, 100), [20.0, 20.0, 80.0, 60.0]);
        assert!(load_truth(&dir, "missing").unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}

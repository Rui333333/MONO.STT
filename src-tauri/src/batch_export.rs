use std::{collections::HashSet, io::Write, path::Path};
use tauri_plugin_dialog::DialogExt;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

// Each entry is (title, extension, UTF-8 transcript).
type Entry = (String, String, String);

#[tauri::command]
pub async fn export_zip(
    app: tauri::AppHandle,
    name: String,
    entries: Vec<Entry>,
) -> Result<bool, String> {
    if entries.is_empty()
        || entries
            .iter()
            .any(|(_, ext, _)| !matches!(ext.as_str(), "srt" | "txt"))
    {
        return Err("请选择转录记录和 SRT 或 TXT 格式".into());
    }
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("ZIP 压缩包", &["zip"])
        .set_file_name(name)
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || write_zip(&path, entries))
        .await
        .map_err(|error| error.to_string())??;
    Ok(true)
}

pub(crate) fn filename_stem(title: &str) -> String {
    let mut stem: String = title
        .trim()
        .chars()
        .take(80)
        .map(|ch| {
            if ch.is_control() || "\\/:*?\"<>|".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    stem.truncate(stem.trim_end_matches([' ', '.']).len());
    if stem.is_empty() {
        return "transcript".into();
    }
    let first_part = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    if matches!(first_part.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || first_part
            .strip_prefix("COM")
            .or_else(|| first_part.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
    {
        stem.insert(0, '_');
    }
    stem
}

fn write_zip(path: &Path, entries: Vec<Entry>) -> Result<(), String> {
    let directory = path.parent().ok_or("无法找到保存目录")?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("无法在所选目录创建压缩包：{error}"))?;
    let mut zip = ZipWriter::new(temporary.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut used_names = HashSet::new();
    for (title, ext, content) in entries {
        let stem = filename_stem(&title);
        let mut name = format!("{stem}.{ext}");
        let mut duplicate = 1;
        while !used_names.insert(name.to_lowercase()) {
            duplicate += 1;
            name = format!("{stem} ({duplicate}).{ext}");
        }
        zip.start_file(&name, options)
            .map_err(|error| format!("无法压缩 {name}：{error}"))?;
        zip.write_all(content.as_bytes())
            .map_err(|error| format!("无法写入 {name}：{error}"))?;
    }
    zip.finish()
        .map_err(|error| format!("无法完成压缩包：{error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("无法保存压缩包：{error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, io::Read};

    #[test]
    fn exports_utf8_transcripts_with_unique_windows_filenames() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("20261125123456.zip");
        std::fs::write(&path, b"old archive").unwrap();
        let cases = [
            ("会议记录", "srt", "第一句\n", "会议记录.srt"),
            ("会议记录", "txt", "第一句", "会议记录.txt"),
            ("会议记录", "srt", "第二句\n", "会议记录 (2).srt"),
            ("AUX.txt", "txt", "第三句", "_AUX.txt.txt"),
            ("a/../b", "txt", "第四句", "a_.._b.txt"),
            ("Report", "txt", "第五句", "Report.txt"),
            ("report", "txt", "第六句", "report (2).txt"),
        ];
        write_zip(
            &path,
            cases
                .iter()
                .map(|(title, ext, text, _)| ((*title).into(), (*ext).into(), (*text).into()))
                .collect(),
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        assert_eq!(archive.len(), cases.len());
        for (_, _, expected, name) in cases {
            let mut entry = archive.by_name(name).unwrap();
            assert_eq!(entry.compression(), CompressionMethod::Deflated);
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            assert_eq!(text, expected);
        }
    }
}

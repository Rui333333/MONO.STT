use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Write},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    ptr::{null, null_mut},
    sync::{Mutex, TryLockError},
    time::Duration,
};
use tauri::Manager;
use windows_sys::Win32::{
    Security::Credentials::{
        CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    },
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};

const API: &str = "https://open.feishu.cn/open-apis";
const DOCUMENTS: &str = "/docx/v1/documents";
const CREDENTIAL_KEY: &str = "MONO.STT.Feishu";
const BATCH_SIZE: usize = 50;

#[derive(Default)]
pub struct FeishuPublish(Mutex<()>);

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn credentials() -> Result<Option<Value>, String> {
    let mut raw = null_mut::<CREDENTIALW>();
    if unsafe {
        CredReadW(
            wide(CREDENTIAL_KEY).as_ptr(),
            CRED_TYPE_GENERIC,
            0,
            &mut raw,
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(1168) {
            Ok(None)
        } else {
            Err(format!("无法读取 Windows 凭据：{error}"))
        };
    }
    let parsed = unsafe {
        let size = (*raw).CredentialBlobSize as usize;
        if size == 0 || (*raw).CredentialBlob.is_null() {
            Err("飞书凭据为空，请重新配置".into())
        } else {
            serde_json::from_slice(std::slice::from_raw_parts((*raw).CredentialBlob, size))
                .map_err(|_| "无法读取飞书凭据，请重新配置".into())
        }
    };
    unsafe { CredFree(raw.cast()) };
    parsed.map(Some)
}

fn store_credentials(value: &Value) -> Result<(), String> {
    let mut blob = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if blob.len() > 2560 {
        return Err("飞书凭据过长".into());
    }
    let mut name = wide(CREDENTIAL_KEY);
    let mut entry = CREDENTIALW::default();
    entry.Type = CRED_TYPE_GENERIC;
    entry.TargetName = name.as_mut_ptr();
    entry.CredentialBlobSize = blob.len() as u32;
    entry.CredentialBlob = blob.as_mut_ptr();
    entry.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if unsafe { CredWriteW(&entry, 0) } != 0 {
        Ok(())
    } else {
        Err(format!(
            "无法保存 Windows 凭据：{}",
            io::Error::last_os_error()
        ))
    }
}

struct ApiError {
    message: String,
    rejected: bool,
    missing_permissions: bool,
}

impl ApiError {
    fn unknown(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rejected: false,
            missing_permissions: false,
        }
    }
}

fn curl_quote(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

fn api(
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    // Pass headers and JSON through stdin so credentials stay out of process arguments.
    let mut config = format!(
        "url = {}\nrequest = {}\n",
        curl_quote(&format!("{API}{path}")),
        curl_quote(method)
    );
    if let Some(token) = token {
        config.push_str(&format!(
            "header = {}\n",
            curl_quote(&format!("Authorization: Bearer {token}"))
        ));
    }
    if let Some(body) = body {
        config.push_str("header = \"Content-Type: application/json; charset=utf-8\"\n");
        config.push_str(&format!(
            "data-binary = {}\n",
            curl_quote(&body.to_string())
        ));
    }
    let curl = PathBuf::from(
        std::env::var_os("SystemRoot").ok_or_else(|| ApiError::unknown("缺少 SystemRoot"))?,
    )
    .join(r"System32\curl.exe");
    let mut child = Command::new(curl)
        .args([
            "--disable",
            "--silent",
            "--proto",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| ApiError {
            message: format!("无法启动飞书连接：{error}"),
            rejected: true,
            missing_permissions: false,
        })?;
    if child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(config.as_bytes())
        .is_err()
    {
        let _ = child.wait();
        return Err(ApiError::unknown("无法发送飞书请求"));
    }
    let output = child
        .wait_with_output()
        .map_err(|_| ApiError::unknown("未能读取飞书响应"))?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or_default();
        return Err(ApiError {
            message: format!("连接飞书失败（curl {code}）"),
            rejected: matches!(code, 5 | 6 | 7 | 35 | 60),
            missing_permissions: false,
        });
    }
    let response = String::from_utf8_lossy(&output.stdout);
    let (response, status) = response
        .rsplit_once('\n')
        .ok_or_else(|| ApiError::unknown("飞书响应格式异常"))?;
    let status = status.trim().parse::<u16>().unwrap_or_default();
    let parsed: Value = serde_json::from_str(response).map_err(|_| ApiError {
        message: format!("飞书响应格式异常（HTTP {status}）"),
        rejected: (400..500).contains(&status),
        missing_permissions: false,
    })?;
    let code = parsed["code"].as_i64();
    if (200..300).contains(&status) && code == Some(0) {
        return Ok(parsed);
    }
    let detail = parsed["msg"].as_str().unwrap_or("请求失败");
    let detail = if path.contains("tenant_access_token") {
        "请核对 App ID 和 App Secret"
    } else {
        detail
    };
    Err(ApiError {
        message: format!(
            "飞书请求失败（{}）：{detail}",
            code.map_or_else(|| status.to_string(), |value| value.to_string())
        ),
        missing_permissions: code == Some(99991672),
        rejected: code == Some(99991672)
            || (400..500).contains(&status)
            || ((200..300).contains(&status) && code.is_some()),
    })
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn token(config: &Value) -> Result<String, String> {
    let response = api(
        "POST",
        "/auth/v3/tenant_access_token/internal",
        None,
        Some(&json!({
            "app_id": text(config, "appId"),
            "app_secret": text(config, "appSecret"),
        })),
    )
    .map_err(|error| error.message)?;
    response["tenant_access_token"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "飞书未返回应用访问凭证".into())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn target(value: &str, kind: &str) -> Result<(String, String), String> {
    let value = value.trim();
    if value.is_empty() || valid_id(value) {
        return Ok((value.into(), "feishu.cn".into()));
    }
    let (host, path) = value
        .strip_prefix("https://")
        .and_then(|url| url.split_once('/'))
        .ok_or("请输入飞书链接或 token")?;
    let identifier = path
        .strip_prefix(&format!("{kind}/"))
        .unwrap_or_default()
        .split(['?', '#', '/'])
        .next()
        .unwrap_or_default();
    if !valid_id(identifier) || !feishu_host(host) {
        return Err("请输入有效的飞书链接或 token".into());
    }
    Ok((identifier.into(), host.into()))
}

fn feishu_host(host: &str) -> bool {
    host == "feishu.cn"
        || host.strip_suffix(".feishu.cn").is_some_and(|label| {
            !label.is_empty()
                && label.split('.').all(|label| {
                    !label.is_empty()
                        && label
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                })
        })
}

#[tauri::command]
pub async fn feishu_save_credentials(
    app_id: String,
    app_secret: String,
    folder: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let app_id = app_id.trim();
        if !valid_id(app_id) || !app_id.starts_with("cli_") {
            return Err("请输入飞书自建应用的 App ID（cli_ 开头）".into());
        }
        let previous = credentials()?.unwrap_or_default();
        let secret = if app_secret.trim().is_empty() && text(&previous, "appId") == app_id {
            text(&previous, "appSecret")
        } else {
            app_secret.trim()
        };
        if secret.is_empty() || secret.len() > 512 || secret.chars().any(char::is_control) {
            return Err("请输入有效的 App Secret".into());
        }
        let (folder_id, host) = target(&folder, "drive/folder")?;
        let config = json!({"appId":app_id,"appSecret":secret,"folder":folder.trim(),
            "folderId":folder_id,"host":host});
        token(&config)?;
        store_credentials(&config)
    })
    .await
    .map_err(|error| error.to_string())?
}

struct Binding<'a> {
    app: &'a tauri::AppHandle,
    id: &'a str,
}

impl<'a> Binding<'a> {
    fn new(app: &'a tauri::AppHandle, id: &'a str) -> Result<Self, String> {
        if !valid_id(id) {
            return Err("转录记录 ID 无效".into());
        }
        Ok(Self { app, id })
    }

    fn key(&self) -> String {
        format!("feishu.binding.{}", self.id)
    }

    fn read(&self) -> Result<Value, String> {
        let store = self.app.state::<crate::store::Store>();
        let key = self.key();
        if let Some(payload) = store.get(&key)? {
            return serde_json::from_str(&payload).map_err(|e| e.to_string());
        }
        let path = self
            .app
            .path()
            .app_local_data_dir()
            .map_err(|e| e.to_string())?
            .join("feishu")
            .join(format!("record-{}.json", self.id));
        match fs::read(path) {
            Ok(bytes) => {
                let binding: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                store.set(&key, &binding.to_string())?;
                Ok(binding)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Value::Null),
            Err(error) => Err(format!("无法读取飞书文档关联：{error}")),
        }
    }

    fn write(&self, value: &Value) -> Result<(), String> {
        self.app
            .state::<crate::store::Store>()
            .set(&self.key(), &value.to_string())
    }
}

fn binding_url(binding: &Value) -> Option<String> {
    let id = text(binding, "documentId");
    let host = text(binding, "host");
    (valid_id(id) && feishu_host(host)).then(|| format!("https://{host}/docx/{id}"))
}

#[tauri::command]
pub fn feishu_status(app: tauri::AppHandle, record_id: String) -> Result<Value, String> {
    let config = credentials()?.unwrap_or_default();
    let binding = Binding::new(&app, &record_id)?.read()?;
    Ok(json!({
        "appId": text(&config, "appId"),
        "folder": text(&config, "folder"),
        "url": binding_url(&binding),
        "needsRecovery": !binding.is_null() && binding["documentId"].is_null(),
        "format": binding["format"],
    }))
}

#[tauri::command]
pub fn feishu_open_document(app: tauri::AppHandle, record_id: String) -> Result<(), String> {
    let binding = Binding::new(&app, &record_id)?.read()?;
    let url = binding_url(&binding).ok_or("此转录尚未关联飞书文档")?;
    open_feishu_url(&url)
}

fn permissions_url(app_id: &str) -> Option<String> {
    (app_id.starts_with("cli_") && valid_id(app_id)).then(||
        format!("https://open.feishu.cn/app/{app_id}/auth?q=docx%3Adocument&op_from=openapi&token_type=tenant"))
}

#[tauri::command]
pub fn feishu_open_permissions() -> Result<(), String> {
    let config = credentials()?.ok_or("请先配置飞书应用凭据")?;
    let url = permissions_url(text(&config, "appId")).ok_or("飞书应用 ID 无效")?;
    open_feishu_url(&url)
}

fn open_feishu_url(url: &str) -> Result<(), String> {
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            wide(url).as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    if (result as isize) <= 32 {
        return Err("无法打开飞书链接".into());
    }
    Ok(())
}

#[derive(Debug)]
enum PublishError {
    Message(String),
    Permissions,
}
impl From<String> for PublishError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}
impl From<&str> for PublishError {
    fn from(value: &str) -> Self {
        Self::Message(value.into())
    }
}
impl From<ApiError> for PublishError {
    fn from(value: ApiError) -> Self {
        if value.missing_permissions {
            Self::Permissions
        } else {
            Self::Message(value.message)
        }
    }
}
impl PublishError {
    fn into_response(self) -> Value {
        match self {
            Self::Permissions => {
                let url = credentials()
                    .ok()
                    .flatten()
                    .and_then(|config| permissions_url(text(&config, "appId")));
                json!({"kind":"permissions","url":url})
            }
            Self::Message(message) => json!({"kind":"error","message":message}),
        }
    }
}

#[tauri::command]
pub async fn feishu_publish(
    app: tauri::AppHandle,
    record_id: String,
    title: String,
    format: String,
    content: String,
    recovery_link: Option<String>,
) -> Result<String, Value> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<FeishuPublish>();
        let _guard = state.0.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => "另一个飞书发布正在进行，请稍后重试",
            TryLockError::Poisoned(_) => "飞书发布状态异常，请重启应用",
        })?;
        publish(
            &Binding::new(&app, &record_id)?,
            &title,
            &format,
            &content,
            recovery_link.as_deref(),
        )
    })
    .await
    .map_err(|error| PublishError::from(error.to_string()).into_response())?
    .map_err(PublishError::into_response)
}

fn publish(
    storage: &Binding<'_>,
    title: &str,
    format: &str,
    content: &str,
    recovery_link: Option<&str>,
) -> Result<String, PublishError> {
    let blocks = transcript_blocks(format, content)?;
    let config = credentials()?.ok_or("请先配置飞书应用凭据")?;
    let app_id = text(&config, "appId");
    let mut binding = storage.read()?;
    if !binding.is_null() && text(&binding, "appId") != app_id {
        return Err("此转录已关联另一飞书应用创建的文档，请切换回原应用凭据".into());
    }
    let token = token(&config)?;
    let title: String = title.trim().chars().take(380).collect();
    let title = if title.is_empty() {
        "转录结果"
    } else {
        &title
    };
    if binding.is_null() {
        binding = json!({ "appId": app_id, "title": title, "host": text(&config,"host") });
        storage.write(&binding)?;
        let mut body = json!({ "title": title });
        let folder = text(&config, "folderId");
        if !folder.is_empty() {
            body["folder_token"] = json!(folder);
        }
        let response = match api("POST", DOCUMENTS, Some(&token), Some(&body)) {
            Ok(response) => response,
            Err(error) if error.rejected => {
                storage
                    .write(&Value::Null)
                    .map_err(|e| format!("{}；无法清理待创建状态：{e}", error.message))?;
                return Err(error.into());
            }
            Err(error) => {
                return Err(format!(
                    "{}。创建结果未确认，请先核对飞书中是否已生成文档。",
                    error.message
                )
                .into())
            }
        };
        let id = text(&response["data"]["document"], "document_id");
        if !valid_id(id) {
            return Err("飞书未返回有效文档 ID，请核对创建结果后关联原文档".into());
        }
        binding["documentId"] = json!(id);
        storage
            .write(&binding)
            .map_err(|error| format!("{error}。已建文档 ID：{id}"))?;
    } else if binding["documentId"].is_null() {
        let (id, host) = target(recovery_link.unwrap_or_default(), "docx")?;
        if id.is_empty() {
            return Err("请填写上次创建的飞书文档链接，避免重复创建".into());
        }
        let meta = api("GET", &format!("{DOCUMENTS}/{id}"), Some(&token), None)?;
        let document = &meta["data"]["document"];
        if text(document, "title") != text(&binding, "title") || document["revision_id"] != 1 {
            return Err("仅可关联与上次请求同标题、尚未编辑的飞书文档，请核对链接".into());
        }
        binding["documentId"] = json!(id);
        binding["host"] = json!(host);
        storage.write(&binding)?;
    }
    let id = text(&binding, "documentId");
    let url = binding_url(&binding).ok_or("飞书文档关联无效")?;
    write_transcript(id, &token, &blocks, api).map_err(|error| match error {
        PublishError::Message(message) => {
            PublishError::Message(format!("{message}。文档已关联，重试会继续使用同一篇文档。"))
        }
        PublishError::Permissions => PublishError::Permissions,
    })?;
    binding["format"] = json!(format);
    storage.write(&binding)?;
    Ok(url)
}

fn transcript_blocks(format: &str, content: &str) -> Result<Vec<Value>, String> {
    if !matches!(format, "srt" | "text") || content.trim().is_empty() {
        return Err("没有可发布的转录内容".into());
    }
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let paragraphs: Vec<_> = if format == "srt" {
        normalized
            .trim()
            .split("\n\n")
            .map(|cue| {
                cue.lines()
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ")
            })
            .collect()
    } else {
        normalized.lines().map(str::to_owned).collect()
    };
    let mut blocks = Vec::new();
    for paragraph in paragraphs
        .into_iter()
        .filter(|line| !line.trim().is_empty())
    {
        for part in split_text(&paragraph, 24_000) {
            blocks.push(json!({ "block_type":2, "text": {
                "elements":[{"text_run":{"content":part}}]
            }}));
        }
    }
    Ok(blocks)
}

fn split_text(text: &str, limit: usize) -> Vec<&str> {
    let mut result = Vec::new();
    let mut remaining = text;
    while let Some((boundary, _)) = remaining.char_indices().nth(limit) {
        let end = remaining[..boundary]
            .rfind('\n')
            .map_or(boundary, |newline| newline + 1);
        result.push(&remaining[..end]);
        remaining = &remaining[end..];
    }
    if !remaining.is_empty() {
        result.push(remaining);
    }
    result
}

fn write_transcript(
    document: &str,
    token: &str,
    blocks: &[Value],
    request: impl Fn(&str, &str, Option<&str>, Option<&Value>) -> Result<Value, ApiError>,
) -> Result<(), PublishError> {
    let path = format!("{DOCUMENTS}/{document}");
    let root = format!("{path}/blocks/{document}");
    let response = request("GET", &path, Some(token), None)?;
    let mut revision = response["data"]["document"]["revision_id"]
        .as_i64()
        .ok_or("飞书未返回文档版本")?;
    let response = request(
        "GET",
        &format!("{root}?document_revision_id={revision}"),
        Some(token),
        None,
    )?;
    let page = &response["data"]["block"];
    if text(page, "block_id") != document || page["block_type"] != 1 {
        return Err("飞书未返回文档的根块".into());
    }
    let previous = page["children"].as_array().map_or(0, Vec::len);

    // Write the complete replacement first so a failed upload leaves the prior body available.
    for (index, chunk) in blocks.chunks(BATCH_SIZE).enumerate() {
        let response = request(
            "POST",
            &format!("{root}/children?document_revision_id={revision}"),
            Some(token),
            Some(&json!({"index":previous + index * BATCH_SIZE, "children":chunk})),
        )?;
        revision = response["data"]["document_revision_id"]
            .as_i64()
            .ok_or("飞书未确认本次正文写入版本")?;
        std::thread::sleep(Duration::from_millis(220));
    }
    let mut end = previous;
    while end > 0 {
        let start = end.saturating_sub(BATCH_SIZE);
        let response = request(
            "DELETE",
            &format!("{root}/children/batch_delete?document_revision_id={revision}"),
            Some(token),
            Some(&json!({"start_index":start, "end_index":end})),
        )?;
        revision = response["data"]["document_revision_id"]
            .as_i64()
            .ok_or("飞书未确认本次正文更新版本")?;
        end = start;
        if end > 0 {
            std::thread::sleep(Duration::from_millis(220));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn writes_one_plain_paragraph_per_srt_cue() {
        let srt = "1\r\n00:00:00,000 --> 00:00:01,000\r\n第一行\r\n第二行\r\n\r\n2\r\n00:00:01,000 --> 00:00:02,000\r\n下一条 🎤\r\n";
        let blocks = transcript_blocks("srt", srt).unwrap();
        let lines: Vec<_> = blocks
            .iter()
            .map(|block| {
                assert_eq!(block["block_type"], 2);
                block["text"]["elements"][0]["text_run"]["content"]
                    .as_str()
                    .unwrap()
            })
            .collect();
        assert_eq!(
            lines,
            [
                "1  00:00:00,000 --> 00:00:01,000  第一行  第二行",
                "2  00:00:01,000 --> 00:00:02,000  下一条 🎤",
            ]
        );
        let paragraphs = transcript_blocks("text", "第一行\r\n\r\n第二行").unwrap();
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(
            paragraphs[1]["text"]["elements"][0]["text_run"]["content"],
            "第二行"
        );
        assert!(transcript_blocks("text", " \r\n").is_err());
    }

    #[test]
    fn retries_replace_partial_upload_in_same_document() {
        let original = json!("原正文");
        let remote = RefCell::new(vec![original.clone()]);
        let revision = Cell::new(1);
        let fail_next_delete = Cell::new(true);
        let request = |method: &str, path: &str, token: Option<&str>, body: Option<&Value>| {
            assert_eq!(token, Some("test-token"));
            assert!(path.starts_with("/docx/v1/documents/doc1"));
            if path.ends_with("/doc1") && method == "GET" {
                return Ok(json!({"data":{"document":{"revision_id": revision.get()}}}));
            }
            assert!(path.ends_with(&format!("document_revision_id={}", revision.get())));
            match method {
                "GET" => {
                    return Ok(json!({"data":{"block":{
                        "block_id":"doc1","block_type":1,"children":*remote.borrow()
                    }}}))
                }
                "POST" => {
                    let body = body.unwrap();
                    assert_eq!(body["index"], remote.borrow().len());
                    remote
                        .borrow_mut()
                        .extend(body["children"].as_array().unwrap().clone());
                }
                "DELETE" => {
                    if fail_next_delete.replace(false) {
                        return Err(ApiError::unknown("simulated connection failure"));
                    }
                    let body = body.unwrap();
                    let start = body["start_index"].as_u64().unwrap() as usize;
                    let end = body["end_index"].as_u64().unwrap() as usize;
                    remote.borrow_mut().drain(start..end);
                }
                _ => panic!("unexpected request"),
            }
            revision.set(revision.get() + 1);
            Ok(json!({"data":{"document_revision_id":revision.get()}}))
        };

        let new = transcript_blocks("text", "第一段\n第二段").unwrap();
        assert!(write_transcript("doc1", "test-token", &new, request).is_err());
        assert_eq!(remote.borrow()[0], original);
        write_transcript("doc1", "test-token", &new, request).unwrap();
        assert_eq!(*remote.borrow(), new);
    }

    #[test]
    fn binding_rejects_non_feishu_hosts() {
        assert!(
            binding_url(&json!({"documentId":"docx123","host":"feishu.cn.evil.example"})).is_none()
        );
        assert_eq!(
            binding_url(&json!({"documentId":"docx123","host":"tenant.feishu.cn"})).as_deref(),
            Some("https://tenant.feishu.cn/docx/docx123")
        );
        assert!(target(
            "https://tenant.feishu.cn.evil.example/drive/folder/test",
            "drive/folder"
        )
        .is_err());
    }
}

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use md5::Md5;
use reqwest::header::{HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::redirect::Policy;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const DEFAULT_HARDWARE_VERSION: &str = "RT08_V3.1";
pub const DEFAULT_ROM_VERSION: &str = "RT08_3.10.48_260309";
pub const DEFAULT_MAC: &str = "31:31:45:37:9C:07";
const MAX_FIRMWARE_BYTES: u64 = 12_288_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OtaRegion {
    China,
    Global,
}

impl OtaRegion {
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::China => "https://china.qcwxwire.com/qcwx/app-update/last-ota/china",
            Self::Global => "https://api1.qcwxkjvip.com/qcwx/app-update/last-ota",
        }
    }

    fn login_endpoint(self) -> &'static str {
        match self {
            Self::China => "https://china.qcwxwire.com/qcwx/users/login/v1",
            Self::Global => "https://api1.qcwxkjvip.com/qcwx/users/login/v1",
        }
    }

    fn guest_token_endpoint(self) -> &'static str {
        match self {
            Self::China => "https://china.qcwxwire.com/qcwx/token/getToken",
            Self::Global => "https://api1.qcwxkjvip.com/qcwx/token/getToken",
        }
    }
}

pub struct OtaFetchOptions {
    pub region: OtaRegion,
    pub hardware_version: String,
    pub rom_version: String,
    pub query_rom_version: Option<String>,
    pub mac: String,
    pub country: String,
    pub output_dir: PathBuf,
    pub metadata_only: bool,
    pub assume_yes: bool,
    pub token_auth: bool,
    pub account_auth: bool,
}

#[derive(Debug, Serialize)]
struct LoginRequest<'a> {
    account: &'a str,
    password: &'a str,
    #[serde(rename = "type")]
    login_type: u8,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LastOtaRequest<'a> {
    app_id: u8,
    uid: u8,
    hardware_version: &'a str,
    rom_version: &'a str,
    os: u8,
    mac: &'a str,
    country: &'a str,
    dev: u8,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactMetadata {
    source_endpoint: String,
    download_source: String,
    requested_hardware_version: String,
    requested_rom_version: String,
    #[serde(default)]
    expected_rom_version: String,
    returned_hardware_version: String,
    returned_version: String,
    file_name: String,
    bytes: u64,
    sha256: String,
    downloaded_at_unix: u64,
    authentication_stored: bool,
    dfu_sent: bool,
}

pub async fn fetch(options: OtaFetchOptions) -> Result<()> {
    let endpoint = options.region.endpoint();
    let query_rom_version = options
        .query_rom_version
        .as_deref()
        .unwrap_or(&options.rom_version);
    let request = LastOtaRequest {
        app_id: 1,
        uid: 1,
        hardware_version: &options.hardware_version,
        rom_version: query_rom_version,
        os: 1,
        mac: &options.mac,
        country: &options.country,
        dev: 2,
    };

    println!("OTA_QUERY_READ_ONLY");
    println!("endpoint={endpoint}");
    println!("hardwareVersion={}", options.hardware_version);
    println!("expectedRomVersion={}", options.rom_version);
    println!("queryRomVersion={query_rom_version}");
    if query_rom_version != options.rom_version {
        println!("说明：queryRomVersion 仅用于让官方服务器返回最新版；下载结果仍必须精确匹配 expectedRomVersion。");
    }
    println!("mac={}", mask_mac(&options.mac));
    println!("country={}", options.country);
    println!("说明：这只会向官方服务器查询元数据，不连接戒指，也不会发送 DFU。\n");

    if !options.assume_yes && !confirm("是否将以上设备信息提交给 QRing 官方服务器？[y/N] ")?
    {
        bail!("已取消，未发送网络请求");
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .user_agent("r08-ota-research/0.1")
        .build()
        .context("无法创建 HTTPS 客户端")?;
    let token = authenticate(
        &client,
        options.region,
        options.token_auth,
        options.account_auth,
    )
    .await?;
    let mut token_header = HeaderValue::from_str(token.trim()).context("登录令牌包含无效字符")?;
    token_header.set_sensitive(true);
    let response = client
        .post(endpoint)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header("token", token_header)
        .json(&request)
        .send()
        .await
        .context("无法连接 QRing OTA 服务")?;
    drop(token);

    let http_status = response.status();
    let body: Value = response
        .json()
        .await
        .with_context(|| format!("OTA 服务返回的内容不是有效 JSON（HTTP {http_status}）"))?;
    let ret_code = find_scalar(&body, &["retCode", "code"]);
    let message = find_string(&body, &["message", "msg"]).unwrap_or_default();
    println!("http_status={http_status}");
    if let Some(code) = &ret_code {
        println!("retCode={code}");
    }
    if !message.is_empty() {
        println!("message={message}");
    }

    if is_no_upgrade_code(ret_code.as_deref()) {
        println!("OTA_NO_UPGRADE 当前版本已是官方服务器认定的最新版，没有升级包可下载。");
        return Ok(());
    }

    if !http_status.is_success() || !is_success_code(ret_code.as_deref()) {
        bail!("OTA 查询未成功；请根据 retCode 和 message 检查官方服务状态");
    }

    let download_url = find_string(&body, &["downloadUrl", "downloadURL"]);
    if download_url.is_none() {
        println!("OTA_NO_DOWNLOAD_URL 服务没有返回固件下载地址，设备可能已是最新版本。");
        return Ok(());
    }

    let download_url = Url::parse(download_url.as_deref().unwrap()).context("OTA 下载地址无效")?;
    if download_url.scheme() != "https" {
        bail!("拒绝非 HTTPS 的固件下载地址");
    }
    let safe_download_source = url_without_query(&download_url);
    let returned_hardware = find_string(&body, &["hardwareVersion"]).unwrap_or_default();
    let returned_version =
        find_string(&body, &["version", "romVersion", "firmwareVersion"]).unwrap_or_default();

    println!("download_source={safe_download_source}");
    println!("returned_hardware={returned_hardware}");
    println!("returned_version={returned_version}");

    verify_exact_candidate(
        &options.hardware_version,
        &options.rom_version,
        &returned_hardware,
        &returned_version,
        download_url.path(),
    )?;

    if options.metadata_only {
        println!("OTA_METADATA_ONLY 未下载固件文件。");
        return Ok(());
    }

    let download_client = Client::builder()
        .timeout(Duration::from_secs(120))
        .redirect(Policy::limited(5))
        .user_agent("r08-ota-research/0.1")
        .build()
        .context("无法创建固件下载客户端")?;
    let artifact = download_firmware(
        &download_client,
        &download_url,
        &options.output_dir,
        &returned_version,
    )
    .await?;
    let metadata = ArtifactMetadata {
        source_endpoint: endpoint.to_owned(),
        download_source: safe_download_source,
        requested_hardware_version: options.hardware_version,
        requested_rom_version: query_rom_version.to_owned(),
        expected_rom_version: options.rom_version,
        returned_hardware_version: returned_hardware,
        returned_version,
        file_name: artifact.file_name.clone(),
        bytes: artifact.bytes,
        sha256: artifact.sha256.clone(),
        downloaded_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        authentication_stored: false,
        dfu_sent: false,
    };
    write_metadata(&artifact.path, &metadata)?;

    println!("OTA_DOWNLOADED {}", artifact.path.display());
    println!("bytes={}", artifact.bytes);
    println!("sha256={}", artifact.sha256);
    println!("TOKEN_STORED false");
    println!("DFU_SENT false");
    println!("下一步只能进行离线检查；这仍不代表已经授权刷写。");
    Ok(())
}

async fn authenticate(
    client: &Client,
    region: OtaRegion,
    token_auth: bool,
    account_auth: bool,
) -> Result<Zeroizing<String>> {
    if token_auth && account_auth {
        bail!("--token-auth 与 --account-auth 不能同时使用");
    }
    if token_auth {
        let token = Zeroizing::new(
            rpassword::prompt_password("请输入 QRing 登录令牌（输入不会显示，也不会保存）：")
                .context("无法从本机终端安全读取 QRing 登录令牌")?,
        );
        if token.trim().is_empty() {
            bail!("登录令牌不能为空");
        }
        return Ok(token);
    }

    if !account_auth {
        return request_guest_token(client, region).await;
    }

    let account = Zeroizing::new(
        rpassword::prompt_password("请输入 QRing 登录邮箱（输入不会显示，也不会保存）：")
            .context("无法从本机终端安全读取 QRing 登录邮箱")?,
    );
    if account.trim().is_empty() {
        bail!("登录邮箱不能为空");
    }
    let password = Zeroizing::new(
        rpassword::prompt_password("请输入 QRing 登录密码（输入不会显示，也不会保存）：")
            .context("无法从本机终端安全读取 QRing 登录密码")?,
    );
    if password.is_empty() {
        bail!("登录密码不能为空");
    }
    let password_hash = Zeroizing::new(hash_password(&password));
    drop(password);

    let request = LoginRequest {
        account: account.trim(),
        password: &password_hash,
        login_type: 2,
    };
    let response = client
        .post(region.login_endpoint())
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .json(&request)
        .send()
        .await
        .context("无法连接 QRing 登录服务")?;
    let http_status = response.status();
    let body: Value = response
        .json()
        .await
        .with_context(|| format!("QRing 登录服务返回的内容不是有效 JSON（HTTP {http_status}）"))?;
    let ret_code = find_scalar(&body, &["retCode", "code"]);
    let message = find_string(&body, &["message", "msg"]).unwrap_or_default();
    if !http_status.is_success() || !is_success_code(ret_code.as_deref()) {
        if message.is_empty() {
            bail!("QRing 登录失败（HTTP {http_status}）");
        }
        bail!("QRing 登录失败：{message}");
    }
    let token = find_string(&body, &["token"]).context("QRing 登录成功响应缺少 token")?;
    if token.trim().is_empty() {
        bail!("QRing 登录成功响应包含空 token");
    }
    println!("QRING_LOGIN_OK");
    Ok(Zeroizing::new(token))
}

async fn request_guest_token(client: &Client, region: OtaRegion) -> Result<Zeroizing<String>> {
    let response = client
        .get(region.guest_token_endpoint())
        .query(&[("key", "qcwx_android")])
        .header(ACCEPT, "application/json")
        .send()
        .await
        .context("无法连接 QRing 访客令牌服务")?;
    let http_status = response.status();
    let body: Value = response.json().await.with_context(|| {
        format!("QRing 访客令牌服务返回的内容不是有效 JSON（HTTP {http_status}）")
    })?;
    let ret_code = find_scalar(&body, &["retCode", "code"]);
    let message = find_string(&body, &["message", "msg"]).unwrap_or_default();
    if !http_status.is_success() || !is_success_code(ret_code.as_deref()) {
        if message.is_empty() {
            bail!("QRing 访客令牌申请失败（HTTP {http_status}）");
        }
        bail!("QRing 访客令牌申请失败：{message}");
    }
    let token = find_string(&body, &["data", "token"]).context("QRing 访客令牌响应缺少令牌数据")?;
    if token.trim().is_empty() {
        bail!("QRing 访客令牌响应包含空令牌");
    }
    println!("QRING_GUEST_TOKEN_OK");
    Ok(Zeroizing::new(token))
}

fn hash_password(password: &str) -> String {
    format!("{:x}", Md5::digest(password.as_bytes()))
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn mask_mac(mac: &str) -> String {
    let segments: Vec<_> = mac.split(':').collect();
    if segments.len() == 6 {
        format!("{}:**:**:**:{}:{}", segments[0], segments[4], segments[5])
    } else {
        "<masked>".to_owned()
    }
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::String(value)) = map.get(*key) {
                    if !value.trim().is_empty() {
                        return Some(value.clone());
                    }
                }
            }
            map.values().find_map(|value| find_string(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_string(value, keys)),
        _ => None,
    }
}

fn find_scalar(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key) {
                    match value {
                        Value::String(value) => return Some(value.clone()),
                        Value::Number(value) => return Some(value.to_string()),
                        _ => {}
                    }
                }
            }
            map.values().find_map(|value| find_scalar(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_scalar(value, keys)),
        _ => None,
    }
}

fn is_success_code(code: Option<&str>) -> bool {
    matches!(code, None | Some("0") | Some("200"))
}

fn is_no_upgrade_code(code: Option<&str>) -> bool {
    matches!(code, Some("60001"))
}

fn verify_exact_candidate(
    requested_hardware: &str,
    requested_version: &str,
    returned_hardware: &str,
    returned_version: &str,
    url_path: &str,
) -> Result<()> {
    if returned_hardware.is_empty() {
        bail!("响应缺少 hardwareVersion；为防止跨硬件下载，已停止");
    }
    if !returned_hardware.eq_ignore_ascii_case(requested_hardware) {
        bail!(
            "硬件版本不匹配：请求 {requested_hardware}，服务器返回 {returned_hardware}；已停止下载"
        );
    }
    if returned_version.is_empty() {
        bail!("响应缺少固件版本；为防止下载不明镜像，已停止");
    }
    if !versions_compatible(requested_version, returned_version)
        && !versions_compatible(requested_version, url_path)
    {
        bail!(
            "固件版本不匹配：设备是 {requested_version}，服务器返回 {returned_version}；这不是当前固件备份，已停止下载"
        );
    }
    Ok(())
}

fn versions_compatible(left: &str, right: &str) -> bool {
    if left.eq_ignore_ascii_case(right) {
        return true;
    }
    match (first_dotted_version(left), first_dotted_version(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn first_dotted_version(value: &str) -> Option<String> {
    value
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .find(|part| part.matches('.').count() >= 2 && part.split('.').all(|n| !n.is_empty()))
        .map(ToOwned::to_owned)
}

fn url_without_query(url: &Url) -> String {
    let mut safe = url.clone();
    safe.set_query(None);
    safe.set_fragment(None);
    safe.to_string()
}

struct DownloadedArtifact {
    path: PathBuf,
    file_name: String,
    bytes: u64,
    sha256: String,
}

async fn download_firmware(
    client: &Client,
    url: &Url,
    output_dir: &Path,
    version: &str,
) -> Result<DownloadedArtifact> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("无法创建输出目录 {}", output_dir.display()))?;
    let file_name = firmware_file_name(url, version);
    let final_path = output_dir.join(&file_name);
    let part_path = output_dir.join(format!(".{file_name}.part-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part_path)
        .with_context(|| format!("无法创建临时下载文件 {}", part_path.display()))?;
    let mut partial = PartialFile::new(part_path.clone());

    let response = client
        .get(url.clone())
        .send()
        .await
        .context("无法下载 OTA 固件")?
        .error_for_status()
        .context("OTA 固件下载服务器返回错误")?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_FIRMWARE_BYTES)
    {
        bail!("固件超过官方 App 的 12,288,000 字节上限，已停止");
    }

    let mut stream = response.bytes_stream();
    let mut bytes = 0_u64;
    let mut digest = Sha256::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("下载 OTA 固件时连接中断")?;
        bytes += chunk.len() as u64;
        if bytes > MAX_FIRMWARE_BYTES {
            bail!("固件超过官方 App 的 12,288,000 字节上限，已停止");
        }
        digest.update(&chunk);
        file.write_all(&chunk)?;
    }
    file.sync_all()?;
    drop(file);
    if bytes == 0 {
        bail!("服务器返回了空固件文件");
    }
    let sha256 = format!("{:x}", digest.finalize());

    if final_path.exists() {
        let existing = sha256_file(&final_path)?;
        if existing != sha256 {
            bail!(
                "目标文件已存在且 SHA-256 不同，拒绝覆盖：{}",
                final_path.display()
            );
        }
        return Ok(DownloadedArtifact {
            path: final_path,
            file_name,
            bytes,
            sha256,
        });
    }

    fs::rename(&part_path, &final_path)
        .with_context(|| format!("无法保存固件到 {}", final_path.display()))?;
    partial.keep();
    Ok(DownloadedArtifact {
        path: final_path,
        file_name,
        bytes,
        sha256,
    })
}

fn firmware_file_name(url: &Url, version: &str) -> String {
    let from_url = url
        .path_segments()
        .and_then(|mut parts| parts.next_back())
        .filter(|name| name.to_ascii_lowercase().ends_with(".bin"));
    let raw = from_url
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("RT08_{version}.bin"));
    let sanitized: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('.');
    if sanitized.is_empty() {
        "RT08_firmware.bin".to_owned()
    } else {
        sanitized.to_owned()
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn write_metadata(artifact_path: &Path, metadata: &ArtifactMetadata) -> Result<()> {
    let metadata_path = artifact_path.with_extension("metadata.json");
    if metadata_path.exists() {
        println!("METADATA_EXISTS {}（未覆盖）", metadata_path.display());
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&metadata_path)?;
    let encoded = serde_json::to_vec_pretty(metadata)?;
    file.write_all(&encoded)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    println!("OTA_METADATA {}", metadata_path.display());
    Ok(())
}

struct PartialFile {
    path: PathBuf,
    keep: bool,
}

impl PartialFile {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_uses_qring_field_names() {
        let request = LastOtaRequest {
            app_id: 1,
            uid: 1,
            hardware_version: DEFAULT_HARDWARE_VERSION,
            rom_version: DEFAULT_ROM_VERSION,
            os: 1,
            mac: DEFAULT_MAC,
            country: "CN",
            dev: 2,
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["hardwareVersion"], DEFAULT_HARDWARE_VERSION);
        assert_eq!(value["romVersion"], DEFAULT_ROM_VERSION);
        assert!(value.get("hardware_version").is_none());
    }

    #[test]
    fn login_request_matches_official_app_and_hashes_password() {
        assert_eq!(
            hash_password("password"),
            "5f4dcc3b5aa765d61d8327deb882cf99"
        );
        let password_hash = hash_password("password");
        let request = LoginRequest {
            account: "person@example.test",
            password: &password_hash,
            login_type: 2,
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["account"], "person@example.test");
        assert_eq!(value["password"], "5f4dcc3b5aa765d61d8327deb882cf99");
        assert_eq!(value["type"], 2);
    }

    #[test]
    fn reads_official_guest_token_response() {
        let response = json!({
            "retCode": 0,
            "message": "success",
            "data": "0123456789abcdef0123456789abcdef"
        });
        assert_eq!(
            find_string(&response, &["data", "token"]).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            OtaRegion::China.guest_token_endpoint(),
            "https://china.qcwxwire.com/qcwx/token/getToken"
        );
    }

    #[test]
    fn recognizes_official_no_upgrade_status() {
        let response = json!({
            "retCode": 60001,
            "message": "No upgraded version",
            "data": null
        });
        let code = find_scalar(&response, &["retCode", "code"]);
        assert!(is_no_upgrade_code(code.as_deref()));
        assert!(!is_success_code(code.as_deref()));
    }

    #[test]
    fn reads_nested_ota_response() {
        let response = json!({
            "retCode": 200,
            "data": {
                "hardwareVersion": "RT08_V3.1",
                "version": "3.10.48",
                "downloadUrl": "https://cdn.example/RT08_3.10.48.bin?signature=secret"
            }
        });
        assert_eq!(
            find_string(&response, &["downloadUrl"]).as_deref(),
            Some("https://cdn.example/RT08_3.10.48.bin?signature=secret")
        );
        assert_eq!(find_scalar(&response, &["retCode"]).as_deref(), Some("200"));
    }

    #[test]
    fn accepts_short_server_version_for_exact_device_build() {
        assert!(versions_compatible("RT08_3.10.48_260309", "3.10.48"));
        assert!(!versions_compatible("RT08_3.10.48_260309", "3.10.49"));
    }

    #[test]
    fn rejects_cross_hardware_and_cross_version_images() {
        let hardware_error = verify_exact_candidate(
            DEFAULT_HARDWARE_VERSION,
            DEFAULT_ROM_VERSION,
            "RT08_V2.0",
            "3.10.48",
            "/RT08_3.10.48.bin",
        )
        .unwrap_err();
        assert!(hardware_error.to_string().contains("硬件版本不匹配"));

        let version_error = verify_exact_candidate(
            DEFAULT_HARDWARE_VERSION,
            DEFAULT_ROM_VERSION,
            DEFAULT_HARDWARE_VERSION,
            "3.10.49",
            "/RT08_3.10.49.bin",
        )
        .unwrap_err();
        assert!(version_error.to_string().contains("固件版本不匹配"));
    }

    #[test]
    fn strips_signed_query_from_evidence() {
        let url = Url::parse("https://cdn.example/ring.bin?signature=secret#fragment").unwrap();
        assert_eq!(url_without_query(&url), "https://cdn.example/ring.bin");
    }

    #[test]
    fn sanitizes_server_file_name() {
        let url = Url::parse("https://cdn.example/path/RT08%20firmware.bin").unwrap();
        assert_eq!(firmware_file_name(&url, "3.10.48"), "RT08_20firmware.bin");
    }

    #[test]
    fn masks_device_address_in_console() {
        assert_eq!(mask_mac(DEFAULT_MAC), "31:**:**:**:9C:07");
    }
}

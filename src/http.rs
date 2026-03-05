use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::blocking::Client;

use crate::cli::MirrorSource;

pub fn build_http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(20))
        .user_agent("clash-cli/0.1")
        .build()
        .context("创建 HTTP 客户端失败")
}

pub fn download_candidates(original_url: &str, mirror: MirrorSource) -> Vec<String> {
    let mut urls = Vec::new();
    let ghfast_url = format!("https://ghfast.top/{original_url}");

    match mirror {
        MirrorSource::Auto => {
            if original_url.starts_with("https://github.com/") {
                urls.push(ghfast_url);
            }
            urls.push(original_url.to_string());
        }
        MirrorSource::Ghfast => urls.push(ghfast_url),
        MirrorSource::Github => urls.push(original_url.to_string()),
    }

    urls
}

pub fn download_to_file(client: &Client, url: &str, output_path: &Path) -> Result<()> {
    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("下载请求失败: {url}"))?
        .error_for_status()
        .with_context(|| format!("下载响应失败: {url}"))?;

    let mut file = File::create(output_path)
        .with_context(|| format!("创建下载文件失败: {}", output_path.display()))?;
    io::copy(&mut response, &mut file)
        .with_context(|| format!("写入文件失败: {}", output_path.display()))?;
    file.flush()
        .with_context(|| format!("刷新文件失败: {}", output_path.display()))?;
    Ok(())
}

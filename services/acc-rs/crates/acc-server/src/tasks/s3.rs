//! 极简 S3 客户端（SigV4 签名），只覆盖备份需要的 PUT / LIST / DELETE。
//!
//! 不引入 object_store：备份场景只需要三个操作，手写签名可减少依赖与编译时间。

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio_util::io::ReaderStream;

pub struct S3Client {
    endpoint: String,
    host: String,
    access_key: String,
    secret_key: String,
    region: String,
    bucket: String,
    http: reqwest::Client,
}

impl S3Client {
    pub fn new(
        endpoint: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
        bucket: &str,
    ) -> anyhow::Result<Self> {
        let url = reqwest::Url::parse(endpoint)?;
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("MinIO endpoint 缺少主机名"))?
            .to_string();
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            host,
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            region: region.to_string(),
            bucket: bucket.to_string(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3600))
                .build()?,
        })
    }

    fn uri_encode(input: &str) -> String {
        percent_encoding::utf8_percent_encode(input, AWS_ENCODE_SET).to_string()
    }

    fn sign(
        &self,
        method: &str,
        canonical_uri: &str,
        canonical_query: &str,
        payload_hash: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<(String, String)> {
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();

        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            self.host, payload_hash, amz_date
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );

        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );

        let k_date = hmac(format!("AWS4{}", self.secret_key).as_bytes(), date.as_bytes());
        let k_region = hmac(&k_date, self.region.as_bytes());
        let k_service = hmac(&k_region, b"s3");
        let k_signing = hmac(&k_service, b"aws4_request");
        let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        );

        vec![
            ("authorization".to_string(), authorization),
            ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
            ("x-amz-date".to_string(), amz_date),
        ]
    }

    fn object_url(&self, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.endpoint,
            self.bucket,
            key.split('/').map(Self::uri_encode).collect::<Vec<_>>().join("/")
        )
    }

    /// 上传本地文件；使用 `UNSIGNED-PAYLOAD` 避免二次读盘。
    pub async fn put_file(&self, key: &str, path: &Path) -> anyhow::Result<()> {
        let file = tokio::fs::File::open(path).await?;
        let size = file.metadata().await?.len();
        let canonical_uri = format!(
            "/{}/{}",
            self.bucket,
            key.split('/').map(Self::uri_encode).collect::<Vec<_>>().join("/")
        );
        let headers = self.sign(
            "PUT",
            &canonical_uri,
            "",
            "UNSIGNED-PAYLOAD",
            chrono::Utc::now(),
        );

        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let mut request = self
            .http
            .put(self.object_url(key))
            .header(reqwest::header::CONTENT_LENGTH, size)
            .body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("S3 PUT {key} 失败: {status} {text}");
        }
        Ok(())
    }

    /// 列出前缀下的对象，返回 `(key, last_modified)`。
    pub async fn list(&self, prefix: &str) -> anyhow::Result<Vec<(String, String)>> {
        let mut token: Option<String> = None;
        let mut out = Vec::new();

        loop {
            let mut query = format!("list-type=2&prefix={}", Self::uri_encode(prefix));
            if let Some(t) = &token {
                query.push_str(&format!("&continuation-token={}", Self::uri_encode(t)));
            }
            let canonical_uri = format!("/{}", self.bucket);
            let headers = self.sign("GET", &canonical_uri, &query, "UNSIGNED-PAYLOAD", chrono::Utc::now());

            let url = format!("{}/{}?{}", self.endpoint, self.bucket, query);
            let mut request = self.http.get(url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let response = request.send().await?;
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                anyhow::bail!("S3 LIST 失败: {status} {text}");
            }
            let text = response.text().await?;

            for entry in text.split("<Contents>").skip(1) {
                let key = extract_tag(entry, "Key");
                let modified = extract_tag(entry, "LastModified");
                if let Some(key) = key {
                    out.push((key, modified.unwrap_or_default()));
                }
            }

            match extract_tag(&text, "NextContinuationToken") {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => break,
            }
        }
        Ok(out)
    }

    /// 下载对象到本地文件（用于恢复演练/恢复）。
    pub async fn get_file(&self, key: &str, dest: &Path) -> anyhow::Result<()> {
        let canonical_uri = format!(
            "/{}/{}",
            self.bucket,
            key.split('/').map(Self::uri_encode).collect::<Vec<_>>().join("/")
        );
        let headers = self.sign("GET", &canonical_uri, "", "UNSIGNED-PAYLOAD", chrono::Utc::now());
        let mut request = self.http.get(self.object_url(key));
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("S3 GET {key} 失败: {status} {text}");
        }
        let mut file = tokio::fs::File::create(dest).await?;
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        Ok(())
    }

    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let canonical_uri = format!(
            "/{}/{}",
            self.bucket,
            key.split('/').map(Self::uri_encode).collect::<Vec<_>>().join("/")
        );
        let headers = self.sign("DELETE", &canonical_uri, "", "UNSIGNED-PAYLOAD", chrono::Utc::now());
        let mut request = self.http.delete(self.object_url(key));
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().await?;
        // 404 视为已删除
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("S3 DELETE {key} 失败: {status} {text}");
        }
        Ok(())
    }
}

const AWS_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC 任意长度密钥");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

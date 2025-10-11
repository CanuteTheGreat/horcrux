///! S3-compatible object storage backend
///!
///! Provides S3-compatible storage for VM backups and ISO images
///! Works with AWS S3, MinIO, Ceph RGW, and other S3-compatible services

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use reqwest::Client;
use std::path::PathBuf;

/// S3 storage manager
pub struct S3Manager {
    client: Client,
}

/// S3 configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    pub endpoint: String,       // S3 endpoint URL
    pub region: String,          // AWS region or "us-east-1" for MinIO
    pub bucket: String,          // S3 bucket name
    pub access_key: String,      // Access key ID
    pub secret_key: String,      // Secret access key
    pub use_path_style: bool,    // Use path-style addressing (for MinIO)
    pub use_ssl: bool,           // Use HTTPS
}

/// S3 object metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Object {
    pub key: String,
    pub size: u64,
    pub etag: String,
    pub last_modified: String,
    pub storage_class: Option<String>,
}

impl S3Manager {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Validate S3 storage pool
    pub async fn validate_pool(&self, pool: &super::StoragePool) -> Result<()> {
        

        // Parse s3:// path to extract bucket info
        // Expected format: "s3://bucket-name" or "s3://endpoint/bucket-name"
        let path = &pool.path;

        if !path.starts_with("s3://") {
            return Err(horcrux_common::Error::InvalidConfig(
                "S3 pool path must start with 's3://'".to_string()
            ));
        }

        let bucket_part = path.strip_prefix("s3://").unwrap();
        if bucket_part.is_empty() {
            return Err(horcrux_common::Error::InvalidConfig(
                "S3 pool path must specify bucket name".to_string()
            ));
        }

        // Basic bucket name validation completed in mod.rs
        // For now, we can't validate the actual connection without credentials
        // which are stored separately from the pool configuration
        tracing::info!("S3 storage pool validation passed (offline check): {}", pool.path);

        Ok(())
    }

    /// Validate S3 configuration
    pub async fn validate_config(&self, config: &S3Config) -> Result<()> {
        // Test connection by listing bucket
        self.list_objects(config, None, Some(1)).await?;
        Ok(())
    }

    /// Upload file to S3
    pub async fn upload_file(
        &self,
        config: &S3Config,
        local_path: &str,
        s3_key: &str,
    ) -> Result<String> {
        tracing::info!("Uploading {} to S3 bucket {} as {}", local_path, config.bucket, s3_key);

        // Read file
        let data = tokio::fs::read(local_path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read file: {}", e))
        })?;

        // Build URL
        let url = self.build_url(config, s3_key);

        // Build authorization header (simplified - real implementation needs AWS Signature V4)
        let auth_header = format!("AWS {}:{}", config.access_key, config.secret_key);

        // Upload
        let response = self.client
            .put(&url)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Upload failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(horcrux_common::Error::System(
                format!("S3 upload failed ({}): {}", status, error_text)
            ));
        }

        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        tracing::info!("Successfully uploaded to S3, ETag: {}", etag);

        Ok(etag)
    }

    /// Download file from S3
    pub async fn download_file(
        &self,
        config: &S3Config,
        s3_key: &str,
        local_path: &str,
    ) -> Result<()> {
        tracing::info!("Downloading {} from S3 bucket {} to {}", s3_key, config.bucket, local_path);

        let url = self.build_url(config, s3_key);
        let auth_header = format!("AWS {}:{}", config.access_key, config.secret_key);

        let response = self.client
            .get(&url)
            .header("Authorization", auth_header)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Download failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(
                format!("S3 download failed: {}", status)
            ));
        }

        let bytes = response.bytes().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read response: {}", e))
        })?;

        // Create parent directories
        if let Some(parent) = PathBuf::from(local_path).parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create directories: {}", e))
            })?;
        }

        tokio::fs::write(local_path, &bytes).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to write file: {}", e))
        })?;

        tracing::info!("Successfully downloaded from S3");

        Ok(())
    }

    /// Delete object from S3
    pub async fn delete_object(
        &self,
        config: &S3Config,
        s3_key: &str,
    ) -> Result<()> {
        tracing::info!("Deleting {} from S3 bucket {}", s3_key, config.bucket);

        let url = self.build_url(config, s3_key);
        let auth_header = format!("AWS {}:{}", config.access_key, config.secret_key);

        let response = self.client
            .delete(&url)
            .header("Authorization", auth_header)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Delete failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(
                format!("S3 delete failed: {}", status)
            ));
        }

        tracing::info!("Successfully deleted from S3");

        Ok(())
    }

    /// List objects in bucket
    pub async fn list_objects(
        &self,
        config: &S3Config,
        prefix: Option<&str>,
        max_keys: Option<usize>,
    ) -> Result<Vec<S3Object>> {
        let mut url = self.build_url(config, "");
        url.push_str("?list-type=2");

        if let Some(p) = prefix {
            url.push_str(&format!("&prefix={}", urlencoding::encode(p)));
        }

        if let Some(max) = max_keys {
            url.push_str(&format!("&max-keys={}", max));
        }

        let auth_header = format!("AWS {}:{}", config.access_key, config.secret_key);

        let response = self.client
            .get(&url)
            .header("Authorization", auth_header)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("List failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(
                format!("S3 list failed: {}", status)
            ));
        }

        let body = response.text().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read response: {}", e))
        })?;

        // Parse XML response (simplified - real implementation should use proper XML parser)
        let objects = self.parse_list_response(&body)?;

        Ok(objects)
    }

    /// Get object metadata
    pub async fn head_object(
        &self,
        config: &S3Config,
        s3_key: &str,
    ) -> Result<S3Object> {
        let url = self.build_url(config, s3_key);
        let auth_header = format!("AWS {}:{}", config.access_key, config.secret_key);

        let response = self.client
            .head(&url)
            .header("Authorization", auth_header)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("HEAD request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(
                format!("S3 HEAD failed: {}", status)
            ));
        }

        let headers = response.headers();

        let size = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let etag = headers
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let last_modified = headers
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        Ok(S3Object {
            key: s3_key.to_string(),
            size,
            etag,
            last_modified,
            storage_class: None,
        })
    }

    /// Create multipart upload for large files
    pub async fn create_multipart_upload(
        &self,
        config: &S3Config,
        s3_key: &str,
    ) -> Result<String> {
        let url = format!("{}?uploads", self.build_url(config, s3_key));
        let auth_header = format!("AWS {}:{}", config.access_key, config.secret_key);

        let response = self.client
            .post(&url)
            .header("Authorization", auth_header)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create multipart upload: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System("Failed to create multipart upload".to_string()));
        }

        let body = response.text().await.unwrap_or_default();

        // Extract upload ID from XML response
        let upload_id = body
            .split("<UploadId>")
            .nth(1)
            .and_then(|s| s.split("</UploadId>").next())
            .ok_or_else(|| horcrux_common::Error::System("Invalid multipart upload response".to_string()))?
            .to_string();

        Ok(upload_id)
    }

    /// Build S3 URL
    fn build_url(&self, config: &S3Config, key: &str) -> String {
        let protocol = if config.use_ssl { "https" } else { "http" };

        if config.use_path_style {
            // Path-style: https://s3.endpoint.com/bucket/key
            format!("{}://{}/{}/{}",
                protocol,
                config.endpoint,
                config.bucket,
                key
            )
        } else {
            // Virtual-hosted-style: https://bucket.s3.endpoint.com/key
            format!("{}://{}.{}{}",
                protocol,
                config.bucket,
                config.endpoint,
                if key.is_empty() { "/" } else { &format!("/{}", key) }
            )
        }
    }

    /// Parse S3 list response (simplified XML parsing)
    fn parse_list_response(&self, xml: &str) -> Result<Vec<S3Object>> {
        let mut objects = Vec::new();

        // Very basic XML parsing - production should use proper XML library
        for content in xml.split("<Contents>").skip(1) {
            if let Some(end) = content.find("</Contents>") {
                let content_block = &content[..end];

                let key = self.extract_xml_value(content_block, "Key").unwrap_or_default();
                let size = self.extract_xml_value(content_block, "Size")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let etag = self.extract_xml_value(content_block, "ETag").unwrap_or_default();
                let last_modified = self.extract_xml_value(content_block, "LastModified").unwrap_or_default();
                let storage_class = self.extract_xml_value(content_block, "StorageClass");

                objects.push(S3Object {
                    key,
                    size,
                    etag,
                    last_modified,
                    storage_class,
                });
            }
        }

        Ok(objects)
    }

    /// Extract value from XML tag
    fn extract_xml_value(&self, xml: &str, tag: &str) -> Option<String> {
        let start_tag = format!("<{}>", tag);
        let end_tag = format!("</{}>", tag);

        xml.split(&start_tag)
            .nth(1)
            .and_then(|s| s.split(&end_tag).next())
            .map(|s| s.to_string())
    }
}

impl S3Config {
    /// Create MinIO configuration
    pub fn minio(endpoint: &str, access_key: &str, secret_key: &str, bucket: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            region: "us-east-1".to_string(),
            bucket: bucket.to_string(),
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            use_path_style: true,
            use_ssl: false,
        }
    }

    /// Create AWS S3 configuration
    pub fn aws_s3(region: &str, access_key: &str, secret_key: &str, bucket: &str) -> Self {
        Self {
            endpoint: format!("s3.{}.amazonaws.com", region),
            region: region.to_string(),
            bucket: bucket.to_string(),
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            use_path_style: false,
            use_ssl: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_url_path_style() {
        let manager = S3Manager::new();
        let config = S3Config::minio("localhost:9000", "key", "secret", "mybucket");

        let url = manager.build_url(&config, "myfile.txt");
        assert_eq!(url, "http://localhost:9000/mybucket/myfile.txt");
    }

    #[test]
    fn test_build_url_virtual_hosted() {
        let manager = S3Manager::new();
        let config = S3Config::aws_s3("us-east-1", "key", "secret", "mybucket");

        let url = manager.build_url(&config, "myfile.txt");
        assert_eq!(url, "https://mybucket.s3.us-east-1.amazonaws.com/myfile.txt");
    }

    #[test]
    fn test_parse_list_response() {
        let manager = S3Manager::new();
        let xml = r#"
        <?xml version="1.0"?>
        <ListBucketResult>
            <Contents>
                <Key>test.txt</Key>
                <Size>1024</Size>
                <ETag>"abc123"</ETag>
                <LastModified>2025-10-08T10:00:00.000Z</LastModified>
            </Contents>
        </ListBucketResult>
        "#;

        let objects = manager.parse_list_response(xml).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].key, "test.txt");
        assert_eq!(objects[0].size, 1024);
    }
}

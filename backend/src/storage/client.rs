use reqwest::Client as HttpClient;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use thiserror::Error;
use tracing::debug;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("S3 error: {0}")]
    S3Error(String),

    #[error("Failed to download image: {0}")]
    DownloadError(String),

    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    #[error("Image not found: {0}")]
    NotFound(String),

    #[error("HTTP error {status}: {message}")]
    HttpError { status: u16, message: String },
}

impl From<s3::error::S3Error> for StorageError {
    fn from(err: s3::error::S3Error) -> Self {
        StorageError::S3Error(err.to_string())
    }
}

/// Result of downloading a single image
pub struct DownloadResult {
    /// S3 key where image was stored (if successful)
    pub s3_key: Option<String>,
    /// HTTP status code from source
    pub http_status: Option<u16>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Whether download was successful
    pub success: bool,
}

/// Client for MinIO (S3-compatible storage)
pub struct StorageClient {
    bucket: Box<Bucket>,
    http_client: HttpClient,
    image_path_template: String,
}

impl StorageClient {
    /// Create client from environment variables
    pub fn from_env(image_path_template: String) -> Result<Self, StorageError> {
        let endpoint =
            std::env::var("MINIO_ENDPOINT").unwrap_or_else(|_| "http://localhost:9000".to_string());
        let access_key = std::env::var("MINIO_ACCESS_KEY")
            .map_err(|_| StorageError::ConfigError("MINIO_ACCESS_KEY not set".to_string()))?;
        let secret_key = std::env::var("MINIO_SECRET_KEY")
            .map_err(|_| StorageError::ConfigError("MINIO_SECRET_KEY not set".to_string()))?;
        let bucket_name = std::env::var("MINIO_BUCKET")
            .map_err(|_| StorageError::ConfigError("MINIO_BUCKET not set".to_string()))?;

        Self::new(
            &endpoint,
            &access_key,
            &secret_key,
            &bucket_name,
            image_path_template,
        )
    }

    /// Create client with explicit parameters
    pub fn new(
        endpoint: &str,
        access_key: &str,
        secret_key: &str,
        bucket_name: &str,
        image_path_template: String,
    ) -> Result<Self, StorageError> {
        let region = Region::Custom {
            region: "us-east-1".to_string(),
            endpoint: endpoint.to_string(),
        };

        let credentials = Credentials::new(Some(access_key), Some(secret_key), None, None, None)
            .map_err(|e| StorageError::ConfigError(e.to_string()))?;

        let bucket = Bucket::new(bucket_name, region, credentials)
            .map_err(|e| StorageError::S3Error(e.to_string()))?
            .with_path_style();

        let http_client = HttpClient::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| StorageError::ConfigError(e.to_string()))?;

        Ok(Self {
            bucket,
            http_client,
            image_path_template,
        })
    }

    /// Ensure bucket exists
    pub async fn ensure_bucket_exists(&self) -> Result<(), StorageError> {
        match self
            .bucket
            .list("".to_string(), Some("/".to_string()))
            .await
        {
            Ok(_) => Ok(()),
            Err(_) => {
                let creds = self.bucket.credentials().await.map_err(|e| {
                    StorageError::S3Error(format!("Failed to get credentials: {}", e))
                })?;
                Bucket::create_with_path_style(
                    &self.bucket.name(),
                    self.bucket.region().clone(),
                    creds,
                    s3::bucket_ops::BucketConfiguration::default(),
                )
                .await
                .map_err(|e| StorageError::S3Error(format!("Failed to create bucket: {}", e)))?;
                Ok(())
            }
        }
    }

    /// Generate image path from template
    fn image_path(&self, listing_id: i64, index: i32) -> String {
        self.image_path_template
            .replace("{listing_id}", &listing_id.to_string())
            .replace("{index}", &index.to_string())
    }

    /// Download and store a single image, returning detailed result
    pub async fn download_and_store_single(
        &self,
        listing_id: i64,
        index: i32,
        url: &str,
    ) -> DownloadResult {
        let response = match self.http_client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                return DownloadResult {
                    s3_key: None,
                    http_status: None,
                    error: Some(format!("Request failed: {}", e)),
                    success: false,
                };
            }
        };

        let status = response.status().as_u16();

        if !response.status().is_success() {
            return DownloadResult {
                s3_key: None,
                http_status: Some(status),
                error: Some(format!("HTTP {}", status)),
                success: false,
            };
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return DownloadResult {
                    s3_key: None,
                    http_status: Some(status),
                    error: Some(format!("Failed to read body: {}", e)),
                    success: false,
                };
            }
        };

        let path = self.image_path(listing_id, index);

        if let Err(e) = self
            .bucket
            .put_object_with_content_type(&path, &bytes, &content_type)
            .await
        {
            return DownloadResult {
                s3_key: None,
                http_status: Some(status),
                error: Some(format!("S3 upload failed: {}", e)),
                success: false,
            };
        }

        debug!("Stored image {}/{} to {}", listing_id, index, path);

        DownloadResult {
            s3_key: Some(path),
            http_status: Some(status),
            error: None,
            success: true,
        }
    }

    /// Get image from storage
    pub async fn get_image(&self, listing_id: i64, index: i32) -> Result<Vec<u8>, StorageError> {
        let path = self.image_path(listing_id, index);
        let response = self.bucket.get_object(&path).await?;

        if response.status_code() == 404 {
            return Err(StorageError::NotFound(path));
        }

        Ok(response.to_vec())
    }

    /// Get image by S3 key
    pub async fn get_image_by_key(&self, s3_key: &str) -> Result<Vec<u8>, StorageError> {
        let response = self.bucket.get_object(s3_key).await?;

        if response.status_code() == 404 {
            return Err(StorageError::NotFound(s3_key.to_string()));
        }

        Ok(response.to_vec())
    }

    /// Check if images exist for listing
    pub async fn has_images(&self, listing_id: i64) -> Result<bool, StorageError> {
        let prefix = format!("{}/", listing_id);
        let list = self.bucket.list(prefix, Some("/".to_string())).await?;

        Ok(list.iter().any(|r| !r.contents.is_empty()))
    }

    /// Count images for listing
    pub async fn count_images(&self, listing_id: i64) -> Result<usize, StorageError> {
        let prefix = format!("{}/", listing_id);
        let list = self.bucket.list(prefix, Some("/".to_string())).await?;

        Ok(list.iter().map(|r| r.contents.len()).sum())
    }

    /// Delete all images for listing
    pub async fn delete_images(&self, listing_id: i64) -> Result<usize, StorageError> {
        let prefix = format!("{}/", listing_id);
        let list = self
            .bucket
            .list(prefix.clone(), Some("/".to_string()))
            .await?;

        let keys: Vec<String> = list
            .iter()
            .flat_map(|r| r.contents.iter())
            .map(|obj| obj.key.clone())
            .collect();

        let count = keys.len();

        for key in keys {
            self.bucket.delete_object(&key).await?;
        }

        Ok(count)
    }

    /// Get presigned URL for image
    pub async fn get_presigned_url(
        &self,
        listing_id: i64,
        index: i32,
        expires_secs: u32,
    ) -> Result<String, StorageError> {
        let path = self.image_path(listing_id, index);
        self.bucket
            .presign_get(&path, expires_secs, None)
            .await
            .map_err(|e| StorageError::S3Error(e.to_string()))
    }

    /// Get presigned URL by S3 key
    pub async fn get_presigned_url_by_key(
        &self,
        s3_key: &str,
        expires_secs: u32,
    ) -> Result<String, StorageError> {
        self.bucket
            .presign_get(s3_key, expires_secs, None)
            .await
            .map_err(|e| StorageError::S3Error(e.to_string()))
    }
}

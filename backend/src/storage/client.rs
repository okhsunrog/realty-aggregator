use futures::future::join_all;
use reqwest::Client as HttpClient;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use thiserror::Error;
use tracing::{debug, warn};

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
}

impl From<s3::error::S3Error> for StorageError {
    fn from(err: s3::error::S3Error) -> Self {
        StorageError::S3Error(err.to_string())
    }
}

/// Клиент для работы с MinIO (S3-совместимое хранилище)
pub struct StorageClient {
    bucket: Box<Bucket>,
    http_client: HttpClient,
    image_path_template: String,
}

impl StorageClient {
    /// Создать клиент из переменных окружения
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

    /// Создать клиент с явными параметрами
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

    /// Создать bucket если не существует
    pub async fn ensure_bucket_exists(&self) -> Result<(), StorageError> {
        // Пробуем получить список объектов - если ошибка, создаём bucket
        match self
            .bucket
            .list("".to_string(), Some("/".to_string()))
            .await
        {
            Ok(_) => Ok(()),
            Err(_) => {
                // Bucket не существует, создаём
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

    /// Сформировать путь для изображения
    fn image_path(&self, listing_id: i64, index: usize) -> String {
        self.image_path_template
            .replace("{listing_id}", &listing_id.to_string())
            .replace("{index}", &index.to_string())
    }

    /// Скачать изображения с внешних URL и сохранить в MinIO
    /// Возвращает список успешно сохранённых индексов
    pub async fn download_and_store_images(
        &self,
        listing_id: i64,
        image_urls: &[String],
    ) -> Result<Vec<usize>, StorageError> {
        if image_urls.is_empty() {
            return Ok(vec![]);
        }

        debug!(
            "Downloading {} images for listing {}",
            image_urls.len(),
            listing_id
        );

        // Скачиваем все изображения параллельно
        let download_futures: Vec<_> = image_urls
            .iter()
            .enumerate()
            .map(|(idx, url)| self.download_and_store_single(listing_id, idx, url))
            .collect();

        let results = join_all(download_futures).await;

        // Собираем успешные индексы
        let successful: Vec<usize> = results
            .into_iter()
            .enumerate()
            .filter_map(|(idx, result)| match result {
                Ok(()) => Some(idx),
                Err(e) => {
                    warn!(
                        "Failed to download image {} for listing {}: {}",
                        idx, listing_id, e
                    );
                    None
                }
            })
            .collect();

        debug!(
            "Successfully stored {}/{} images for listing {}",
            successful.len(),
            image_urls.len(),
            listing_id
        );

        Ok(successful)
    }

    /// Скачать и сохранить одно изображение
    async fn download_and_store_single(
        &self,
        listing_id: i64,
        index: usize,
        url: &str,
    ) -> Result<(), StorageError> {
        // Скачиваем
        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| StorageError::DownloadError(format!("{}: {}", url, e)))?;

        if !response.status().is_success() {
            return Err(StorageError::DownloadError(format!(
                "{}: HTTP {}",
                url,
                response.status()
            )));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| StorageError::DownloadError(format!("{}: {}", url, e)))?;

        // Сохраняем в MinIO
        let path = self.image_path(listing_id, index);
        self.bucket
            .put_object_with_content_type(&path, &bytes, &content_type)
            .await?;

        Ok(())
    }

    /// Получить изображение из хранилища
    pub async fn get_image(&self, listing_id: i64, index: usize) -> Result<Vec<u8>, StorageError> {
        let path = self.image_path(listing_id, index);
        let response = self.bucket.get_object(&path).await?;

        if response.status_code() == 404 {
            return Err(StorageError::NotFound(path));
        }

        Ok(response.to_vec())
    }

    /// Получить все изображения для объявления
    pub async fn get_all_images(&self, listing_id: i64) -> Result<Vec<Vec<u8>>, StorageError> {
        let prefix = format!("{}/", listing_id);
        let list = self
            .bucket
            .list(prefix.clone(), Some("/".to_string()))
            .await?;

        let mut images = Vec::new();

        for item in list.iter().flat_map(|r| r.contents.iter()) {
            let response = self.bucket.get_object(&item.key).await?;
            if response.status_code() == 200 {
                images.push(response.to_vec());
            }
        }

        Ok(images)
    }

    /// Проверить, есть ли изображения для объявления
    pub async fn has_images(&self, listing_id: i64) -> Result<bool, StorageError> {
        let prefix = format!("{}/", listing_id);
        let list = self.bucket.list(prefix, Some("/".to_string())).await?;

        Ok(list.iter().any(|r| !r.contents.is_empty()))
    }

    /// Получить количество изображений для объявления
    pub async fn count_images(&self, listing_id: i64) -> Result<usize, StorageError> {
        let prefix = format!("{}/", listing_id);
        let list = self.bucket.list(prefix, Some("/".to_string())).await?;

        Ok(list.iter().map(|r| r.contents.len()).sum())
    }

    /// Удалить все изображения объявления
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

    /// Получить presigned URL для изображения (для отдачи клиенту)
    pub async fn get_presigned_url(
        &self,
        listing_id: i64,
        index: usize,
        expires_secs: u32,
    ) -> Result<String, StorageError> {
        let path = self.image_path(listing_id, index);
        self.bucket
            .presign_get(&path, expires_secs, None)
            .await
            .map_err(|e| StorageError::S3Error(e.to_string()))
    }
}

use std::future::Future;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Error;

use super::BackupReader;
use crate::backup::{AsyncReadChunk, CryptConfig, DataBlob, ReadChunk};
use crate::tools::runtime::block_on;

/// Read chunks from remote host using ``BackupReader``
#[derive(Clone)]
pub struct RemoteChunkReader {
    client: Arc<BackupReader>,
    crypt_config: Option<Arc<CryptConfig>>,
    cache_hint: HashMap<[u8; 32], usize>,
    cache: Arc<Mutex<HashMap<[u8; 32], Vec<u8>>>>,
}

impl RemoteChunkReader {
    /// Create a new instance.
    ///
    /// Chunks listed in ``cache_hint`` are cached and kept in RAM.
    pub fn new(
        client: Arc<BackupReader>,
        crypt_config: Option<Arc<CryptConfig>>,
        cache_hint: HashMap<[u8; 32], usize>,
    ) -> Self {
        Self {
            client,
            crypt_config,
            cache_hint,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Downloads raw chunk. This only verifies the (untrusted) CRC32, use
    /// DataBlob::verify_unencrypted or DataBlob::decode before storing/processing further.
    pub async fn read_raw_chunk(&self, digest: &[u8; 32]) -> Result<DataBlob, Error> {
        let mut chunk_data = Vec::with_capacity(4 * 1024 * 1024);

        self.client
            .download_chunk(&digest, &mut chunk_data)
            .await?;

        let chunk = DataBlob::load_from_reader(&mut &chunk_data[..])?;

        Ok(chunk)
    }
}

impl ReadChunk for RemoteChunkReader {
    fn read_raw_chunk(&self, digest: &[u8; 32]) -> Result<DataBlob, Error> {
        block_on(Self::read_raw_chunk(self, digest))
    }

    fn read_chunk(&self, digest: &[u8; 32]) -> Result<Vec<u8>, Error> {
        if let Some(raw_data) = (*self.cache.lock().unwrap()).get(digest) {
            return Ok(raw_data.to_vec());
        }

        let chunk = ReadChunk::read_raw_chunk(self, digest)?;

        let raw_data = chunk.decode(self.crypt_config.as_ref().map(Arc::as_ref), Some(digest))?;

        let use_cache = self.cache_hint.contains_key(digest);
        if use_cache {
            (*self.cache.lock().unwrap()).insert(*digest, raw_data.to_vec());
        }

        Ok(raw_data)
    }
}

impl AsyncReadChunk for RemoteChunkReader {
    fn read_raw_chunk<'a>(
        &'a self,
        digest: &'a [u8; 32],
    ) -> Pin<Box<dyn Future<Output = Result<DataBlob, Error>> + Send + 'a>> {
        Box::pin(Self::read_raw_chunk(self, digest))
    }

    fn read_chunk<'a>(
        &'a self,
        digest: &'a [u8; 32],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, Error>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(raw_data) = (*self.cache.lock().unwrap()).get(digest) {
                return Ok(raw_data.to_vec());
            }

            let chunk = Self::read_raw_chunk(self, digest).await?;

            let raw_data = chunk.decode(self.crypt_config.as_ref().map(Arc::as_ref), Some(digest))?;

            let use_cache = self.cache_hint.contains_key(digest);
            if use_cache {
                (*self.cache.lock().unwrap()).insert(*digest, raw_data.to_vec());
            }

            Ok(raw_data)
        })
    }
}

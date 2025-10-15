use crate::error::DownloadError;
use digest::Digest;
use log::debug;
use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::Sha256;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChecksumAlgorithm {
    MD5,
    SHA1,
    SHA256,
    NONE,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub value: Option<String>,
}

impl DownloadChecksum {
    pub fn verify(&self, file_path: &Path) -> Result<bool, DownloadError> {
        let expected = match &self.value {
            Some(v) => v,
            None => {
                debug!(
                    "[Checksum] No expected value for file: {:?}, skipping verification",
                    file_path
                );
                return Ok(true); // 没有期望值则跳过
            }
        };

        debug!(
            "[Checksum] Verifying file: {:?} using algorithm: {:?}",
            file_path, self.algorithm
        );

        let actual = match self.algorithm {
            ChecksumAlgorithm::MD5 => calculate_md5(file_path)?,
            ChecksumAlgorithm::SHA1 => calculate_sha1(file_path)?,
            ChecksumAlgorithm::SHA256 => calculate_sha256(file_path)?,
            ChecksumAlgorithm::NONE => {
                debug!(
                    "[Checksum] Algorithm NONE for file: {:?}, skipping verification",
                    file_path
                );
                return Ok(true);
            }
        };

        let result = &actual == expected;
        debug!(
            "[Checksum] File: {:?}, Algorithm: {:?}, Expected: {}, Actual: {}, Result: {}",
            file_path, self.algorithm, expected, actual, result
        );

        Ok(result)
    }
}

fn calculate_md5(file_path: &Path) -> Result<String, DownloadError> {
    debug!("[Checksum] Calculating MD5 for file: {:?}", file_path);
    let file = File::open(file_path).map_err(|e| DownloadError::Other(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Md5::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| DownloadError::Other(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = format!("{:x}", hasher.finalize());
    debug!("[Checksum] MD5 result for file {:?}: {}", file_path, hash);
    Ok(hash)
}

fn calculate_sha1(file_path: &Path) -> Result<String, DownloadError> {
    debug!("[Checksum] Calculating SHA1 for file: {:?}", file_path);
    let file = File::open(file_path).map_err(|e| DownloadError::Other(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| DownloadError::Other(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = format!("{:x}", hasher.finalize());
    debug!("[Checksum] SHA1 result for file {:?}: {}", file_path, hash);
    Ok(hash)
}

fn calculate_sha256(file_path: &Path) -> Result<String, DownloadError> {
    debug!("[Checksum] Calculating SHA256 for file: {:?}", file_path);
    let file = File::open(file_path).map_err(|e| DownloadError::Other(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| DownloadError::Other(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = format!("{:x}", hasher.finalize());
    debug!(
        "[Checksum] SHA256 result for file {:?}: {}",
        file_path, hash
    );
    Ok(hash)
}

use crate::checksum::Checksum;
use crate::error::Error;
use chrono::Utc;
use std::path::Path;

pub(crate) fn verify_file_checksums(
    checksums: &mut [Checksum],
    file_path: &Path,
) -> Result<bool, Error> {
    for checksum in checksums.iter_mut() {
        match checksum.verify(file_path) {
            Ok(true) => {
                checksum.verified = Some(true);
                checksum.verified_at = Some(Utc::now());
            }
            Ok(false) => {
                checksum.verified = Some(false);
                checksum.verified_at = Some(Utc::now());
                return Ok(false);
            }
            Err(err) => {
                checksum.verified = Some(false);
                checksum.verified_at = Some(Utc::now());
                return Err(err);
            }
        }
    }

    Ok(true)
}
